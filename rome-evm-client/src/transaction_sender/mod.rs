pub mod atomic_tx;
pub mod atomic_tx_holder;
pub mod cross_rollup_tx;
pub mod iterative_tx;
pub mod iterative_tx_holder;
pub mod transaction_builder;
pub mod transaction_signer;
mod tx_batch;
pub mod utils;

use {
    super::transaction_sender::utils::{build_solana_tx, TRANSMIT_TX_SIZE},
    crate::error::Result,
    async_trait::async_trait,
    emulator::Emulation,
    solana_client::rpc_client::RpcClient,
    solana_program::{
        entrypoint::MAX_PERMITTED_DATA_INCREASE, instruction::Instruction, pubkey::Pubkey,
    },
    solana_sdk::{
        hash::Hash,
        signature::{Keypair, Signature},
        signer::Signer,
        transaction::Transaction,
    },
    std::sync::Arc,
    tokio_util::sync::CancellationToken,
    tx_batch::TxBatch,
};

#[async_trait]
pub trait RomeTx: Send + Sync {
    fn emulate(&self, payer: &Pubkey) -> Result<Emulation>;
    fn build_transmit_tx_instructions(&self, payer: &Pubkey) -> Result<Vec<Instruction>>;
    fn build_execute_instructions(&self, payer: &Pubkey) -> Result<Vec<Instruction>>;
    fn build_commit_instruction(&self, payer: &Pubkey) -> Result<Option<Instruction>>;
    fn cross_rollup_available(&self) -> bool;

    fn build_commit_transaction(
        &self,
        recent_blockhash: Hash,
        payer: &Keypair,
    ) -> Result<Option<Transaction>> {
        if let Some(instr) = self.build_commit_instruction(&payer.pubkey())? {
            Ok(Some(build_solana_tx(recent_blockhash, payer, &[instr])?))
        } else {
            Ok(None)
        }
    }

    async fn send(
        &self,
        rpc_client: Arc<RpcClient>,
        payer: Arc<Keypair>,
        token: CancellationToken,
    ) -> Result<Vec<Signature>> {
        // (Optional) Send TransmitTx steps
        let instructions = self.build_transmit_tx_instructions(&payer.pubkey())?;
        let mut signatures = if !instructions.is_empty() {
            let limit: usize = MAX_PERMITTED_DATA_INCREASE / TRANSMIT_TX_SIZE;
            let mut list = instructions.as_slice();
            while list.len() > limit {
                let (left, right) = list.split_at(limit);
                let batch = TxBatch::new(rpc_client.clone(), left, payer.clone())?;
                batch.send(token.clone()).await?;
                list = right;
            }

            let batch = TxBatch::new(rpc_client.clone(), list, payer.clone())?;
            batch.send(token.clone()).await?
        } else {
            vec![]
        };

        // (Optional) Send Execute steps
        let instructions = self.build_execute_instructions(&payer.pubkey())?;
        signatures.extend(if !instructions.is_empty() {
            let batch = TxBatch::new(rpc_client.clone(), instructions.as_slice(), payer.clone())?;
            batch.send(token.clone()).await?
        } else {
            vec![]
        });

        // (Optional) Send commit step
        if let Some(tx) =
            self.build_commit_transaction(rpc_client.get_latest_blockhash()?, &payer)?
        {
            signatures.push(rpc_client.send_and_confirm_transaction(&tx)?);
        }

        Ok(signatures)
    }
}
