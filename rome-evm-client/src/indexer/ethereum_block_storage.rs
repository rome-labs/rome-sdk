use crate::error::ProgramResult;
use crate::indexer::parsers::block_parser::TxResult;
use crate::indexer::BlockParseResult;
use async_trait::async_trait;
use ethers::addressbook::Address;
use ethers::prelude::{
    Block, Bloom, Bytes, OtherFields, Transaction, TransactionReceipt, H256, H256 as TxHash, H64,
    U256,
};
use ethers::types::U64;
use jsonrpsee_core::Serialize;
use serde::Deserialize;
use solana_program::clock::UnixTimestamp;
use solana_sdk::clock::Slot;
use std::collections::BTreeMap;

// Ethereum block id.
// Consists of Solana slot number and index of the Eth block within this slot
pub type EthBlockId = (Slot, usize);

#[derive(Debug, Clone)]
pub enum FinalizedBlock {
    Produced(H256),
    Pending(EthBlockId),
}

#[derive(Clone, Default)]
pub struct PendingBlock {
    pub transactions: BTreeMap<usize, (Transaction, TxResult)>,
    pub gas_recipient: Option<Address>,
    pub slot_timestamp: Option<UnixTimestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockParams {
    pub hash: H256,
    pub parent_hash: Option<H256>,
    pub number: U64,
    pub timestamp: U256,
}

pub type PendingBlocks = BTreeMap<EthBlockId, PendingBlock>;
pub type ProducedBlocks = BTreeMap<EthBlockId, BlockParams>;

pub struct ProductionResult {
    pub produced_blocks: ProducedBlocks,
    pub finalized_block: H256,
}

#[async_trait]
pub trait BlockProducer: Send + Sync {
    async fn last_produced_block(&self) -> ProgramResult<U64>;

    async fn get_block_params(&self, block_number: U64) -> ProgramResult<BlockParams>;

    // Builds chain from pending_blocks on top of block with given parent_hash in a given order
    async fn produce_blocks(
        &self,
        producer_params: &ProducerParams,
        limit: Option<usize>,
    ) -> ProgramResult<ProductionResult>;
}

const SHA3_UNCLES: [u8; 32] = [
    0x1d, 0xcc, 0x4d, 0xe8, 0xde, 0xc7, 0x5d, 0x7a, 0xab, 0x85, 0xb5, 0x67, 0xb6, 0xcc, 0xd4, 0x1a,
    0xd3, 0x12, 0x45, 0x1b, 0x94, 0x8a, 0x74, 0x13, 0xf0, 0xa1, 0x42, 0xfd, 0x40, 0xd4, 0x93, 0x47,
];

#[derive(Clone, Serialize, Debug)]
#[serde(untagged)]
pub enum BlockType {
    BlockWithTransactions(Block<Transaction>),
    BlockWithHashes(Block<TxHash>),
}

impl BlockType {
    pub fn base<B>(
        hash: Option<H256>,
        parent_hash: H256,
        tx_root: H256,
        number: Option<U64>,
        gas_used: U256,
        timestamp: U256,
    ) -> Block<B> {
        Block::<B> {
            hash,
            parent_hash,
            number,
            gas_limit: U256::from(48000000000000u64),
            uncles_hash: H256::from(SHA3_UNCLES),
            author: Some(Address::default()),
            state_root: H256::zero(),
            transactions_root: tx_root,
            receipts_root: tx_root,
            gas_used,
            extra_data: Bytes::default(),
            logs_bloom: Some(Bloom::default()),
            timestamp,
            difficulty: U256::zero(),
            total_difficulty: Some(U256::zero()),
            seal_fields: vec![],
            uncles: vec![],
            transactions: vec![],
            size: Some(U256::one()),
            mix_hash: Some(H256::default()),
            nonce: Some(H64::zero()),
            base_fee_per_gas: Some(U256::zero()),
            blob_gas_used: None,
            excess_blob_gas: None,
            parent_beacon_block_root: None,
            withdrawals: None,
            withdrawals_root: None,
            other: OtherFields::default(),
        }
    }

    pub fn genesis(timestamp: U256, full_transactions: bool) -> Self {
        if full_transactions {
            BlockType::BlockWithTransactions(Self::base::<Transaction>(
                Some(H256::zero()),
                H256::zero(),
                H256::zero(),
                Some(U64::zero()),
                U256::zero(),
                timestamp,
            ))
        } else {
            BlockType::BlockWithHashes(Self::base::<TxHash>(
                Some(H256::zero()),
                H256::zero(),
                H256::zero(),
                Some(U64::zero()),
                U256::zero(),
                timestamp,
            ))
        }
    }
}

pub struct ProducerParams {
    pub parent_hash: Option<H256>,
    pub finalized_block: Option<FinalizedBlock>,
    pub pending_blocks: PendingBlocks,
}

pub struct ReproduceParams {
    pub producer_params: ProducerParams,
    pub expected_results: BTreeMap<EthBlockId, BlockParams>,
}

#[async_trait]
pub trait EthereumBlockStorage: Send + Sync {
    async fn reproduce_blocks(
        &self,
        from_slot: Slot,
        to_slot: Slot,
    ) -> ProgramResult<Option<ReproduceParams>>;

    async fn register_parse_results(
        &self,
        parse_results: BTreeMap<Slot, BlockParseResult>,
    ) -> ProgramResult<Option<ProducerParams>>;

    async fn latest_block(&self) -> ProgramResult<Option<U64>>;

    async fn get_block_number(&self, hash: &H256) -> ProgramResult<Option<U64>>;

    async fn get_block_by_number(
        &self,
        number: U64,
        full_transactions: bool,
    ) -> ProgramResult<Option<BlockType>>;

    async fn blocks_produced(
        &self,
        producer_params: &ProducerParams,
        produced_blocks: ProducedBlocks,
    ) -> ProgramResult<()>;

    fn chain(&self) -> u64;

    // Returns slot number of the latest slot which contains produced block(s)
    // or None if no blocks yet being produced
    async fn get_max_slot_produced(&self) -> ProgramResult<Option<Slot>>;

    async fn retain_from_slot(&self, from_slot: Slot) -> ProgramResult<()>;

    async fn get_transaction_receipt(
        &self,
        tx_hash: &TxHash,
    ) -> ProgramResult<Option<TransactionReceipt>>;

    async fn get_transaction(&self, tx_hash: &TxHash) -> ProgramResult<Option<Transaction>>;

    async fn get_slot_for_eth_block(&self, block_number: U64) -> ProgramResult<Option<Slot>>;

    async fn clean_from_slot(&self, from_slot: Slot) -> ProgramResult<()>;
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ReceiptParams {
    pub blockhash: H256,
    pub block_number: U64,
    pub tx_index: usize,
    pub block_gas_used: U256,
    pub first_log_index: U256,
}
