use crate::{
    error::{ProgramResult, RomeEvmError},
    indexer::log_parser,
    tx::{utils::build_solana_tx, AtomicTx, AtomicTxHolder, IterativeTx, IterativeTxHolder},
};
use bincode::serialize;
use emulator::{emulate, Emulation};
use ethers::types::{Bytes, TxHash};
use rome_evm::NUMBER_OPCODES_PER_TX;
use rome_solana::{batch::AdvanceTx, types::SyncAtomicRpcClient};
use rome_utils::holder::HolderFactory;
use serde_json::json;
use solana_client::rpc_config::RpcSendTransactionConfig;
use solana_program::{
    entrypoint::MAX_PERMITTED_DATA_INCREASE,
    hash::Hash,
    instruction::{AccountMeta, Instruction},
};
use solana_sdk::{
    bs58, packet::PACKET_DATA_SIZE, pubkey::Pubkey, signature::Keypair, signer::Signer,
    transaction::Transaction,
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
    /// Holder factory to create holder instances
    holder_factory: HolderFactory,
}

impl TxBuilder {
    /// Create a new Transaction Builder
    pub fn new(chain_id: u64, program_id: Pubkey, rpc_client: SyncAtomicRpcClient) -> Self {
        Self {
            chain_id,
            program_id,
            rpc_client,
            holder_factory: HolderFactory::default(),
        }
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
        Self::check_revert(&emulation)?;

        Ok(emulation)
    }

    // check for revert
    pub fn check_revert(emulation: &Emulation) ->ProgramResult<()> {
        if let Some(vm) = emulation.vm.as_ref() {
            if vm.exit_reason.is_revert() {
                let mes = vm
                    .return_value
                    .as_ref()
                    .and_then(|value| log_parser::decode_revert(value))
                    .unwrap_or_default();

                let data = vm.return_value.clone().unwrap_or_default();

                return Err(RomeEvmError::Revert(mes, data));
            }
        }
        Ok(())
    }

    /// Build a transaction
    #[tracing::instrument(skip(self, rlp, payer))]
    pub async fn build_tx(
        &self,
        rlp: Bytes,
        tx_hash: TxHash,
        payer: &Keypair,
    ) -> ProgramResult<Box<dyn AdvanceTx<'_, Error = RomeEvmError>>> {
        tracing::info!("Building Transaction with payer: {:?}", payer.pubkey());

        let payer_key = payer.pubkey();

        let mut atomic_tx = AtomicTx::new_owned(self.clone(), rlp.to_vec());
        // Build the instruction
        atomic_tx.ix(&payer_key)?;
        let emulation = atomic_tx.emulation.as_ref().unwrap();
        let vm = emulation.vm.as_ref().expect("Vm expected");

        let is_atomic_tx = vm.steps_executed <= NUMBER_OPCODES_PER_TX
            && emulation.allocated <= MAX_PERMITTED_DATA_INCREASE;

        tracing::info!(
            is_atomic_tx,
            steps_executed = vm.steps_executed,
            allocated = emulation.allocated,
            "Building Transaction", // Log message
        );

        // atomic tx
        if is_atomic_tx {
            let ix = atomic_tx.ix.as_ref().unwrap();

            // Build the transaction
            let tx = build_solana_tx(Hash::default(), payer, ix)?;

            // Check if the holder is needed
            if is_holder_needed(&tx)? {
                tracing::info!("Atomic Tx Holder needed");

                let holder = self.holder_factory.lock_holder().await;

                let atomix_tx_holder = AtomicTxHolder::new(self.clone(), holder, rlp, tx_hash);

                return Ok(Box::new(atomix_tx_holder));
            } else {
                tracing::info!("Simple Atomic Tx");
                return Ok(Box::new(atomic_tx));
            };
        }

        assert!(!is_atomic_tx);

        // Lock a holder
        let holder = self.holder_factory.lock_holder().await;

        // iterative tx
        let mut iterative_tx = IterativeTx::new(self.clone(), holder, rlp.clone())?;
        iterative_tx.ixs(&payer_key)?;
        let ixs = iterative_tx.ixs.as_ref().unwrap();

        let tx = build_solana_tx(
            Hash::default(),
            payer,
            ixs.last().expect("no instructions in iterative Tx"),
        )?;

        if is_holder_needed(&tx)? {
            tracing::info!("Iterative Tx Holder needed");

            let holder = iterative_tx.take_holder();

            let iterative_tx_holder = IterativeTxHolder::new(self.clone(), holder, rlp, tx_hash);

            Ok(Box::new(iterative_tx_holder))
        } else {
            tracing::info!("Iterative Tx without Holder needed");

            Ok(Box::new(iterative_tx))
        }
    }

    /// Build a Solana instruction from [Emulation] and data
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
