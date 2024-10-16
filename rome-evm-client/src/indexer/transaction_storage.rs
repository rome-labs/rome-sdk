use {
    crate::{
        error::ProgramResult,
        indexer::transaction_data::{TransactionData, TransactionResult},
    },
    ethers::types::{
        transaction::eip2718::TypedTransaction, Signature as EthSignature, TransactionReceipt,
        TxHash, H256, U256, U64,
    },
    solana_program::clock::Slot,
    solana_sdk::signature::Signature as SolSignature,
    solana_transaction_status::UiTransactionStatusMeta,
    std::collections::{btree_map, BTreeMap},
};

#[derive(Default, Clone)]
pub struct TransactionStorage {
    transactions: BTreeMap<TxHash, TransactionData>,
}

impl TransactionStorage {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_transaction(&self, tx_hash: &TxHash) -> Option<&TransactionData> {
        self.transactions.get(tx_hash)
    }

    fn update_or_create_transaction<T>(
        &mut self,
        evm_tx_hash: TxHash,
        constructor: impl FnOnce() -> ProgramResult<TransactionData>,
        updater: impl FnOnce(&mut TransactionData) -> ProgramResult<T>,
    ) -> ProgramResult<T> {
        match self.transactions.entry(evm_tx_hash) {
            btree_map::Entry::Occupied(mut entry) => updater(entry.get_mut()),
            btree_map::Entry::Vacant(entry) => updater(entry.insert(constructor()?)),
        }
    }

    pub fn register_small_tx(
        &mut self,
        sol_slot_number: Slot,
        sol_tx_idx: usize,
        sol_signature: &SolSignature,
        tx_request: TypedTransaction,
        eth_signature: EthSignature,
        sol_meta: &UiTransactionStatusMeta,
    ) -> ProgramResult<Option<TransactionResult>> {
        let evm_tx_hash = tx_request.hash(&eth_signature);
        self.update_or_create_transaction(
            evm_tx_hash,
            || TransactionData::new_small_tx(&tx_request, eth_signature),
            |tx| {
                tx.update_execute_tx(
                    evm_tx_hash,
                    sol_slot_number,
                    sol_tx_idx,
                    *sol_signature,
                    sol_meta,
                )
            },
        )
    }

    pub fn register_big_tx(
        &mut self,
        evm_tx_hash: TxHash,
        sol_slot_number: Slot,
        sol_tx_idx: usize,
        sol_signature: &SolSignature,
        sol_meta: &UiTransactionStatusMeta,
    ) -> ProgramResult<Option<TransactionResult>> {
        self.update_or_create_transaction(
            evm_tx_hash,
            || Ok(TransactionData::new_big_tx()),
            |tx| {
                tx.update_execute_tx(
                    evm_tx_hash,
                    sol_slot_number,
                    sol_tx_idx,
                    *sol_signature,
                    sol_meta,
                )
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn register_transmit_tx(
        &mut self,
        evm_tx_hash: TxHash,
        sol_slot_number: Slot,
        holder_offset: usize,
        holder_data: &[u8],
        sol_tx_idx: usize,
        sol_signature: &SolSignature,
        meta: &UiTransactionStatusMeta,
    ) -> ProgramResult<Option<TransactionResult>> {
        if meta.err.is_some() {
            return Ok(None);
        }

        self.update_or_create_transaction(
            evm_tx_hash,
            || Ok(TransactionData::new_big_tx()),
            |tx| {
                tx.update_transmit_tx(
                    sol_slot_number,
                    sol_tx_idx,
                    sol_signature,
                    holder_offset,
                    holder_data,
                )
            },
        )
    }

    pub fn try_finalize_transaction(
        &mut self,
        tx_hash: &TxHash,
        log_index: U256,
        transaction_index: usize,
        block_hash: H256,
        block_number: U64,
        block_gas_used: U256,
    ) -> ProgramResult<Option<&TransactionReceipt>> {
        if let Some(tx) = self.transactions.get_mut(tx_hash) {
            tx.try_finalize(
                log_index,
                U64::from(transaction_index),
                block_hash,
                block_number,
                block_gas_used,
            )?;
            Ok(tx.get_receipt())
        } else {
            Ok(None)
        }
    }
}
