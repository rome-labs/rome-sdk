use {
    super::{
        atomic_tx::AtomicTx, atomic_tx_holder::AtomicTxHolder, iterative_tx::IterativeTx,
        iterative_tx_holder::IterativeTxHolder, transaction_signer::TransactionSigner,
        utils::build_solana_tx, RomeTx,
    },
    crate::{
        error::{Result, RomeEvmError::*},
        indexer::log_parser::decode_revert,
    },
    bincode::serialize,
    emulator::{emulate, Emulation},
    ethers::types::{Bytes, TxHash},
    rome_evm::NUMBER_OPCODES_PER_TX,
    serde_json::json,
    solana_client::{rpc_client::RpcClient, rpc_config::RpcSendTransactionConfig},
    solana_program::{
        entrypoint::MAX_PERMITTED_DATA_INCREASE,
        hash::Hash,
        instruction::{AccountMeta, Instruction},
    },
    solana_sdk::{
        bs58, packet::PACKET_DATA_SIZE, pubkey::Pubkey, signer::Signer, transaction::Transaction,
    },
    solana_transaction_status::UiTransactionEncoding,
    std::sync::Arc,
};

#[derive(Clone)]
pub struct TransactionBuilder {
    program_id: Pubkey,
    rpc_client: Arc<RpcClient>,
}

impl TransactionBuilder {
    pub fn new(program_id: Pubkey, rpc_client: Arc<RpcClient>) -> Self {
        Self {
            program_id,
            rpc_client: Arc::clone(&rpc_client),
        }
    }

    pub fn check_revert(emulation: &Emulation) ->Result<()> {
        if let Some(vm) = emulation.vm.as_ref() {
            if vm.exit_reason.is_revert() {
                let mes = decode_revert(vm.return_value.as_ref()).unwrap_or_default();
                let data = vm.return_value.clone().unwrap_or_default();

                return Err(Revert(mes, data));
            }
        }
        Ok(())
    }
    pub fn emulate(&self, data: &[u8], payer: &Pubkey) -> Result<Emulation> {
        let emulation = emulate(&self.program_id, data, payer, self.rpc_client.clone())?;
        Self::check_revert(&emulation)?;

        Ok(emulation)
    }

    pub fn build_transaction(
        &self,
        rlp: Bytes,
        tx_hash: TxHash,
        signer: &TransactionSigner,
    ) -> Result<Box<dyn RomeTx>> {
        let atomic_tx = AtomicTx::new(self.clone(), rlp.clone());
        let emulation = atomic_tx.emulate(&signer.keypair().pubkey())?;
        let vm = emulation.vm.as_ref().expect("Vm expected");

        // atomic tx or iterative tx
        if vm.steps_executed <= NUMBER_OPCODES_PER_TX
            && emulation.allocated <= MAX_PERMITTED_DATA_INCREASE
        {
            if let Some(ix) = atomic_tx.build_commit_instruction(&signer.keypair().pubkey())? {
                let tx = build_solana_tx(Hash::default(), signer.keypair(), &[ix])?;
                if is_holder_needed(&tx)? {
                    Ok(Box::new(AtomicTxHolder::new(
                        self.clone(),
                        signer.lock_holder()?,
                        rlp,
                        tx_hash,
                    )?))
                } else {
                    Ok(Box::new(atomic_tx))
                }
            } else {
                unreachable!("AtomicTx did not create commit instruction")
            }
        } else {
            // iterative tx
            let iterative_tx = IterativeTx::new(self.clone(), signer.lock_holder()?, rlp.clone())?;
            let ixs = iterative_tx.build_execute_instructions(&signer.keypair().pubkey())?;
            let tx = build_solana_tx(
                Hash::default(),
                signer.keypair(),
                &[ixs
                    .first()
                    .expect("no instructions in iterative Tx")
                    .clone()],
            )?;

            if is_holder_needed(&tx)? {
                Ok(Box::new(IterativeTxHolder::new(
                    self.clone(),
                    signer.lock_holder()?,
                    rlp,
                    tx_hash,
                )?))
            } else {
                Ok(Box::new(iterative_tx))
            }
        }
    }

    pub fn build_solana_instr(&self, emulation: &Emulation, data: &[u8]) -> Instruction {
        let mut meta = vec![];
        for (key, item) in &emulation.accounts {
            if item.writable {
                meta.push(AccountMeta::new(*key, false))
            } else {
                meta.push(AccountMeta::new_readonly(*key, false))
            }
        }

        Instruction::new_with_bytes(self.program_id, &data, meta)
    }
}

fn is_holder_needed(tx: &Transaction) -> Result<bool> {
    let json = serialize_encode(tx, UiTransactionEncoding::Base64)?;
    Ok(json.len() > PACKET_DATA_SIZE)
}

fn serialize_encode<T>(input: &T, encoding: UiTransactionEncoding) -> Result<String>
where
    T: serde::ser::Serialize,
{
    let serialized = serialize(input)?;

    let encoded = match encoding {
        UiTransactionEncoding::Base58 => bs58::encode(serialized).into_string(),
        UiTransactionEncoding::Base64 => base64::encode(serialized),
        _ => {
            tracing::warn!("unsupported encoding: {encoding}. Supported encodings: base58, base64");
            return Err(Custom(format!("unsupported tx encoding: {}", encoding)));
        }
    };
    let config = RpcSendTransactionConfig::default();
    let json = json!([encoded, config]);

    Ok(json.to_string())
}
