use crate::error::ProgramResult;
use crate::indexer::indexed_transaction::IndexedTransaction;
use crate::indexer::transaction_data::TransactionData;
use crate::indexer::transaction_storage::{BlockBuilder, TransactionStorage};
use crate::indexer::tx_parser::GasReport;
use async_trait::async_trait;
use ethers::prelude::transaction::eip2718::TypedTransaction;
use ethers::prelude::{
    Signature as EthSignature, Transaction, TransactionReceipt, H256 as TxHash, H256, U256, U64,
};
use solana_program::clock::Slot;
use solana_sdk::signature::Signature as SolSignature;
use solana_transaction_status::UiTransactionStatusMeta;
use std::collections::{btree_map, BTreeMap, LinkedList};
use std::sync::Arc;
use tokio::sync::{RwLock, RwLockWriteGuard};

#[derive(Default, Clone)]
pub struct TransactionInMemoryStorage {
    transactions: Arc<RwLock<BTreeMap<TxHash, TransactionData>>>,
}

impl TransactionInMemoryStorage {
    pub fn new() -> Self {
        Self::default()
    }
}

enum PostponedTx {
    SmallTx {
        sol_slot_number: Slot,
        sol_tx_idx: usize,
        sol_signature: SolSignature,
        tx_request: TypedTransaction,
        eth_signature: EthSignature,
        sol_meta: UiTransactionStatusMeta,
    },
    BigTx {
        evm_tx_hash: TxHash,
        sol_slot_number: Slot,
        sol_tx_idx: usize,
        sol_signature: SolSignature,
        sol_meta: UiTransactionStatusMeta,
    },
    TransmitTx {
        evm_tx_hash: TxHash,
        sol_slot_number: Slot,
        holder_offset: usize,
        holder_data: Vec<u8>,
        sol_tx_idx: usize,
        sol_signature: SolSignature,
        meta: UiTransactionStatusMeta,
    },
}

pub struct InMemoryBlockBuilder<'a> {
    lock: RwLockWriteGuard<'a, BTreeMap<TxHash, TransactionData>>,
    txs: LinkedList<PostponedTx>,
}

fn update_or_create_transaction<T>(
    lock: &mut RwLockWriteGuard<BTreeMap<TxHash, TransactionData>>,
    evm_tx_hash: TxHash,
    constructor: impl FnOnce() -> ProgramResult<TransactionData>,
    updater: impl FnOnce(&mut TransactionData) -> ProgramResult<T>,
) -> ProgramResult<T> {
    match lock.entry(evm_tx_hash) {
        btree_map::Entry::Occupied(mut entry) => updater(entry.get_mut()),
        btree_map::Entry::Vacant(entry) => updater(entry.insert(constructor()?)),
    }
}

#[async_trait]
impl BlockBuilder for InMemoryBlockBuilder<'_> {
    fn register_small_tx(
        &mut self,
        sol_slot_number: Slot,
        sol_tx_idx: usize,
        sol_signature: SolSignature,
        tx_request: TypedTransaction,
        eth_signature: EthSignature,
        sol_meta: UiTransactionStatusMeta,
    ) {
        self.txs.push_back(PostponedTx::SmallTx {
            sol_slot_number,
            sol_tx_idx,
            sol_signature,
            tx_request,
            eth_signature,
            sol_meta,
        })
    }

    fn register_big_tx(
        &mut self,
        evm_tx_hash: TxHash,
        sol_slot_number: Slot,
        sol_tx_idx: usize,
        sol_signature: SolSignature,
        sol_meta: UiTransactionStatusMeta,
    ) {
        self.txs.push_back(PostponedTx::BigTx {
            evm_tx_hash,
            sol_slot_number,
            sol_tx_idx,
            sol_signature,
            sol_meta,
        })
    }

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
    ) {
        self.txs.push_back(PostponedTx::TransmitTx {
            evm_tx_hash,
            sol_slot_number,
            holder_offset,
            holder_data,
            sol_tx_idx,
            sol_signature,
            meta,
        })
    }

    async fn end(mut self, slot_number: Slot) -> ProgramResult<LinkedList<TxHash>> {
        let mut result = LinkedList::new();
        for op in self.txs {
            if let Some(Some(tx_result)) = match op {
                PostponedTx::SmallTx {
                    sol_slot_number,
                    sol_tx_idx,
                    sol_signature,
                    tx_request,
                    eth_signature,
                    sol_meta,
                } => {
                    let evm_tx_hash = tx_request.hash(&eth_signature);
                    Some(update_or_create_transaction(
                        &mut self.lock,
                        evm_tx_hash,
                        || TransactionData::new_small_tx(&tx_request, eth_signature),
                        |tx| {
                            tx.update_execute_tx(
                                evm_tx_hash,
                                sol_slot_number,
                                sol_tx_idx,
                                sol_signature,
                                &sol_meta,
                            )
                        },
                    )?)
                }
                PostponedTx::BigTx {
                    evm_tx_hash,
                    sol_slot_number,
                    sol_tx_idx,
                    sol_signature,
                    sol_meta,
                } => Some(update_or_create_transaction(
                    &mut self.lock,
                    evm_tx_hash,
                    || Ok(TransactionData::new_big_tx()),
                    |tx| {
                        tx.update_execute_tx(
                            evm_tx_hash,
                            sol_slot_number,
                            sol_tx_idx,
                            sol_signature,
                            &sol_meta,
                        )
                    },
                )?),
                PostponedTx::TransmitTx {
                    evm_tx_hash,
                    sol_slot_number,
                    holder_offset,
                    holder_data,
                    sol_tx_idx,
                    sol_signature,
                    meta,
                } => {
                    if meta.err.is_none() {
                        Some(update_or_create_transaction(
                            &mut self.lock,
                            evm_tx_hash,
                            || Ok(TransactionData::new_big_tx()),
                            |tx| {
                                tx.update_transmit_tx(
                                    sol_slot_number,
                                    sol_tx_idx,
                                    &sol_signature,
                                    holder_offset,
                                    holder_data.as_slice(),
                                )
                            },
                        )?)
                    } else {
                        None
                    }
                }
            } {
                if let Some(included_in_slot) = tx_result.included_in_slot {
                    if included_in_slot == slot_number {
                        result.push_back(tx_result.tx_hash);
                    }
                }
            }
        }

        Ok(result)
    }
}

#[async_trait]
impl TransactionStorage for TransactionInMemoryStorage {
    type TransactionType = TransactionData;
    type BlockType<'a> = InMemoryBlockBuilder<'a>;

    async fn get_transaction(&self, tx_hash: &TxHash) -> ProgramResult<Option<TransactionData>> {
        let lock = self.transactions.read().await;
        Ok(lock.get(tx_hash).cloned())
    }

    async fn get_full_txs(&self, tx_hashes: Arc<Vec<TxHash>>) -> ProgramResult<Vec<Transaction>> {
        let lock = self.transactions.read().await;
        Ok(tx_hashes
            .iter()
            .filter_map(|tx_hash| lock.get(tx_hash).and_then(|tx| tx.eth_transaction()))
            .collect())
    }

    async fn get_txs(&self, tx_hashes: Arc<Vec<TxHash>>) -> ProgramResult<Vec<TxHash>> {
        let lock = self.transactions.read().await;
        Ok(tx_hashes
            .iter()
            .filter_map(|tx_hash| {
                lock.get(tx_hash)
                    .and_then(|tx| tx.eth_transaction().map(|_| *tx_hash))
            })
            .collect())
    }

    async fn get_full_txs_with_gas_report(
        &self,
        tx_hashes: Arc<Vec<TxHash>>,
    ) -> ProgramResult<Vec<(Transaction, GasReport)>> {
        let lock = self.transactions.read().await;
        Ok(tx_hashes
            .iter()
            .filter_map(|tx_hash| {
                lock.get(tx_hash).and_then(|tx| {
                    if let (Some(eth_tx), Some(gas_report)) =
                        (tx.eth_transaction(), tx.gas_report())
                    {
                        Some((eth_tx, gas_report))
                    } else {
                        None
                    }
                })
            })
            .collect())
    }

    async fn start_block<'a, 'b: 'a>(&'b self) -> ProgramResult<Self::BlockType<'a>> {
        Ok(InMemoryBlockBuilder {
            lock: self.transactions.write().await,
            txs: LinkedList::new(),
        })
    }

    async fn should_continue_parsing(&self, tx_hashes: &LinkedList<TxHash>) -> ProgramResult<bool> {
        if tx_hashes.is_empty() {
            return Ok(false);
        }

        let lock = self.transactions.read().await;
        for tx_hash in tx_hashes {
            if let Some(tx_data) = lock.get(tx_hash) {
                return Ok(!tx_data.is_complete());
            }
        }

        Ok(true)
    }

    async fn try_finalize_transaction(
        &self,
        tx_hash: &TxHash,
        log_index: U256,
        transaction_index: usize,
        block_hash: H256,
        block_number: U64,
        block_gas_used: U256,
    ) -> ProgramResult<Option<TransactionReceipt>> {
        let mut lock = self.transactions.write().await;
        if let Some(tx) = lock.get_mut(tx_hash) {
            tx.try_finalize(
                log_index,
                U64::from(transaction_index),
                block_hash,
                block_number,
                block_gas_used,
            )?;
            Ok(tx.eth_receipt())
        } else {
            Ok(None)
        }
    }
}
