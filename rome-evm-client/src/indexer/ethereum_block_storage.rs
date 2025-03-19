use crate::error::ProgramResult;
use crate::indexer::pending_blocks::PendingBlocks;
use crate::indexer::produced_blocks::ProducedBlocks;
use crate::indexer::BlockParseResult;
use async_trait::async_trait;
use ethers::addressbook::Address;
use ethers::prelude::{
    Block, Bloom, Bytes, OtherFields, Transaction, TransactionReceipt, H256, H256 as TxHash, H64,
    U256,
};
use ethers::types::U64;
use jsonrpsee_core::Serialize;
use solana_sdk::clock::Slot;
use std::collections::BTreeMap;

// Ethereum block id.
// Consists of Solana slot number and index of the Eth block within this slot
pub type EthBlockId = (Slot, usize);

#[derive(Debug, Clone, Serialize)]
pub enum FinalizedBlock {
    Produced(H256),
    Pending(EthBlockId),
}

/// Contains set of pending blocks residing on the same branch of the Solana history and
/// additional parameters for its production
#[derive(Clone, Serialize)]
pub struct ProducerParams {
    /// Hash of the eth-block on top of which pending blocks must be appended
    pub parent_hash: Option<H256>,

    /// Latest finalized eth-block. it can yet have no blockhash (not produced). In that case,
    /// this block is referenced by Solana slot number and index within corresponding Solana block
    pub finalized_block: Option<FinalizedBlock>,

    /// Set of pending blocks all resides on the same branch of solana block history
    pub pending_blocks: PendingBlocks,
}

pub struct ReproduceParams {
    pub producer_params: ProducerParams,
    pub expected_results: ProducedBlocks,
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

#[async_trait]
pub trait EthereumBlockStorage: Send + Sync {
    /// Retrieves pending blocks that are not yet not produced (has no blockhashes/numbers).
    async fn get_pending_blocks(&self) -> ProgramResult<Option<ProducerParams>>;

    /// Used to reproduce blocks based on the provided range of `Slot` values (`from_slot` to `to_slot`).
    /// Returns reproducible parameters (`ReproduceParams`), which include:
    ///     - `ProducerParams` for block production.
    ///     - The expected results (`ProducedBlocks`).
    async fn reproduce_blocks(
        &self,
        from_slot: Slot,
        to_slot: Slot,
    ) -> ProgramResult<Option<ReproduceParams>>;

    /// Accepts parsed block data (`BlockParseResult`) associated with `Slot` values, provided as a `BTreeMap`.
    /// Stores these results for further processing.
    async fn register_parse_results(
        &self,
        parse_results: BTreeMap<Slot, BlockParseResult>,
    ) -> ProgramResult<()>;

    /// Retrieves the number of the most recently produced block.
    async fn latest_block(&self) -> ProgramResult<Option<U64>>;

    /// Fetches a block number based on its hash (`H256`).
    async fn get_block_number(&self, hash: &H256) -> ProgramResult<Option<U64>>;

    /// Retrieves a block by its number (`U64`).
    /// If `full_transactions` is `true`, it returns the block along with full transaction details (`Block<Transaction>`).
    /// If `false`, it returns a block with only transaction hashes (`Block<TxHash>`).
    async fn get_block_by_number(
        &self,
        number: U64,
        full_transactions: bool,
    ) -> ProgramResult<Option<BlockType>>;

    /// Registers blocks that have been produced, based on `ProducerParams` and the resulting `ProducedBlocks`.
    async fn blocks_produced(
        &self,
        producer_params: &ProducerParams,
        produced_blocks: ProducedBlocks,
    ) -> ProgramResult<()>;

    /// Returns the latest `Slot` value containing produced blocks.
    /// If no blocks have been produced yet, it returns `None`.
    async fn get_max_slot_produced(&self) -> ProgramResult<Option<Slot>>;

    /// Keeps only the blocks starting from the specified `Slot`. Blocks with lesser slot values are discarded.
    async fn retain_from_slot(&self, from_slot: Slot) -> ProgramResult<()>;

    /// Fetches the receipt of a transaction given its hash (`TxHash`).
    async fn get_transaction_receipt(
        &self,
        tx_hash: &TxHash,
    ) -> ProgramResult<Option<TransactionReceipt>>;

    /// Retrieves full transaction details for a specific hash (`TxHash`).
    async fn get_transaction(&self, tx_hash: &TxHash) -> ProgramResult<Option<Transaction>>;

    /// Maps an Ethereum block number (`U64`) to its corresponding Solana `Slot`.
    async fn get_slot_for_eth_block(&self, block_number: U64) -> ProgramResult<Option<Slot>>;

    /// Cleans or removes data starting from the given `Slot`.
    /// This method removes data for higher slots opposite to retain_from_slot
    async fn clean_from_slot(&self, from_slot: Slot) -> ProgramResult<()>;
}
