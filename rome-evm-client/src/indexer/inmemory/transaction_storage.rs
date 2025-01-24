use crate::error::ProgramResult;
use crate::error::RomeEvmError::Custom;
use crate::indexer::{
    self, ethereum_block_storage::ReceiptParams, parsers::block_parser::TxResult, BlockParams,
    BlockParseResult, PendingBlocks, ProducedBlocks,
};
use ethers::types::{Bloom, Log, OtherFields, Transaction, TransactionReceipt, TxHash, U256, U64};
use solana_program::clock::Slot;
use std::collections::{BTreeMap, HashMap};
use std::ops::Add;
use std::sync::Arc;
use tokio::sync::RwLock;

struct IndexedResult {
    pub tx_result: TxResult,
    pub receipt_params: Option<ReceiptParams>,
}

struct InMemoryTransaction {
    pub tx: Transaction,
    pub results: BTreeMap<Slot, IndexedResult>,
}

#[derive(Default)]
struct TxContainer {
    transactions: HashMap<TxHash, InMemoryTransaction>,
}

impl TxContainer {
    pub fn register_parse_result(&mut self, parse_result: BlockParseResult) -> ProgramResult<()> {
        for (tx, tx_result) in parse_result.transactions {
            let tx_hash = tx.hash;
            self.transactions
                .entry(tx_hash)
                .or_insert(InMemoryTransaction {
                    tx,
                    results: BTreeMap::new(),
                })
                .results
                .insert(
                    parse_result.slot_number,
                    IndexedResult {
                        tx_result: tx_result.clone(),
                        receipt_params: None,
                    },
                );
        }

        Ok(())
    }

    pub fn get_transactions(
        &self,
        slot_number: Slot,
        hashes: &[TxHash],
    ) -> ProgramResult<Vec<Transaction>> {
        let mut result = Vec::new();
        for tx_hash in hashes {
            if let Some(tx) = self.get_transaction(tx_hash, Some(slot_number)) {
                result.push(tx);
            } else {
                return Err(Custom(format!(
                    "get_transactions: Transaction {:?} not found ",
                    tx_hash
                )));
            }
        }

        Ok(result)
    }

    pub fn get_transactions_with_results(
        &self,
        hashes: &[TxHash],
        slot_number: Slot,
    ) -> ProgramResult<BTreeMap<usize, (Transaction, TxResult)>> {
        let mut result = BTreeMap::new();
        let mut tx_idx: usize = 0;
        for tx_hash in hashes {
            if let Some(inmemory_tx) = self.transactions.get(tx_hash) {
                if let Some(indexed_tx) = inmemory_tx.results.get(&slot_number) {
                    result.insert(
                        tx_idx,
                        (inmemory_tx.tx.clone(), indexed_tx.tx_result.clone()),
                    );
                    tx_idx += 1;
                }
            } else {
                return Err(Custom(format!(
                    "get_transactions_with_gas_reports: Transaction {:?} not found ",
                    tx_hash
                )));
            }
        }

        Ok(result)
    }

    pub fn block_produced(
        &mut self,
        slot_number: Slot,
        block_params: &BlockParams,
        tx_hashes: &[TxHash],
    ) -> ProgramResult<()> {
        let mut block_gas_used = U256::zero();
        let mut log_index = U256::zero();
        for (tx_index, tx_hash) in tx_hashes.iter().enumerate() {
            if let Some(inmemory_tx) = self.transactions.get_mut(tx_hash) {
                if let Some(indexed_result) = inmemory_tx.results.get_mut(&slot_number) {
                    indexed_result.receipt_params = Some(ReceiptParams {
                        blockhash: block_params.hash,
                        block_number: block_params.number,
                        tx_index,
                        block_gas_used,
                        first_log_index: log_index,
                    });
                    block_gas_used += indexed_result.tx_result.gas_report.gas_value;
                    log_index += U256::from(indexed_result.tx_result.logs.len());
                } else {
                    return Err(Custom(format!(
                        "Transaction {:?} result for slot {:?} not found",
                        tx_hash, slot_number
                    )));
                }
            }
        }

        Ok(())
    }

    fn get_indexed_result(
        &self,
        tx_hash: &TxHash,
        slot_number: Option<Slot>,
    ) -> Option<&IndexedResult> {
        if let Some(inmemory_tx) = self.transactions.get(tx_hash) {
            if let Some(slot_number) = slot_number {
                inmemory_tx.results.get(&slot_number)
            } else {
                inmemory_tx.results.last_key_value().map(|(_, r)| r)
            }
        } else {
            None
        }
    }

    pub fn get_transaction_receipt(
        &self,
        tx_hash: &TxHash,
        slot_number: Option<Slot>,
    ) -> Option<TransactionReceipt> {
        if let Some(tx) = self.transactions.get(tx_hash) {
            if let Some(indexed_result) = self.get_indexed_result(tx_hash, slot_number) {
                if let Some(receipt_params) = &indexed_result.receipt_params {
                    let mut transaction_log_index = receipt_params.first_log_index;
                    let transaction_index = U64::from(receipt_params.tx_index);

                    Some(TransactionReceipt {
                        transaction_hash: tx.tx.hash,
                        transaction_index,
                        transaction_type: tx.tx.transaction_type,
                        block_hash: Some(receipt_params.blockhash),
                        block_number: Some(receipt_params.block_number),
                        from: tx.tx.from,
                        to: tx.tx.to,
                        gas_used: Some(indexed_result.tx_result.gas_report.gas_value),
                        cumulative_gas_used: receipt_params
                            .block_gas_used
                            .checked_add(indexed_result.tx_result.gas_report.gas_value)
                            .expect("This must never happen - block gas overflow!"),
                        contract_address: indexer::calc_contract_address(
                            &tx.tx.from,
                            &tx.tx.to,
                            &tx.tx.nonce,
                        ),
                        logs: indexed_result
                            .tx_result
                            .logs
                            .iter()
                            .enumerate()
                            .map(|(log_index, log)| {
                                let result = Log {
                                    block_hash: Some(receipt_params.blockhash),
                                    block_number: Some(receipt_params.block_number),
                                    transaction_hash: Some(tx.tx.hash),
                                    transaction_index: Some(transaction_index),
                                    log_index: Some(
                                        U256::from(log_index)
                                            .checked_add(transaction_log_index)
                                            .expect("Unable to increment log index"),
                                    ),
                                    transaction_log_index: Some(transaction_log_index),
                                    removed: Some(false),
                                    ..log.clone()
                                };
                                transaction_log_index = transaction_log_index.add(U256::one());
                                result
                            })
                            .collect::<Vec<Log>>(),
                        status: Some(U64::from(
                            (indexed_result.tx_result.exit_reason.code == 0) as u64,
                        )),
                        logs_bloom: Bloom::default(),
                        root: None,
                        effective_gas_price: if let Some(price) = tx.tx.gas_price {
                            Some(price)
                        } else if let Some(price) = tx.tx.max_priority_fee_per_gas {
                            Some(price)
                        } else {
                            panic!("No gas_price nor max_priority_fee_per_gas defined")
                        },
                        other: OtherFields::default(),
                    })
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    }

    pub fn get_transaction(
        &self,
        tx_hash: &TxHash,
        slot_number: Option<Slot>,
    ) -> Option<Transaction> {
        if let Some(tx) = self.transactions.get(tx_hash) {
            let mut tx = tx.tx.clone();
            if let Some(indexed_result) = self.get_indexed_result(tx_hash, slot_number) {
                if let Some(receipt_params) = &indexed_result.receipt_params {
                    tx.block_hash = Some(receipt_params.blockhash);
                    tx.block_number = Some(receipt_params.block_number);
                    tx.transaction_index = Some(U64::from(receipt_params.tx_index));
                }
            }

            Some(tx)
        } else {
            None
        }
    }

    fn remove_transactions(&mut self, transactions: Vec<TxHash>) -> ProgramResult<()> {
        for tx in &transactions {
            self.transactions.remove(tx);
        }

        Ok(())
    }
}

#[derive(Default, Clone)]
pub struct TransactionStorage {
    transactions: Arc<RwLock<TxContainer>>,
}

impl TransactionStorage {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn register_parse_result(&self, parse_result: BlockParseResult) -> ProgramResult<()> {
        self.transactions
            .write()
            .await
            .register_parse_result(parse_result)
    }

    pub async fn get_transaction_receipt(
        &self,
        tx_hash: &TxHash,
    ) -> ProgramResult<Option<TransactionReceipt>> {
        Ok(self
            .transactions
            .read()
            .await
            .get_transaction_receipt(tx_hash, None))
    }

    pub async fn get_transaction(&self, tx_hash: &TxHash) -> ProgramResult<Option<Transaction>> {
        Ok(self
            .transactions
            .read()
            .await
            .get_transaction(tx_hash, None))
    }

    pub async fn get_transactions(
        &self,
        slot_number: Slot,
        hashes: &[TxHash],
    ) -> ProgramResult<Vec<Transaction>> {
        self.transactions
            .read()
            .await
            .get_transactions(slot_number, hashes)
    }

    pub async fn get_transactions_with_results(
        &self,
        hashes: &[TxHash],
        slot_number: Slot,
    ) -> ProgramResult<BTreeMap<usize, (Transaction, TxResult)>> {
        self.transactions
            .read()
            .await
            .get_transactions_with_results(hashes, slot_number)
    }

    pub async fn blocks_produced(
        &self,
        pending_blocks: PendingBlocks,
        produced_blocks: ProducedBlocks,
    ) -> ProgramResult<()> {
        let mut lock = self.transactions.write().await;
        for (block_id, pending_block) in pending_blocks {
            if let Some(block_params) = produced_blocks.get(&block_id) {
                lock.block_produced(
                    block_id.0,
                    block_params,
                    &pending_block
                        .transactions
                        .values()
                        .map(|(tx, _)| tx.hash)
                        .collect::<Vec<_>>(),
                )?
            }
        }

        Ok(())
    }

    pub async fn remove_transactions(&self, transactions: Vec<TxHash>) -> ProgramResult<()> {
        self.transactions
            .write()
            .await
            .remove_transactions(transactions)
    }
}

#[cfg(test)]
mod test {
    use crate::indexer::ethereum_block_storage::EthBlockId;
    use crate::indexer::parsers::block_parser::TxResult;
    use crate::indexer::{
        inmemory::TransactionStorage,
        test::{create_simple_tx, create_wallet},
        BlockParams, BlockParseResult, PendingBlock,
    };
    use ethers::prelude::H256;
    use ethers::types::{Address, Transaction, U256, U64};
    use rand::random;
    use solana_program::clock::Slot;
    use std::collections::BTreeMap;
    use tokio;

    const CHAIN_ID: u64 = 100;

    async fn reg_block_and_check_receipt(
        storage: TransactionStorage,
        slot_number: Slot,
        tx: Transaction,
        tx_result: TxResult,
        gas_recipient: Option<Address>,
    ) {
        let tx_obj = storage.get_transaction(&tx.hash).await.unwrap();
        assert!(tx_obj.is_some());
        let tx_obj = tx_obj.unwrap();
        assert!(tx_obj.block_number.is_none());
        assert!(tx_obj.block_hash.is_none());

        let block_id: EthBlockId = (slot_number, 0);
        let blockhash = H256::random();
        let block_number = U64::from(random::<u64>());
        storage
            .blocks_produced(
                BTreeMap::from([(
                    block_id.clone(),
                    PendingBlock {
                        transactions: BTreeMap::from([(0, (tx.clone(), tx_result))]),
                        gas_recipient,
                        slot_timestamp: Some(0),
                    },
                )]),
                BTreeMap::from([(
                    block_id.clone(),
                    BlockParams {
                        hash: blockhash,
                        parent_hash: Some(H256::random()),
                        number: block_number,
                        timestamp: U256::zero(),
                    },
                )]),
            )
            .await
            .unwrap();

        let tx_obj = storage.get_transaction(&tx.hash).await.unwrap();
        assert!(tx_obj.is_some());
        let tx_obj = tx_obj.unwrap();
        assert_eq!(tx_obj.block_number, Some(block_number));
        assert_eq!(tx_obj.block_hash, Some(blockhash));

        let receipt = storage
            .get_transaction_receipt(&tx.hash)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(receipt.block_hash, Some(blockhash));
        assert_eq!(receipt.block_number, Some(block_number));
    }

    #[tokio::test]
    async fn test_register_parse_result() {
        let wallet = create_wallet();
        let gas_recipient = Address::random();
        let storage = TransactionStorage::new();
        let slot_number: Slot = 13;

        let (tx, tx_result) =
            create_simple_tx(Some(U64::from(CHAIN_ID)), &wallet, Some(gas_recipient)).await;

        storage
            .register_parse_result(BlockParseResult {
                slot_number,
                parent_slot_number: slot_number - 1,
                timestamp: Some(0i64),
                transactions: vec![(tx.clone(), tx_result.clone())],
            })
            .await
            .unwrap();

        reg_block_and_check_receipt(storage, slot_number, tx, tx_result, Some(gas_recipient)).await;
    }

    #[tokio::test]
    async fn test_failed_to_register_block_in_different_slot() {
        let wallet = create_wallet();
        let gas_recipient = Address::random();
        let storage = TransactionStorage::new();
        let slot_number: Slot = 13;

        let (tx, tx_result) =
            create_simple_tx(Some(U64::from(CHAIN_ID)), &wallet, Some(gas_recipient)).await;

        storage
            .register_parse_result(BlockParseResult {
                slot_number,
                parent_slot_number: slot_number - 1,
                timestamp: Some(0i64),
                transactions: vec![(tx.clone(), tx_result.clone())],
            })
            .await
            .unwrap();

        let block_id: EthBlockId = (slot_number + 1, 0);
        let blockhash = H256::random();
        let block_number = U64::from(random::<u64>());
        assert!(storage
            .blocks_produced(
                BTreeMap::from([(
                    block_id.clone(),
                    PendingBlock {
                        transactions: BTreeMap::from([(0, (tx.clone(), tx_result))]),
                        gas_recipient: Some(gas_recipient),
                        slot_timestamp: Some(0),
                    },
                )]),
                BTreeMap::from([(
                    block_id.clone(),
                    BlockParams {
                        hash: blockhash,
                        parent_hash: Some(H256::random()),
                        number: block_number,
                        timestamp: U256::zero(),
                    },
                )]),
            )
            .await
            .is_err());
    }
}
