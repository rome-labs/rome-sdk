use self::claim::EngineClaim;
use self::config::GethEngineConfig;
use crate::engine::types::param::{
    ExecutionPayload, ForkchoiceUpdateParams, ForkchoiceUpdateResponse, PayloadStatus,
};
use ethers::core::types::{H256, U64};
use ethers::prelude::ProviderError::CustomError;
use ethers::types::U256;
use reqwest::header::HeaderMap;
use rome_evm_client::indexer::TxResult;
use rome_evm_client::indexer::{BlockParams, PendingBlock};
use rome_utils::auth::AuthState;
use rome_utils::jsonrpc::{JsonRpcClient, JsonRpcRequest};
use serde_json::{from_value, json, to_value};
use std::fmt::Debug;
use std::ops::Deref;
use std::str::FromStr;
use std::sync::{Arc, RwLock};

/// Claim to authenticate geth engine.
pub mod claim;
/// Geth engine configuration.
pub mod config;
pub mod engine_api_block_producer;
/// Types related to geth engine.
pub mod types;

/// Geth Engine
#[derive(Debug)]
pub struct GethEngine {
    _client: JsonRpcClient,
    geth_engine_secret: String,
    headers: Arc<RwLock<HeaderMap>>,
}

impl GethEngine {
    /// Create a new GethEngine instance
    pub fn new(config: GethEngineConfig) -> Self {
        Self {
            _client: JsonRpcClient::new(config.geth_engine_addr),
            geth_engine_secret: config.geth_engine_secret,
            headers: Arc::new(RwLock::new(HeaderMap::default())),
        }
    }

    fn get_headers(&self) -> anyhow::Result<HeaderMap> {
        Ok(self
            .headers
            .read()
            .map_err(|_| CustomError("RwLock error".to_string()))?
            .deref()
            .clone())
    }

    fn update_token(&self) -> anyhow::Result<HeaderMap> {
        let auth_state = AuthState::from_str(&self.geth_engine_secret)?;
        let claim = EngineClaim::new();
        let token = claim.to_token(&auth_state)?;

        let mut lock = self
            .headers
            .write()
            .map_err(|_| CustomError("RwLock error".to_string()))?;
        lock.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&format!("Bearer {}", token))?,
        );

        tracing::info!("Engine API token updated");
        Ok(lock.deref().clone())
    }

    pub async fn send_request(
        &self,
        method: &str,
        params: Vec<serde_json::Value>,
    ) -> anyhow::Result<serde_json::Value> {
        // First attempt to send
        match self
            ._client
            .send_req::<serde_json::Value>(
                JsonRpcRequest::new_with_params_owned(method, params.clone()),
                Some(self.get_headers()?),
            )
            .await
        {
            Ok(res) => Ok(res),
            Err(err) => {
                if let Some(req_err) = err.downcast_ref::<reqwest::Error>() {
                    if let Some(status_code) = req_err.status() {
                        // Authentication/authorization errors - generate token and make second attempt
                        if status_code.as_u16() == 401u16 || status_code.as_u16() == 403u16 {
                            self._client
                                .send_req::<serde_json::Value>(
                                    JsonRpcRequest::new_with_params_owned(method, params.clone()),
                                    Some(self.update_token()?),
                                )
                                .await
                        } else {
                            Err(err)
                        }
                    } else {
                        Err(err)
                    }
                } else {
                    Err(err)
                }
            }
        }
    }

    pub async fn advance_rollup_state(
        &self,
        parent_hash: H256,
        timestamp: U256,
        pending_block: &PendingBlock,
    ) -> anyhow::Result<BlockParams> {
        //
        // ------------------- Do fork choice update 1 -------------------
        //

        let gas_prices: Vec<u64> = pending_block
            .transactions
            .iter()
            .filter_map(|(tx, _)| tx.gas_price)
            .map(|price| price.as_u64())
            .collect();

        let tx_results: Vec<&TxResult> = pending_block
            .transactions
            .iter()
            .map(|(_, tx_result)| tx_result)
            .collect();

        let gas_used: Vec<u64> = tx_results
            .iter()
            .map(|tx_result| tx_result.gas_report.gas_value.as_u64())
            .collect();

        let forkchoice_params = ForkchoiceUpdateParams {
            transactions: pending_block
                .transactions
                .iter()
                .map(|(tx, _)| serde_json::Value::from(tx.rlp().to_string()))
                .collect(),
            timestamp: format!("0x{:x}", timestamp),
            prev_randao: "0xc130d5e63c61c935f6089e61140ca9136172677cf6aa5800dcc1cf0a02152a14"
                .to_string(),
            suggested_fee_recipient: format!(
                "{:?}",
                pending_block.gas_recipient.unwrap_or_default()
            ),
            withdrawals: vec![],
            no_tx_pool: true,
            gas_prices,
            gas_used: gas_used.clone(),
        };

        let blockhash = serde_json::Value::from(format!("0x{:x}", parent_hash));
        let forkchoice_request = vec![
            json!({
                "headBlockHash": blockhash,
                "safeBlockHash": blockhash,
                "finalizedBlockHash": blockhash,
            }),
            to_value(forkchoice_params).unwrap(),
        ];
        let fcu1_res = self
            .send_request("engine_forkchoiceUpdatedV2", forkchoice_request)
            .await?;

        //
        // ------------------------- Get Payload -------------------------
        //

        let parsed_fcu_res: ForkchoiceUpdateResponse = from_value(fcu1_res)?;
        let payload_id = parsed_fcu_res.payload_id;
        // tracing::info!("Payload ID: {:#?}", payload_id);

        let get_payload_request = vec![json!(payload_id)];
        let payload_res = self
            .send_request("engine_getPayloadV2", get_payload_request)
            .await?;

        //
        // ------------------------- Send Payload -------------------------
        //

        let execution_payload = payload_res
            .get("executionPayload")
            .expect("Failed to get execution payload");

        let mut execution_payload: ExecutionPayload = from_value(execution_payload.clone())?;
        let block_hash = H256::from_str(execution_payload.block_hash.as_str())?;
        let block_number = U64::from(execution_payload.block_number);
        execution_payload.rome_gas_used = Some(gas_used);
        let send_payload_res = self
            .send_request("engine_newPayloadV2", vec![to_value(execution_payload)?])
            .await?;
        let parsed_send_payload_res: PayloadStatus = from_value(send_payload_res)?;
        let new_blockhash = parsed_send_payload_res.latest_valid_hash;

        //
        // ------------------- Do fork choice update 2 -------------------
        //

        let forkchoice_request = vec![
            json!({
                "headBlockHash": new_blockhash,
                "safeBlockHash": new_blockhash,
                "finalizedBlockHash": new_blockhash,
            }),
            json!(null),
        ];

        let _ = self
            .send_request("engine_forkchoiceUpdatedV2", forkchoice_request)
            .await?;

        Ok(BlockParams {
            hash: block_hash,
            parent_hash: Some(parent_hash),
            number: block_number,
            timestamp,
        })
    }
}
