use crate::error::RomeEvmError::Custom;
use crate::indexer::ethereum_block_storage::{EthBlockId, PendingBlock, ReproduceBlocks};
use crate::indexer::parsers::block_parser::EthBlock;
use crate::indexer::{inmemory::TransactionStorage, BlockParams, PendingBlocks, ProducedBlocks};
use async_trait::async_trait;
use ethers::prelude::{Block, Transaction, TransactionReceipt, TxHash};
use solana_program::clock::Slot;
use std::collections::btree_map;
use {
    crate::{
        error::ProgramResult,
        indexer::{self, ethereum_block_storage::BlockType, BlockParseResult},
    },
    ethers::types::{H256, U64},
    std::{
        collections::{BTreeMap, HashMap},
        sync::Arc,
    },
    tokio::sync::{RwLock, RwLockWriteGuard},
};

impl EthBlock {
    pub async fn get_block(
        &self,
        full_transactions: bool,
        transaction_storage: &Arc<TransactionStorage>,
    ) -> ProgramResult<BlockType> {
        Ok(if full_transactions {
            BlockType::BlockWithTransactions(Block::<Transaction> {
                transactions: transaction_storage
                    .get_transactions(self.id.0, &self.transactions)
                    .await?,
                ..self.get_block_base()
            })
        } else {
            BlockType::BlockWithHashes(Block::<TxHash> {
                transactions: self.transactions.clone(),
                ..self.get_block_base()
            })
        })
    }

    pub async fn get_pending_block(
        &self,
        transaction_storage: &Arc<TransactionStorage>,
    ) -> ProgramResult<PendingBlock> {
        Ok(PendingBlock {
            transactions: transaction_storage
                .get_transactions_with_results(&self.transactions, self.id.0)
                .await?,
            gas_recipient: self.gas_recipient,
            slot_timestamp: self.slot_timestamp,
        })
    }
}

pub struct SolanaBlock {
    pub slot_number: Slot,
    pub parent_slot_number: Slot,
    pub eth_blocks: Vec<EthBlock>,
}

impl SolanaBlock {
    pub fn get_last_blockhash(&self) -> Option<H256> {
        self.eth_blocks
            .iter()
            .rfind(|eth_block| eth_block.block_params.is_some())
            .and_then(|eth_block| eth_block.block_params.as_ref().map(|p| p.hash))
    }

    pub async fn get_pending_blocks(
        &self,
        blocks: &mut PendingBlocks,
        transaction_storage: &Arc<TransactionStorage>,
    ) -> ProgramResult<Option<H256>> {
        for eth_block in &self.eth_blocks {
            if let Some(block_params) = &eth_block.block_params {
                return Ok(Some(block_params.hash));
            } else {
                blocks.insert(
                    eth_block.id,
                    eth_block.get_pending_block(transaction_storage).await?,
                );
            }
        }

        Ok(None)
    }
}

async fn update_block<'a>(
    by_sol_slot_lock: &mut RwLockWriteGuard<'a, BTreeMap<Slot, SolanaBlock>>,
    by_number_lock: &mut RwLockWriteGuard<'a, BTreeMap<U64, EthBlockId>>,
    by_hash_lock: &mut RwLockWriteGuard<'a, HashMap<H256, EthBlockId>>,
    block_id: &EthBlockId,
    block_params: &BlockParams,
) -> ProgramResult<()> {
    let (slot_number, eth_block_idx) = block_id;
    if let Some(solana_block) = by_sol_slot_lock.get_mut(slot_number) {
        if let Some(eth_block) = solana_block.eth_blocks.get_mut(*eth_block_idx) {
            eth_block.block_params = Some(block_params.clone());
            by_number_lock.insert(block_params.number, *block_id);
            by_hash_lock.insert(block_params.hash, *block_id);
            Ok(())
        } else {
            Err(Custom(format!(
                "add_reg_result: EthBlock {:?} not found",
                block_id
            )))
        }
    } else {
        Err(Custom(format!(
            "add_reg_result: solana block {:?} not found",
            block_id
        )))
    }
}

#[derive(Clone)]
pub struct EthereumBlockStorage {
    blocks_by_sol_slot: Arc<RwLock<BTreeMap<Slot, SolanaBlock>>>,

    blocks_by_number: Arc<RwLock<BTreeMap<U64, EthBlockId>>>,

    // Blockhashes only produced after state-advance on op-geth
    blocks_by_hash: Arc<RwLock<HashMap<H256, EthBlockId>>>,

    transaction_storage: Arc<TransactionStorage>,

    chain_id: u64,
}

impl EthereumBlockStorage {
    pub fn new(chain_id: u64) -> Self {
        Self {
            blocks_by_sol_slot: Arc::new(RwLock::new(BTreeMap::new())),
            blocks_by_number: Arc::new(RwLock::new(BTreeMap::new())),
            blocks_by_hash: Arc::new(RwLock::new(HashMap::new())),
            transaction_storage: Arc::new(TransactionStorage::new()),
            chain_id,
        }
    }

    async fn get_block_by_id(&self, block_id: &EthBlockId) -> Option<EthBlock> {
        let lock = self.blocks_by_sol_slot.read().await;
        let (slot_number, eth_block_idx) = block_id;
        lock.get(slot_number)
            .and_then(|block| block.eth_blocks.get(*eth_block_idx))
            .cloned()
    }
}

#[async_trait]
impl indexer::EthereumBlockStorage for EthereumBlockStorage {
    async fn get_pending_blocks(&self) -> ProgramResult<Option<(Option<H256>, PendingBlocks)>> {
        let lock = self.blocks_by_sol_slot.read().await;

        // Find a not-advanced chain of blocks which is higher than advanced one
        if let (max_advanced, Some(max_not_advanced)) = (
            lock.iter()
                .rfind(|(_, block)| block.get_last_blockhash().is_some())
                .map(|(s, _)| *s),
            lock.iter()
                .rfind(|(_, block)| block.get_last_blockhash().is_none())
                .map(|(s, _)| *s),
        ) {
            if let Some(max_advanced) = max_advanced {
                if max_not_advanced < max_advanced {
                    return Ok(None);
                }
            }

            // Build chain of solana blocks to advance state
            let mut current_slot = max_not_advanced;
            let mut pending_blocks = PendingBlocks::default();
            let parent_blockhash = loop {
                if let Some(solana_block) = lock.get(&current_slot) {
                    if let Some(parent_blockhash) = solana_block
                        .get_pending_blocks(&mut pending_blocks, &self.transaction_storage)
                        .await?
                    {
                        break Some(parent_blockhash);
                    } else if current_slot != 0 {
                        current_slot = solana_block.parent_slot_number;
                    } else {
                        break None;
                    }
                } else {
                    break None;
                }
            };

            if !pending_blocks.is_empty() {
                Ok(Some((parent_blockhash, pending_blocks)))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }

    async fn reproduce_blocks(
        &self,
        _from_slot: Slot,
        _to_slot: Slot,
    ) -> ProgramResult<Option<(Option<TxHash>, ReproduceBlocks)>> {
        Ok(None)
    }

    async fn register_parse_results(
        &self,
        parse_results: BTreeMap<Slot, BlockParseResult>,
    ) -> ProgramResult<Option<(Option<H256>, PendingBlocks)>> {
        if !parse_results.is_empty() {
            let mut lock = self.blocks_by_sol_slot.write().await;
            for (slot_number, parse_result) in parse_results {
                match lock.entry(slot_number) {
                    btree_map::Entry::Occupied(_) => {
                        tracing::info!("Slot {:?} already has been registered", slot_number)
                    }
                    btree_map::Entry::Vacant(entry) => {
                        entry.insert(SolanaBlock {
                            slot_number,
                            parent_slot_number: parse_result.parent_slot_number,
                            eth_blocks: parse_result.create_eth_blocks(),
                        });

                        self.transaction_storage
                            .register_parse_result(parse_result)
                            .await?;
                    }
                }
            }
        }

        self.get_pending_blocks().await
    }

    async fn latest_block(&self) -> ProgramResult<Option<U64>> {
        let lock = self.blocks_by_number.read().await;
        Ok(if let Some((_, block_id)) = lock.last_key_value() {
            self.get_block_by_id(block_id)
                .await
                .and_then(|b| b.block_params.as_ref().map(|p| p.number))
        } else {
            None
        })
    }

    async fn get_block_number(&self, hash: &H256) -> ProgramResult<Option<U64>> {
        let lock = self.blocks_by_hash.read().await;
        Ok(if let Some(block_id) = lock.get(hash) {
            self.get_block_by_id(block_id)
                .await
                .and_then(|b| b.block_params.as_ref().map(|p| p.number))
        } else {
            None
        })
    }

    async fn get_block_by_number(
        &self,
        number: U64,
        full_transactions: bool,
    ) -> ProgramResult<Option<BlockType>> {
        let lock = self.blocks_by_number.read().await;
        Ok(if let Some(block_id) = lock.get(&number) {
            if let Some(block) = self.get_block_by_id(block_id).await {
                Some(
                    block
                        .get_block(full_transactions, &self.transaction_storage)
                        .await?,
                )
            } else {
                None
            }
        } else {
            None
        })
    }

    async fn blocks_produced(
        &self,
        pending_blocks: PendingBlocks,
        produced_blocks: ProducedBlocks,
    ) -> ProgramResult<()> {
        let mut by_sol_slot_lock = self.blocks_by_sol_slot.write().await;
        let mut by_number_lock = self.blocks_by_number.write().await;
        let mut by_hash_lock = self.blocks_by_hash.write().await;

        for block_id in pending_blocks.keys() {
            if let Some(block_params) = produced_blocks.get(block_id) {
                update_block(
                    &mut by_sol_slot_lock,
                    &mut by_number_lock,
                    &mut by_hash_lock,
                    block_id,
                    block_params,
                )
                .await?;
                tracing::info!("new block registered {:?}:{:?}", block_id, block_params);
            }
        }

        self.transaction_storage
            .blocks_produced(pending_blocks, produced_blocks)
            .await?;

        Ok(())
    }

    fn chain(&self) -> u64 {
        self.chain_id
    }

    async fn get_max_slot_produced(&self) -> ProgramResult<Option<Slot>> {
        Ok(self
            .blocks_by_sol_slot
            .read()
            .await
            .last_key_value()
            .map(|(slot, _)| *slot))
    }

    async fn retain_from_slot(&self, from_slot: Slot) -> ProgramResult<()> {
        let mut by_slot_lock = self.blocks_by_sol_slot.write().await;
        let mut by_number_lock = self.blocks_by_number.write().await;
        let mut by_hash_lock = self.blocks_by_hash.write().await;

        let mut transactions_to_remove = Vec::new();
        by_slot_lock.retain(|slot, solana_block| {
            if *slot < from_slot {
                for eth_block in &solana_block.eth_blocks {
                    if let Some(block_params) = &eth_block.block_params {
                        by_number_lock.remove(&block_params.number);
                        by_hash_lock.remove(&block_params.hash);
                    }

                    for tx in &eth_block.transactions {
                        transactions_to_remove.push(*tx);
                    }
                }

                false
            } else {
                true
            }
        });

        self.transaction_storage
            .remove_transactions(transactions_to_remove)
            .await
    }

    async fn get_transaction_receipt(
        &self,
        tx_hash: &TxHash,
    ) -> ProgramResult<Option<TransactionReceipt>> {
        self.transaction_storage
            .get_transaction_receipt(tx_hash)
            .await
    }

    async fn get_transaction(&self, tx_hash: &TxHash) -> ProgramResult<Option<Transaction>> {
        self.transaction_storage.get_transaction(tx_hash).await
    }

    async fn get_slot_for_eth_block(&self, block_number: U64) -> ProgramResult<Option<Slot>> {
        let lock = self.blocks_by_number.read().await;
        Ok(lock.get(&block_number).map(|block_id| block_id.0))
    }
}

#[cfg(test)]
mod test {
    use crate::indexer::parsers::block_parser::TxResult;
    use crate::indexer::test::{create_simple_tx, create_wallet};
    use crate::indexer::{
        inmemory, BlockParams, BlockParseResult, BlockType, EthereumBlockStorage, PendingBlocks,
    };
    use ethers::core::k256::ecdsa::SigningKey;
    use ethers::signers::Wallet;
    use ethers::types::{Address, Transaction, H256, U256, U64};
    use rand::random;
    use solana_program::clock::UnixTimestamp;
    use solana_sdk::clock::Slot;
    use std::collections::{BTreeMap, HashSet};

    const CHAIN_ID1: u64 = 100;
    const CHAIN_ID2: u64 = 101;

    struct BlockParseResultBuilder {
        slot_number: Slot,
        parent_slot_number: Slot,
        timestamp: Option<UnixTimestamp>,
        transactions: Vec<(Transaction, TxResult)>,
    }

    impl BlockParseResultBuilder {
        fn new_with_timestamp(
            slot_number: Slot,
            timestamp: UnixTimestamp,
            parent_slot_number: Slot,
        ) -> Self {
            Self {
                slot_number,
                parent_slot_number,
                timestamp: Some(timestamp),
                transactions: Vec::new(),
            }
        }

        fn new(slot_number: Slot, parent_slot_number: Slot) -> Self {
            let time = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap();

            Self::new_with_timestamp(slot_number, time.as_millis() as i64, parent_slot_number)
        }

        pub fn slot(&self) -> Slot {
            self.slot_number
        }

        pub async fn add_transaction(
            mut self,
            chain_id: u64,
            wallet: &Wallet<SigningKey>,
            gas_recipient: Option<Address>,
        ) -> Self {
            let (tx, tx_result) =
                create_simple_tx(Some(U64::from(chain_id)), wallet, gas_recipient).await;
            self.transactions.push((tx, tx_result));
            self
        }

        pub fn build(&self) -> BlockParseResult {
            BlockParseResult {
                slot_number: self.slot_number,
                parent_slot_number: self.parent_slot_number,
                timestamp: self.timestamp,
                transactions: self.transactions.clone(),
            }
        }

        pub fn check_pending_blocks(&self, pending_blocks: PendingBlocks) -> PendingBlocks {
            let mut blocks_processed = HashSet::new();
            let mut transactions_processed = 0;
            for (block_id, pending_block) in &pending_blocks {
                for (_, (tx, tx_result)) in &pending_block.transactions {
                    let (expected_tx, expected_tx_result) =
                        &self.transactions[transactions_processed];
                    assert_eq!(
                        pending_block.gas_recipient,
                        tx_result.gas_report.gas_recipient
                    );
                    assert!(tx.eq(expected_tx));
                    assert!(tx_result.eq(&expected_tx_result));
                    transactions_processed += 1;
                }
                blocks_processed.insert(block_id);
                if transactions_processed == self.transactions.len() {
                    break;
                }
            }

            let mut result = pending_blocks.clone();
            result.retain(|block_id, _| !blocks_processed.contains(block_id));
            result
        }
    }

    #[tokio::test]
    async fn test_filter_out_chain_id() {
        let storage = inmemory::EthereumBlockStorage::new(CHAIN_ID1);
        let wallet = create_wallet();
        let slot10_result_builder = BlockParseResultBuilder::new(10u64, 9u64)
            .add_transaction(CHAIN_ID1, &wallet, None)
            .await
            .add_transaction(CHAIN_ID2, &wallet, None)
            .await
            .add_transaction(CHAIN_ID2, &wallet, None)
            .await
            .add_transaction(CHAIN_ID1, &wallet, None)
            .await;

        let mut parse_results = BTreeMap::new();
        parse_results.insert(slot10_result_builder.slot(), slot10_result_builder.build());

        let (parent_hash, mut pending_blocks) = storage
            .register_parse_results(parse_results)
            .await
            .unwrap()
            .unwrap();

        assert!(parent_hash.is_none());
        assert_eq!(pending_blocks.len(), 1);
        pending_blocks = slot10_result_builder.check_pending_blocks(pending_blocks);
        assert!(pending_blocks.is_empty());
    }

    #[tokio::test]
    async fn test_grouping_gas_recipients() {
        let gas_recipient1 = Address::random();
        let gas_recipient2 = Address::random();
        let gas_recipient3 = Address::random();

        let storage = inmemory::EthereumBlockStorage::new(CHAIN_ID1);
        let wallet = create_wallet();
        let slot10_result_builder = BlockParseResultBuilder::new(10u64, 9u64)
            // EthBlock 0
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient2))
            .await
            // EthBlock 1
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
            .await
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
            .await
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
            .await
            // EthBlock 2
            .add_transaction(CHAIN_ID1, &wallet, None)
            .await
            // EthBlock 3
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient2))
            .await
            // EthBlock 4
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
            .await
            // EthBlock 5
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient3))
            .await
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient3))
            .await
            // EthBlock 6
            .add_transaction(CHAIN_ID1, &wallet, None)
            .await
            .add_transaction(CHAIN_ID1, &wallet, None)
            .await;

        let mut parse_results = BTreeMap::new();
        parse_results.insert(slot10_result_builder.slot(), slot10_result_builder.build());

        let (parent_hash, mut pending_blocks) = storage
            .register_parse_results(parse_results)
            .await
            .unwrap()
            .unwrap();

        assert!(parent_hash.is_none());
        assert_eq!(pending_blocks.len(), 7);
        pending_blocks = slot10_result_builder.check_pending_blocks(pending_blocks);
        assert!(pending_blocks.is_empty());
    }

    #[tokio::test]
    async fn test_grouping_gas_recipients_across_several_blocks() {
        let gas_recipient1 = Address::random();
        let gas_recipient2 = Address::random();
        let gas_recipient3 = Address::random();

        let storage = inmemory::EthereumBlockStorage::new(CHAIN_ID1);
        let wallet = create_wallet();
        let slot10_result_builder = BlockParseResultBuilder::new(10u64, 9u64)
            // EthBlock 0
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient2))
            .await
            // EthBlock 1
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
            .await
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
            .await
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
            .await
            // EthBlock 2
            .add_transaction(CHAIN_ID1, &wallet, None)
            .await;

        let slot12_result_builder = BlockParseResultBuilder::new(12u64, 10u64)
            // EthBlock 3
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient2))
            .await
            // EthBlock 4
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
            .await
            // EthBlock 5
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient3))
            .await
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient3))
            .await
            // EthBlock 6
            .add_transaction(CHAIN_ID1, &wallet, None)
            .await
            .add_transaction(CHAIN_ID1, &wallet, None)
            .await;

        let mut parse_results = BTreeMap::new();
        parse_results.insert(slot10_result_builder.slot(), slot10_result_builder.build());
        parse_results.insert(slot12_result_builder.slot(), slot12_result_builder.build());

        let (parent_hash, mut pending_blocks) = storage
            .register_parse_results(parse_results)
            .await
            .unwrap()
            .unwrap();

        assert!(parent_hash.is_none());
        assert_eq!(pending_blocks.len(), 7);
        pending_blocks = slot10_result_builder.check_pending_blocks(pending_blocks);
        assert_eq!(pending_blocks.len(), 4);
        pending_blocks = slot12_result_builder.check_pending_blocks(pending_blocks);
        assert!(pending_blocks.is_empty());
    }

    #[tokio::test]
    async fn test_longer_branch_is_pending() {
        /*
        Builds next block structure:

               /--slot11--------slot13
        slot10
               \---------slot12--------slot14---slot15

        Branch ending with slot 15 should be considered pending
         */
        let gas_recipient1 = Address::random();
        let gas_recipient2 = Address::random();
        let gas_recipient3 = Address::random();

        let storage = inmemory::EthereumBlockStorage::new(CHAIN_ID1);
        let wallet = create_wallet();
        let slot10_result_builder = BlockParseResultBuilder::new(10u64, 9u64)
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient2))
            .await
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
            .await
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
            .await
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
            .await
            .add_transaction(CHAIN_ID1, &wallet, None)
            .await;

        let slot11_result_builder = BlockParseResultBuilder::new(11u64, 10u64)
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
            .await
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
            .await;

        let slot12_result_builder = BlockParseResultBuilder::new(12u64, 10u64)
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient2))
            .await
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient3))
            .await
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient3))
            .await
            .add_transaction(CHAIN_ID1, &wallet, None)
            .await
            .add_transaction(CHAIN_ID1, &wallet, None)
            .await;

        let slot13_result_builder = BlockParseResultBuilder::new(13u64, 11u64)
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient2))
            .await
            .add_transaction(CHAIN_ID1, &wallet, None)
            .await
            .add_transaction(CHAIN_ID1, &wallet, None)
            .await;

        let slot14_result_builder = BlockParseResultBuilder::new(14u64, 12u64)
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
            .await
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
            .await
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient2))
            .await
            .add_transaction(CHAIN_ID1, &wallet, None)
            .await;

        let slot15_result_builder = BlockParseResultBuilder::new(15u64, 14u64)
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient2))
            .await
            .add_transaction(CHAIN_ID1, &wallet, None)
            .await;

        let mut parse_results = BTreeMap::new();
        parse_results.insert(slot10_result_builder.slot(), slot10_result_builder.build());
        parse_results.insert(slot11_result_builder.slot(), slot11_result_builder.build());
        parse_results.insert(slot12_result_builder.slot(), slot12_result_builder.build());
        parse_results.insert(slot13_result_builder.slot(), slot13_result_builder.build());
        parse_results.insert(slot14_result_builder.slot(), slot14_result_builder.build());
        parse_results.insert(slot15_result_builder.slot(), slot15_result_builder.build());

        let (parent_hash, mut pending_blocks) = storage
            .register_parse_results(parse_results)
            .await
            .unwrap()
            .unwrap();

        assert!(parent_hash.is_none());
        pending_blocks = slot10_result_builder.check_pending_blocks(pending_blocks);
        pending_blocks = slot12_result_builder.check_pending_blocks(pending_blocks);
        pending_blocks = slot14_result_builder.check_pending_blocks(pending_blocks);
        pending_blocks = slot15_result_builder.check_pending_blocks(pending_blocks);
        assert!(pending_blocks.is_empty());
    }

    #[tokio::test]
    async fn test_absent_result_should_not_cause_block_production_to_fail() {
        let gas_recipient1 = Address::random();
        let gas_recipient2 = Address::random();
        let gas_recipient3 = Address::random();

        let storage = inmemory::EthereumBlockStorage::new(CHAIN_ID1);
        let wallet = create_wallet();
        let slot10_result_builder = BlockParseResultBuilder::new(10u64, 9u64)
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient2))
            .await
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
            .await
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
            .await
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
            .await
            .add_transaction(CHAIN_ID1, &wallet, None)
            .await;

        let slot11_result_builder = BlockParseResultBuilder::new(11u64, 10u64)
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
            .await
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
            .await;

        let slot12_result_builder = BlockParseResultBuilder::new(12u64, 11u64)
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient2))
            .await
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient3))
            .await
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient3))
            .await
            .add_transaction(CHAIN_ID1, &wallet, None)
            .await
            .add_transaction(CHAIN_ID1, &wallet, None)
            .await;

        let mut parse_results = BTreeMap::new();
        parse_results.insert(slot10_result_builder.slot(), slot10_result_builder.build());
        parse_results.insert(slot11_result_builder.slot(), slot11_result_builder.build());
        parse_results.insert(slot12_result_builder.slot(), slot12_result_builder.build());
        let (_, pending_blocks) = storage
            .register_parse_results(parse_results)
            .await
            .unwrap()
            .unwrap();

        let mut results = BTreeMap::new();
        let mut new_parent_hash = None;
        for (block_id, _) in &pending_blocks {
            let blockhash = H256::random();
            results.insert(
                *block_id,
                BlockParams {
                    hash: blockhash,
                    parent_hash: new_parent_hash,
                    number: U64::from(random::<u64>()),
                    timestamp: U256::zero(),
                },
            );
            new_parent_hash = Some(blockhash);
        }

        // remove one of results
        results.remove(&(12u64, 1));

        let res = storage.blocks_produced(pending_blocks, results).await;

        assert!(!res.is_err());
    }

    #[tokio::test]
    async fn test_registration() {
        /*
        Builds next block structure:

        slot10--slot11--slot12--slot13
        */
        let gas_recipient1 = Address::random();
        let gas_recipient2 = Address::random();
        let gas_recipient3 = Address::random();

        let storage = inmemory::EthereumBlockStorage::new(CHAIN_ID1);
        let wallet = create_wallet();
        let slot10_result_builder = BlockParseResultBuilder::new(10u64, 9u64)
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient2))
            .await
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
            .await
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
            .await
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
            .await
            .add_transaction(CHAIN_ID1, &wallet, None)
            .await;

        let slot11_result_builder = BlockParseResultBuilder::new(11u64, 10u64)
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
            .await
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
            .await;

        let slot12_result_builder = BlockParseResultBuilder::new(12u64, 11u64)
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient2))
            .await
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient3))
            .await
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient3))
            .await
            .add_transaction(CHAIN_ID1, &wallet, None)
            .await
            .add_transaction(CHAIN_ID1, &wallet, None)
            .await;

        let slot13_result_builder = BlockParseResultBuilder::new(13u64, 12u64)
            .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient2))
            .await
            .add_transaction(CHAIN_ID1, &wallet, None)
            .await
            .add_transaction(CHAIN_ID1, &wallet, None)
            .await;

        let assert_included_in_parse_results =
            |expected_tx_hash: &H256, parse_results: &BTreeMap<Slot, BlockParseResult>| {
                for (_, result) in parse_results {
                    for (tx_hash, _) in &result.transactions {
                        if tx_hash.hash.eq(expected_tx_hash) {
                            return;
                        }
                    }
                }

                panic!("Transaction {expected_tx_hash} not found in parse results")
            };

        let mut block_number = 0;

        ////////////////////////////////////////////////////////////////////////////////////////////
        // First parse-register interation
        let mut parse_results = BTreeMap::new();
        parse_results.insert(slot10_result_builder.slot(), slot10_result_builder.build());
        parse_results.insert(slot11_result_builder.slot(), slot11_result_builder.build());
        let (parent_hash, pending_blocks) = storage
            .register_parse_results(parse_results.clone())
            .await
            .unwrap()
            .unwrap();

        assert!(parent_hash.is_none());
        let mut reg_results = BTreeMap::new();
        let mut new_parent_hash = None;
        for (block_id, _) in &pending_blocks {
            let blockhash = H256::random();
            reg_results.insert(
                *block_id,
                BlockParams {
                    hash: blockhash,
                    parent_hash: new_parent_hash,
                    number: U64::from(block_number),
                    timestamp: U256::zero(),
                },
            );
            block_number += 1;
            new_parent_hash = Some(blockhash);
        }

        let (_, last_result) = reg_results.last_key_value().unwrap();
        storage
            .blocks_produced(pending_blocks.clone(), reg_results.clone())
            .await
            .unwrap();
        assert_eq!(
            storage.latest_block().await.unwrap(),
            Some(U64::from(block_number - 1))
        );

        for (_, eth_block) in &reg_results {
            assert_eq!(
                storage.get_block_number(&eth_block.hash).await.unwrap(),
                Some(eth_block.number)
            );
            let block = storage
                .get_block_by_number(eth_block.number, false)
                .await
                .unwrap()
                .unwrap();

            match &block {
                BlockType::BlockWithHashes(block) => {
                    assert_eq!(block.hash, Some(eth_block.hash));
                    assert_eq!(block.number, Some(eth_block.number));
                    for tx_hash in &block.transactions {
                        assert_included_in_parse_results(&tx_hash, &parse_results)
                    }
                }
                _ => panic!("BlockWithHashes requested but BlockWithTransactions returned"),
            }
        }

        ////////////////////////////////////////////////////////////////////////////////////////////
        // Second parse-register interation
        let mut parse_results = BTreeMap::new();
        parse_results.insert(slot12_result_builder.slot(), slot12_result_builder.build());
        parse_results.insert(slot13_result_builder.slot(), slot13_result_builder.build());
        let (parent_hash, pending_blocks) = storage
            .register_parse_results(parse_results.clone())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(parent_hash, Some(last_result.hash));
        let mut reg_results = BTreeMap::new();
        let mut new_parent_hash = None;
        for (block_id, _) in &pending_blocks {
            let blockhash = H256::random();
            reg_results.insert(
                *block_id,
                BlockParams {
                    hash: blockhash,
                    parent_hash: new_parent_hash,
                    number: U64::from(block_number),
                    timestamp: U256::zero(),
                },
            );
            block_number += 1;
            new_parent_hash = Some(blockhash);
        }
        storage
            .blocks_produced(pending_blocks.clone(), reg_results.clone())
            .await
            .unwrap();

        assert_eq!(
            storage.latest_block().await.unwrap(),
            Some(U64::from(block_number - 1))
        );

        for (_, eth_block) in &reg_results {
            assert_eq!(
                storage.get_block_number(&eth_block.hash).await.unwrap(),
                Some(eth_block.number)
            );
            let block = storage
                .get_block_by_number(eth_block.number, false)
                .await
                .unwrap()
                .unwrap();
            match &block {
                BlockType::BlockWithHashes(block) => {
                    assert_eq!(block.hash, Some(eth_block.hash));
                    assert_eq!(block.number, Some(eth_block.number));
                    for tx_hash in &block.transactions {
                        assert_included_in_parse_results(&tx_hash, &parse_results)
                    }
                }
                _ => panic!("BlockWithHashes requested but BlockWithTransactions returned"),
            }
        }
    }

    #[tokio::test]
    async fn test_register_several_blocks_with_same_timestamp() {
        const TIMESTAMP: UnixTimestamp = 1234i64;
        let gas_recipient1 = Address::random();
        let gas_recipient2 = Address::random();
        let gas_recipient3 = Address::random();

        let storage = inmemory::EthereumBlockStorage::new(CHAIN_ID1);
        let wallet = create_wallet();

        let slot10_result_builder =
            BlockParseResultBuilder::new_with_timestamp(10u64, TIMESTAMP, 9u64)
                .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient2))
                .await
                .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
                .await
                .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
                .await
                .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
                .await
                .add_transaction(CHAIN_ID1, &wallet, None)
                .await;

        let slot11_result_builder =
            BlockParseResultBuilder::new_with_timestamp(11u64, TIMESTAMP, 10u64)
                .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
                .await
                .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient1))
                .await;

        let slot12_result_builder =
            BlockParseResultBuilder::new_with_timestamp(12u64, TIMESTAMP, 11u64)
                .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient2))
                .await
                .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient3))
                .await
                .add_transaction(CHAIN_ID1, &wallet, Some(gas_recipient3))
                .await
                .add_transaction(CHAIN_ID1, &wallet, None)
                .await
                .add_transaction(CHAIN_ID1, &wallet, None)
                .await;

        let mut parse_results = BTreeMap::new();
        parse_results.insert(slot10_result_builder.slot(), slot10_result_builder.build());
        parse_results.insert(slot11_result_builder.slot(), slot11_result_builder.build());
        parse_results.insert(slot12_result_builder.slot(), slot12_result_builder.build());
        let (parent_hash, mut pending_blocks) = storage
            .register_parse_results(parse_results.clone())
            .await
            .unwrap()
            .unwrap();

        assert!(parent_hash.is_none());
        pending_blocks = slot10_result_builder.check_pending_blocks(pending_blocks);
        pending_blocks = slot11_result_builder.check_pending_blocks(pending_blocks);
        pending_blocks = slot12_result_builder.check_pending_blocks(pending_blocks);
        assert!(pending_blocks.is_empty());
    }
}
