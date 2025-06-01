use crate::error::{ProgramResult, RomeEvmError};
use crate::indexer::{BlockParams, BlockProducer, ProducerParams, ProductionResult};
use ethers::prelude::U64;

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct SingleStateBlockProducerConfig;

pub struct SingleStateBlockProducer;

impl SingleStateBlockProducer {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self
    }
}

impl TryFrom<&SingleStateBlockProducerConfig> for SingleStateBlockProducer {
    type Error = RomeEvmError;

    fn try_from(_: &SingleStateBlockProducerConfig) -> Result<Self, Self::Error> {
        Ok(Self::new())
    }
}

#[async_trait::async_trait]
impl BlockProducer for SingleStateBlockProducer {
    async fn last_produced_block(&self) -> ProgramResult<U64> {
        todo!()
    }

    async fn get_block_params(&self, _block_number: U64) -> ProgramResult<BlockParams> {
        todo!()
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
        let mut result = ProductionResult::default();
        let mut parent_blockhash = producer_params.parent_hash;
        let mut produced = 0;
        for (slot_number, pending_l1_block, l2_block_idx, _) in
            producer_params.pending_blocks.iter()
        {
            result.produced_blocks.insert(
                *slot_number,
                *l2_block_idx,
                BlockParams {
                    hash: pending_l1_block.blockhash,
                    parent_hash: parent_blockhash,
                    number: U64::from(*slot_number),
                    timestamp: pending_l1_block.timestamp,
                },
            );

            parent_blockhash = Some(pending_l1_block.blockhash);
            produced += 1;
            if let Some(limit) = limit {
                if produced >= limit {
                    break;
                }
            }
        }

        Ok(result)
    }
}
