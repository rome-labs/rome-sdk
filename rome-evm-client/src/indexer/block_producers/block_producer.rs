use crate::error::ProgramResult;
use crate::indexer::produced_blocks::BlockParams;
use crate::indexer::{ProducedBlocks, ProducerParams};
use async_trait::async_trait;
use ethers::prelude::{H256, U64};
use serde::Deserialize;

#[derive(Default, Clone, Deserialize)]
pub struct ProductionResult {
    // Parameters of the new blocks produced
    pub produced_blocks: ProducedBlocks,
    // Hash of the latest finalized block
    pub finalized_block: H256,
}

// Calculates blockhashes and numbers for a given set of pending blocks.
// Also participates in indexer start sequence and returns parameters of already
// produced blocks
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
