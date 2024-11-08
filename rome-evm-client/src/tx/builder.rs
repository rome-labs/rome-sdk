use crate::{error::{ProgramResult, RomeEvmError}, indexer::log_parser,
            tx::{
                utils::build_solana_tx, AtomicTx, AtomicTxHolder, IterativeTx, IterativeTxHolder
            }, Payer, ResourceFactory, Resource,
};
use bincode::serialize;
use emulator::{emulate, Emulation};
use ethers::types::{Bytes, TxHash};
use rome_evm::{ExitReason, NUMBER_OPCODES_PER_TX};
use rome_solana::{batch::AdvanceTx, types::SyncAtomicRpcClient};
use serde_json::json;
use solana_client::rpc_config::RpcSendTransactionConfig;
use solana_program::{
    entrypoint::MAX_PERMITTED_DATA_INCREASE,
    hash::Hash,
    instruction::{AccountMeta, Instruction},
};
use solana_sdk::{
    bs58, packet::PACKET_DATA_SIZE, pubkey::Pubkey, transaction::Transaction,
};
use solana_transaction_status::UiTransactionEncoding;

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

    pub async fn lock_resource(&self) -> ProgramResult<Resource> {
        self.resource_factory.get().await
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
        Self::check_emulation(&emulation)?;

        Ok(emulation)
    }

    // check for revert
    pub fn check_emulation(emulation: &Emulation) -> ProgramResult<()> {
        if let Some(vm) = emulation.vm.as_ref() {
            return match vm.exit_reason {
                ExitReason::Succeed(_) => Ok(()),
                ExitReason::Revert(_) => {
                    let mes = vm
                        .return_value
                        .as_ref()
                        .and_then(|value| log_parser::decode_revert(value))
                        .unwrap_or_default();
                    let data = vm.return_value.clone().unwrap_or_default();
                    Err(RomeEvmError::Revert(mes, data))
                }

                exit_reason => Err(RomeEvmError::ExitReason(exit_reason)),
            };
        }
        Ok(())
    }

    fn build_atomic(
        &self,
        atomic_tx: AtomicTx,
        rlp: Bytes,
        tx_hash: TxHash,
    ) -> ProgramResult<Box<dyn AdvanceTx<'_, Error = RomeEvmError>>> {

        let ix = atomic_tx.ix.as_ref().unwrap();
        let tx = build_solana_tx(Hash::default(), &atomic_tx.resource.payer(), ix)?;

        if is_holder_needed(&tx)? {
            tracing::info!("Atomic Tx Holder needed");
            let atomix_tx_holder = AtomicTxHolder::new(
                self.clone(),
                atomic_tx.resource,
                rlp,
                tx_hash
            );

            Ok(Box::new(atomix_tx_holder))
        } else {
            tracing::info!("Simple Atomic Tx");

            Ok(Box::new(atomic_tx))
        }
    }

    fn build_iterative(
        &self,
        resource: Resource,
        rlp: Bytes,
        tx_hash: TxHash,
    ) -> ProgramResult<Box<dyn AdvanceTx<'_, Error = RomeEvmError>>> {

        let mut iterative_tx = IterativeTx::new(self.clone(), resource, rlp.clone())?;
        iterative_tx.ixs()?;
        let ixs = iterative_tx.ixs.as_ref().unwrap();

        let tx = build_solana_tx(
            Hash::default(),
            &iterative_tx.resource.payer(),
            ixs.last().expect("no instructions in iterative Tx"),
        )?;

        if is_holder_needed(&tx)? {
            tracing::info!("Iterative Tx Holder needed");

            let iterative_tx_holder = IterativeTxHolder::new(
                self.clone(),
                iterative_tx.resource,
                rlp,
                tx_hash
            );

            Ok(Box::new(iterative_tx_holder))
        } else {
            tracing::info!("Iterative Tx without Holder needed");

            Ok(Box::new(iterative_tx))
        }
    }

    /// Build a transaction
    #[tracing::instrument(skip(self, rlp))]
    pub async fn build_tx(
        &self,
        rlp: Bytes,
        tx_hash: TxHash,
    ) -> ProgramResult<Box<dyn AdvanceTx<'_, Error = RomeEvmError>>> {

        // Lock a holder, payer
        let resource = self.lock_resource().await?;
        let mut atomic_tx = AtomicTx::new(self.clone(), rlp.to_vec(), resource);
        // Build the instruction
        atomic_tx.ix()?;
        let emulation = atomic_tx.emulation.as_ref().unwrap();
        let vm = emulation.vm.as_ref().expect("Vm expected");

        let is_atomic_tx = vm.steps_executed <= NUMBER_OPCODES_PER_TX
            && emulation.allocated <= MAX_PERMITTED_DATA_INCREASE
            && emulation.syscalls < 64;

        tracing::info!("Building Transaction");
        if is_atomic_tx {
            self.build_atomic(atomic_tx, rlp, tx_hash)
        } else {
            self.build_iterative(atomic_tx.resource, rlp, tx_hash)
        }
    }

    /// Build a Solapayerna instruction from [Emulation] and data
    pub fn build_ix(&self, emulation: &Emulation, data: Vec<u8>) -> Instruction {
        let accounts = emulation
            .accounts
            .iter()
            .map(|(pubkey, item)| AccountMeta {
                pubkey: *pubkey,
                is_signer: false,
                is_writable: item.writable,
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
            session
        )?)
    }
}

fn is_holder_needed(tx: &Transaction) -> ProgramResult<bool> {
    let json = serialize_encode(tx, UiTransactionEncoding::Base64)?;
    Ok(json.len() > PACKET_DATA_SIZE)
}

fn serialize_encode<T>(input: &T, encoding: UiTransactionEncoding) -> ProgramResult<String>
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
    let config = RpcSendTransactionConfig::default();
    let json = json!([encoded, config]);

    Ok(json.to_string())
}
