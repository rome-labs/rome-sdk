use crate::error::RomeEvmError::Custom;
use crate::error::{ProgramResult, RomeEvmError};
use crate::indexer::{
    BlockParams, BlockProducer, FinalizedBlock, ProducedBlocks, ProducerParams, ProductionResult,
};
use async_trait::async_trait;
use ethers::prelude::{H256, U64};
use ethers::providers::{Http, Middleware};
use ethers::types::{BlockId, BlockNumber};
use rome_geth::engine::config::GethEngineConfig;
use rome_geth::engine::{GethEngine, StateAdvanceTx};
use std::sync::Arc;

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct EngineAPIBlockProducerConfig {
    pub geth_engine: GethEngineConfig,
    pub geth_api: String,
}

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

    #[tracing::instrument(name = "block_producer::get_finalized_blockhash", skip(self), fields(parent_hash = ?parent_hash))]
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

impl TryFrom<&EngineAPIBlockProducerConfig> for EngineAPIBlockProducer {
    type Error = RomeEvmError;

    fn try_from(config: &EngineAPIBlockProducerConfig) -> ProgramResult<Self> {
        Ok(EngineAPIBlockProducer::new(
            Arc::new(GethEngine::new(&config.geth_engine)),
            ethers::providers::Provider::<Http>::try_from(&config.geth_api).map_err(|e| {
                RomeEvmError::Custom(format!("Failed to create EngineAPIBlockProducer: {:?}", e))
            })?,
        ))
    }
}

#[async_trait]
impl BlockProducer for EngineAPIBlockProducer {
    async fn last_produced_block(&self) -> ProgramResult<U64> {
        Ok(self.geth_api.get_block_number().await?)
    }

    #[tracing::instrument(name = "block_producer::get_block_params", skip(self), fields(block_number = ?block_number))]
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

    #[tracing::instrument(
        name = "block_producer::produce_blocks",
        skip(self, producer_params, limit)
    )]
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
            let mut produced_blocks = ProducedBlocks::default();

            for (slot_number, pending_l1_block, block_idx, pending_l2_block) in
                producer_params.pending_blocks.iter()
            {
                let finalize_new_block = if let Some(next_finalized) = next_finalized {
                    next_finalized.ge(&(*slot_number, *block_idx))
                } else {
                    false
                };

                match self
                    .geth_engine
                    .advance_rollup_state(
                        current_finalized,
                        parent_hash,
                        pending_l1_block.timestamp,
                        pending_l2_block
                            .transactions
                            .values()
                            .map(|(tx, tx_result)| StateAdvanceTx {
                                rlp: tx.rlp().to_string(),
                                gas_price: tx_result.gas_report.gas_price.as_u64(),
                                gas_used: tx_result.gas_report.gas_value.as_u64(),
                            })
                            .collect(),
                        pending_l2_block.gas_recipient,
                        finalize_new_block,
                    )
                    .await
                {
                    Ok((block_number, block_hash)) => {
                        produced_blocks.insert(
                            *slot_number,
                            *block_idx,
                            BlockParams {
                                hash: block_hash,
                                parent_hash: Some(parent_hash),
                                number: block_number,
                                timestamp: pending_l1_block.timestamp,
                            },
                        );

                        parent_hash = block_hash;
                        if finalize_new_block {
                            current_finalized = block_hash;
                        }

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
                            (slot_number, block_idx),
                            err
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
