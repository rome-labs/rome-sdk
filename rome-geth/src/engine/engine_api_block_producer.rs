use crate::engine::GethEngine;
use async_trait::async_trait;
use ethers::prelude::{H256, U256, U64};
use ethers::providers::{Http, Middleware};
use ethers::types::{BlockId, BlockNumber};
use rome_evm_client::error::ProgramResult;
use rome_evm_client::error::RomeEvmError::Custom;
use rome_evm_client::indexer::{
    BlockParams, BlockProducer, FinalizedBlock, ProducedBlocks, ProducerParams, ProductionResult,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct EngineAPIBlockProducer {
    geth_engine: Arc<GethEngine>,
    geth_api: ethers::providers::Provider<Http>,
}

impl EngineAPIBlockProducer {
    pub fn new(geth_engine: Arc<GethEngine>, geth_api: ethers::providers::Provider<Http>) -> Self {
        Self {
            geth_engine,
            geth_api,
        }
    }

    async fn get_finalized_blockhash(&self, parent_hash: &Option<H256>) -> ProgramResult<H256> {
        if parent_hash.is_some() {
            if let Ok(Some(block)) = self
                .geth_api
                .get_block(BlockId::Number(BlockNumber::Finalized))
                .await
            {
                block
                    .hash
                    .ok_or(Custom("Failed to get finalized blockhash".to_string()))
            } else {
                Err(Custom("Failed to get finalized block".to_string()))
            }
        } else if let Ok(Some(block)) = self
            .geth_api
            .get_block(BlockId::Number(BlockNumber::Latest))
            .await
        {
            block
                .hash
                .ok_or(Custom("Failed to get latest blockhash".to_string()))
        } else {
            Err(Custom("Failed to get latest block".to_string()))
        }
    }
}

#[async_trait]
impl BlockProducer for EngineAPIBlockProducer {
    async fn last_produced_block(&self) -> ProgramResult<U64> {
        Ok(self.geth_api.get_block_number().await?)
    }

    async fn get_block_params(&self, block_number: U64) -> ProgramResult<BlockParams> {
        let Some(block) = self
            .geth_api
            .get_block(BlockId::Number(BlockNumber::Number(block_number)))
            .await?
        else {
            return Err(Custom(format!("Block {:?} was not found", block_number)));
        };

        if let (Some(hash), Some(number)) = (block.hash, block.number) {
            Ok(BlockParams {
                hash,
                parent_hash: Some(block.parent_hash),
                number,
                timestamp: block.timestamp,
            })
        } else {
            Err(Custom(format!(
                "Failed to get block {:?} params",
                block_number
            )))
        }
    }

    async fn produce_blocks(
        &self,
        producer_params: &ProducerParams,
        limit: Option<usize>,
    ) -> ProgramResult<ProductionResult> {
        let parent_block = if let Some(parent_hash) = &producer_params.parent_hash {
            self.geth_api
                .get_block(BlockId::Hash(*parent_hash))
                .await
                .map_err(|e| {
                    Custom(format!(
                        "produce_blocks: Failed to get block for {:?}: {:?}",
                        parent_hash, e
                    ))
                })?
                .unwrap_or_else(|| panic!("Parent block {:?} was not found", parent_hash))
        } else if let Ok(Some(parent_block)) = self
            .geth_api
            .get_block(BlockId::Number(BlockNumber::Latest))
            .await
        {
            if let Some(block_number) = parent_block.number {
                if block_number != U64::zero() {
                    panic!(
                        "Unable to produce pending blocks with unknown parent. \
                        op-geth already has latest block with number {}.",
                        block_number
                    );
                }

                parent_block
            } else {
                panic!("Block number is not set for latest block");
            }
        } else {
            return Err(Custom(
                "produce_blocks: Failed to get latest block".to_string(),
            ));
        };

        let finalized_blockhash = if parent_block
            .number
            .unwrap_or_else(|| panic!("Parent block has no number"))
            != U64::zero()
        {
            self.get_finalized_blockhash(&producer_params.parent_hash)
                .await?
        } else {
            parent_block
                .hash
                .unwrap_or_else(|| panic!("Parent block has no hash"))
        };

        let (mut current_finalized, next_finalized) = match &producer_params.finalized_block {
            Some(FinalizedBlock::Produced(finalized_blockhash)) => (*finalized_blockhash, None),
            Some(FinalizedBlock::Pending(block_id)) => (finalized_blockhash, Some(block_id)),
            None => (finalized_blockhash, None),
        };

        if let Some(blockhash) = parent_block.hash {
            let mut parent_hash = blockhash;
            let mut produced_blocks = ProducedBlocks::new();

            for (block_id, pending_block) in &producer_params.pending_blocks {
                let adjusted_timestamp = pending_block
                    .slot_timestamp
                    .map(|v| U256::from(v))
                    .unwrap_or(parent_block.timestamp);

                let finalize_new_block = if let Some(next_finalized) = next_finalized {
                    next_finalized >= block_id
                } else {
                    false
                };

                match self
                    .geth_engine
                    .advance_rollup_state(
                        current_finalized,
                        parent_hash,
                        adjusted_timestamp,
                        pending_block,
                        finalize_new_block,
                    )
                    .await
                {
                    Ok(res) => {
                        parent_hash = res.hash;
                        if finalize_new_block {
                            current_finalized = res.hash;
                        }

                        produced_blocks.insert(*block_id, res);
                        if let Some(limit) = limit {
                            if produced_blocks.len() >= limit {
                                break;
                            }
                        }
                    }
                    Err(err) => {
                        return Err(Custom(format!(
                            "Failed to produce block in op-geth. {:?}: {:?}\n\
                            Take a look at op-geth logs for details.",
                            block_id, err
                        )))
                    }
                }
            }

            Ok(ProductionResult {
                produced_blocks,
                finalized_block: current_finalized,
            })
        } else {
            Err(Custom(format!(
                "Unable to read parent block {:?}",
                parent_block
            )))
        }
    }
}
