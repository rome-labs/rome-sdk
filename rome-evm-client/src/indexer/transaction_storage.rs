use {
    crate::{
        error::ProgramResult,
        indexer::{indexed_transaction::IndexedTransaction, tx_parser::GasReport},
    },
    async_trait::async_trait,
    ethers::types::{
        transaction::eip2718::TypedTransaction, Signature as EthSignature, Transaction,
        TransactionReceipt, TxHash, H256, U256, U64,
    },
    solana_program::clock::Slot,
    solana_sdk::signature::Signature as SolSignature,
    solana_transaction_status::UiTransactionStatusMeta,
    std::{collections::LinkedList, sync::Arc},
};

#[async_trait]
pub trait BlockBuilder: Send + Sync {
    fn register_small_tx(
        &mut self,
        sol_slot_number: Slot,
        sol_tx_idx: usize,
        sol_signature: SolSignature,
        tx_request: TypedTransaction,
        eth_signature: EthSignature,
        sol_meta: UiTransactionStatusMeta,
    );

    fn register_big_tx(
        &mut self,
        evm_tx_hash: TxHash,
        sol_slot_number: Slot,
        sol_tx_idx: usize,
        sol_signature: SolSignature,
        sol_meta: UiTransactionStatusMeta,
    );

    #[allow(clippy::too_many_arguments)]
    fn register_transmit_tx(
        &mut self,
        evm_tx_hash: TxHash,
        sol_slot_number: Slot,
        holder_offset: usize,
        holder_data: Vec<u8>,
        sol_tx_idx: usize,
        sol_signature: SolSignature,
        meta: UiTransactionStatusMeta,
    );

    async fn end(mut self, slot_number: Slot) -> ProgramResult<LinkedList<TxHash>>;
}

#[async_trait]
pub trait TransactionStorage: Send + Sync {
    type TransactionType: IndexedTransaction;

    type BlockType<'a>: BlockBuilder
    where
        Self: 'a;

    async fn get_transaction(
        &self,
        tx_hash: &TxHash,
    ) -> ProgramResult<Option<Self::TransactionType>>;

    async fn get_full_txs(&self, tx_hashes: Arc<Vec<TxHash>>) -> ProgramResult<Vec<Transaction>>;

    async fn get_txs(&self, tx_hashes: Arc<Vec<TxHash>>) -> ProgramResult<Vec<TxHash>>;

    async fn get_full_txs_with_gas_report(
        &self,
        tx_hashes: Arc<Vec<TxHash>>,
    ) -> ProgramResult<Vec<(Transaction, GasReport)>>;

    async fn start_block<'a, 'b: 'a>(&'b self) -> ProgramResult<Self::BlockType<'a>>;

    async fn should_continue_parsing(&self, tx_hashes: &LinkedList<TxHash>) -> ProgramResult<bool>;

    async fn try_finalize_transaction(
        &self,
        tx_hash: &TxHash,
        log_index: U256,
        transaction_index: usize,
        block_hash: H256,
        block_number: U64,
        block_gas_used: U256,
    ) -> ProgramResult<Option<TransactionReceipt>>;
}
