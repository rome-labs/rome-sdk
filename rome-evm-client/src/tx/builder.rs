use {
    crate::{
        error::{ProgramResult, RomeEvmError::{self, *}},
        tx::{
            AltComposed, AltComposedHolder, AltTx, AtomicTx, AtomicTxHolder, IterativeTx,
            IterativeTxHolder, TransmitTx, AtomicSvm,
        },
        util::{check_accounts_len, check_exit_reason},
        Payer, Resource, ResourceFactory,
    },
    bincode::serialize,
    emulator::{emulate, Emulation},
    ethers::types::{Bytes, TxHash},
    rome_solana::{
        batch::{AdvanceTx, OwnedAtomicIxBatch},
        types::SyncAtomicRpcClient,
    },
    serde_json::json,
    solana_client::rpc_config::RpcSendTransactionConfig,
    solana_program::{
        address_lookup_table::AddressLookupTableAccount,
        hash::Hash,
        instruction::{AccountMeta, Instruction},
    },
    solana_sdk::{
        bs58, commitment_config::CommitmentLevel, packet::PACKET_DATA_SIZE, pubkey::Pubkey,
        signer::keypair::Keypair,
    },
    solana_transaction_status::UiTransactionEncoding,
    std::sync::Arc,
};

pub type Iterable<'a> = Box<dyn AdvanceTx<'static, Error = RomeEvmError>>;
pub const USE_ALT_KEYS_BOUND: usize = 28; // depends on iterative_holder.tx_data() length

/// Transaction Builder for Rome EVM
#[derive(Clone)]
pub struct TxBuilder {
    /// Chain ID of a Rollup
    pub chain_id: u64,
    /// Program ID of the Rome EVM
    program_id: Pubkey,
    /// Synchronous Atomic RPC client
    rpc_client: SyncAtomicRpcClient,
    /// Resource factory to get Solana payer, fee_recipient, holder index
    resource_factory: ResourceFactory,
}

impl TxBuilder {
    /// Create a new Transaction Builder
    pub fn new(
        chain_id: u64,
        program_id: Pubkey,
        rpc_client: SyncAtomicRpcClient,
        payers: Vec<Payer>,
    ) -> Self {
        let resource_factory = ResourceFactory::from_payers(payers);

        Self {
            chain_id,
            program_id,
            rpc_client,
            resource_factory,
        }
    }

    pub async fn lock_resource(&self) -> ProgramResult<Arc<Resource>> {
        let resource = self.resource_factory.get().await?;
        Ok(Arc::new(resource))
    }

    /// get program id
    pub fn program_id(&self) -> &Pubkey {
        &self.program_id
    }

    /// Emulate a transaction
    #[tracing::instrument(skip(self, data))]
    pub fn emulate(&self, data: &[u8], payer: &Pubkey) -> ProgramResult<Emulation> {
        tracing::info!("Emulating Transaction with payer: {:?}", payer);

        let emulation = emulate(&self.program_id, data, payer, self.rpc_client.clone())?;
        check_exit_reason(&emulation)?;
        check_accounts_len(&emulation)?;

        Ok(emulation)
    }

    pub fn compose_iterable_with_holder(
        &self,
        use_alt: bool,
        keys: Vec<Pubkey>,
        transmit_tx: TransmitTx,
        iterable_tx: Iterable,
    ) -> ProgramResult<Box<dyn AdvanceTx<'static, Error = RomeEvmError>>> {
        if use_alt {
            tracing::info!("Address lookup table needed, Tx Holder needed");

            let alt_tx = AltTx::new(self.clone(), transmit_tx.resource.clone(), keys)?;
            let tx = AltComposedHolder::new(alt_tx, transmit_tx, iterable_tx)?;

            Ok(Box::new(tx))
        } else {
            tracing::info!("Tx Holder needed");
            Ok(iterable_tx)
        }
    }
    pub fn compose_iterable_without_holder(
        &self,
        use_alt: bool,
        keys: Vec<Pubkey>,
        resource: Arc<Resource>,
        iterable_tx: Iterable,
    ) -> ProgramResult<Box<dyn AdvanceTx<'static, Error = RomeEvmError>>> {
        if use_alt {
            tracing::info!("Address lookup table needed");
            let alt_tx = AltTx::new(self.clone(), resource, keys)?;
            let tx = AltComposed::new(alt_tx, iterable_tx)?;

            Ok(Box::new(tx))
        } else {
            Ok(iterable_tx)
        }
    }

    pub fn compose_iterable(
        &self,
        ix: &OwnedAtomicIxBatch,
        resource: Arc<Resource>,
        rlp: Bytes,
        hash: TxHash,
        iterable: Iterable,
        is_atomic: bool,
    ) -> ProgramResult<Iterable> {
        let keys = tx_keys(ix);
        let use_alt = use_alt(&keys);

        let alt = if use_alt {
            let acc = AddressLookupTableAccount {
                key: Pubkey::default(),
                addresses: keys.clone(),
            };
            Some(vec![acc])
        } else {
            None
        };

        if use_holder(
            ix,
            &resource.payer(),
            self.rpc_client.commitment().commitment,
            alt.as_ref(),
        )? {
            let transmit_tx = TransmitTx::new(self.clone(), resource.clone(), rlp, hash);

            let iterable_with_holder: Iterable = if is_atomic {
                Box::new(AtomicTxHolder::new(transmit_tx.clone(), use_alt))
            } else {
                Box::new(IterativeTxHolder::new(transmit_tx.clone(), use_alt))
            };

            self.compose_iterable_with_holder(use_alt, keys, transmit_tx, iterable_with_holder)
        } else {
            self.compose_iterable_without_holder(use_alt, keys, resource, iterable)
        }
    }
    /// Build a transaction
    #[tracing::instrument(skip(self, rlp))]
    pub async fn build_tx(&self, rlp: Bytes, hash: TxHash) -> ProgramResult<Iterable> {
        // Lock a holder, payer
        let resource = self.lock_resource().await?;
        let mut atomic_tx = AtomicTx::new(self.clone(), rlp.to_vec(), resource.clone());
        // Build the instruction
        atomic_tx.ix()?;

        let emulation = atomic_tx.emulation.as_ref().unwrap();

        let (ix, tx, is_atomic): (OwnedAtomicIxBatch, Iterable, bool) = if emulation.is_atomic {
            tracing::info!("Building atomic transaction");
            let ix = atomic_tx.ix.as_ref().unwrap().clone();

            (ix, Box::new(atomic_tx), true)
        } else {
            tracing::info!("Building iterative transaction");

            let mut iterative_tx = IterativeTx::new(self.clone(), resource.clone(), rlp.clone())?;
            iterative_tx.ixs()?;

            let ix = iterative_tx
                .ixs
                .as_ref()
                .unwrap()
                .last()
                .expect("no instructions in iterative Tx")
                .clone();

            (ix, Box::new(iterative_tx), false)
        };

        self.compose_iterable(&ix, resource, rlp, hash, tx, is_atomic)
    }

    /// Build a Solana instruction from [Emulation] and data
    pub fn build_ix(&self, emulation: &Emulation, data: Vec<u8>) -> Instruction {
        let accounts = emulation
            .accounts
            .iter()
            .map(|(pubkey, item)| AccountMeta {
                pubkey: *pubkey,
                is_signer: false,
                is_writable: item.account.writable,
            })
            .collect::<Vec<AccountMeta>>();

        Instruction {
            program_id: self.program_id,
            accounts,
            data,
        }
    }

    pub fn client_cloned(&self) -> SyncAtomicRpcClient {
        self.rpc_client.clone()
    }

    pub fn confirm_tx_iterative(
        &self,
        holder: u64,
        hash: TxHash,
        payer: &Pubkey,
        session: u64,
    ) -> ProgramResult<bool> {
        Ok(emulator::confirm_tx_iterative(
            &self.program_id,
            holder,
            rome_evm::H256::from_slice(hash.as_bytes()),
            payer,
            self.client_cloned(),
            self.chain_id,
            session,
        )?)
    }

    /// Build a composite transaction consisting of rome-evm instruction and SVM-instructions
    #[tracing::instrument(skip(self, rlp))]
    pub async fn build_svm_tx(
        &self, 
        rlp:Bytes, 
        svm: Vec<Instruction>, 
        alt_keys: Option<Vec<Pubkey>>
    ) -> ProgramResult<Iterable> {
        let resource = self.lock_resource().await?;
        let atomic_tx = AtomicTx::new(self.clone(), rlp.to_vec(), resource.clone());
        let atomic_svm = AtomicSvm::new(atomic_tx, svm, alt_keys)?;
        let ix = atomic_svm.ix();
        let alts = atomic_svm.alts.as_ref();
        let emulation = atomic_svm.emulation();

        // TODO: can be ignored 
        if !emulation.is_atomic {
            SvmCompositeTxError("transaction is not atomic".to_string());
        }
        
        if use_holder(
            ix,
            &resource.payer(),
            self.rpc_client.commitment().commitment,
            alts
        )? {
            SvmCompositeTxError("TxHolder account needed".to_string());
        }

        Ok(Box::new(atomic_svm))
    }
}

fn use_holder(
    ix: &OwnedAtomicIxBatch,
    payer: &Keypair,
    level: CommitmentLevel,
    alts: Option<&Vec<AddressLookupTableAccount>>,
) -> ProgramResult<bool> {
    let data_len = ix.iter().map(|a| a.data.len()).sum::<usize>();

    if data_len > i16::MAX as usize {
        return Ok(true);
    }

    let tx = if let Some(alts_) = alts {
        ix.compose_v0_solana_tx(payer, Hash::default(), alts_)?
    } else {
        ix.compose_legacy_solana_tx(payer, Hash::default())
    };

    let json = serialize_encode(&tx, UiTransactionEncoding::Base64, level)?;
    // subtract the additional 100 bytes to build json:
    // json!({
    //        "jsonrpc": jsonrpc,
    //        "id": 2.0,
    //        "method": format!("{self}"),
    //        "params": params,
    //     }
    Ok(json.len() > PACKET_DATA_SIZE - 100)
}

fn serialize_encode<T>(
    input: &T,
    encoding: UiTransactionEncoding,
    level: CommitmentLevel,
) -> ProgramResult<String>
where
    T: serde::ser::Serialize,
{
    let serialized = serialize(input)?;

    let encoded = match encoding {
        UiTransactionEncoding::Base58 => bs58::encode(serialized).into_string(),
        UiTransactionEncoding::Base64 => base64::encode(serialized),
        _ => {
            tracing::warn!("unsupported encoding: {encoding}. Supported encodings: base58, base64");
            return Err(RomeEvmError::Custom(format!(
                "unsupported tx encoding: {}",
                encoding
            )));
        }
    };
    let config = RpcSendTransactionConfig {
        encoding: Some(encoding),
        preflight_commitment: Some(level),
        ..Default::default()
    };
    let json = json!([encoded, config]);

    Ok(json.to_string())
}
fn tx_keys(ix: &[Instruction]) -> Vec<Pubkey> {
    ix.iter()
        .map(|x| {
            x.accounts
                .iter()
                .map(|meta| meta.pubkey)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>()
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
}
fn use_alt(keys: &Vec<Pubkey>) -> bool {
    keys.len() > USE_ALT_KEYS_BOUND
}

