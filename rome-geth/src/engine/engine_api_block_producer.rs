use crate::engine::GethEngine;
use async_trait::async_trait;
use ethers::abi::AbiEncode;
use ethers::prelude::{H256, U256, U64};
use ethers::providers::{Http, Middleware};
use rome_evm_client::error::ProgramResult;
use rome_evm_client::error::RomeEvmError::Custom;
use rome_evm_client::indexer::{BlockParams, BlockProducer, PendingBlocks, ProducedBlocks};
use std::collections::BTreeMap;
use std::ops::Add;
use std::str::FromStr;
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
}

#[async_trait]
impl BlockProducer for EngineAPIBlockProducer {
    async fn last_produced_block(&self) -> ProgramResult<U64> {
        Ok(self.geth_api.get_block_number().await?)
    }

    async fn get_block_params(&self, block_number: U64) -> ProgramResult<BlockParams> {
        let res = self
            .geth_engine
            .send_request(
                "eth_getBlockByNumber",
                vec![format!("0x{:x}", block_number).into(), false.into()],
            )
            .await
            .map_err(|e| {
                Custom(format!(
                    "get_block_params: Failed to get block for {:?}: {:?}",
                    block_number, e
                ))
            })?;

        if let (Some(hash), parent_hash, Some(number), Some(timestamp)) = (
            res.get("hash"),
            res.get("parentHash"),
            res.get("number"),
            res.get("timestamp"),
        ) {
            Ok(BlockParams {
                hash: H256::from_str(hash.as_str().unwrap()).unwrap(),
                parent_hash: parent_hash.map(|v| H256::from_str(v.as_str().unwrap()).unwrap()),
                number: U64::from_str(number.as_str().unwrap()).unwrap(),
                timestamp: U256::from_str(timestamp.as_str().unwrap()).unwrap(),
            })
        } else {
            Err(Custom(format!(
                "Failed to decode response into BlockParams: {:?}",
                res
            )))
        }
    }

    async fn produce_blocks(
        &self,
        parent_hash: Option<H256>,
        pending_blocks: &PendingBlocks,
    ) -> ProgramResult<ProducedBlocks> {
        let parent_block = if let Some(parent_hash) = parent_hash {
            self.geth_engine
                .send_request(
                    "eth_getBlockByHash",
                    vec![parent_hash.encode_hex().into(), false.into()],
                )
                .await
        } else {
            self.geth_engine
                .send_request("eth_getBlockByNumber", vec!["latest".into(), false.into()])
                .await
        }
        .map_err(|e| {
            Custom(format!(
                "produce_blocks: Failed to get block for {:?}: {:?}",
                parent_hash, e
            ))
        })?;

        if let (Some(blockhash), Some(parent_timestamp)) =
            (parent_block.get("hash"), parent_block.get("timestamp"))
        {
            let mut parent_hash = H256::from_str(blockhash.as_str().unwrap()).unwrap();
            let mut timestamp = U256::from_str(parent_timestamp.as_str().unwrap())
                .unwrap()
                .add(1);
            let mut results = BTreeMap::new();
            for (block_id, pending_block) in pending_blocks {
                let adjusted_timestamp = pending_block
                    .slot_timestamp
                    .map(|v| {
                        let slot_timestamp = U256::from(v * 1000);
                        if slot_timestamp <= timestamp {
                            timestamp
                        } else {
                            slot_timestamp
                        }
                    })
                    .unwrap_or(timestamp);

                match self
                    .geth_engine
                    .advance_rollup_state(parent_hash, adjusted_timestamp, pending_block)
                    .await
                {
                    Ok(res) => {
                        parent_hash = res.hash;
                        results.insert(*block_id, res);
                        timestamp = adjusted_timestamp.add(1);
                    }
                    Err(err) => {
                        return Err(Custom(format!(
                            "Failed to advance state for block {:?}: {:?}",
                            block_id, err
                        )))
                    }
                }
            }

            Ok(results)
        } else {
            Err(Custom(format!(
                "Unable to read parent block {:?}",
                parent_block
            )))
        }
    }
}
