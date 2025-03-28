use self::claim::EngineClaim;
use self::config::GethEngineConfig;
use crate::engine::types::param::{
    ExecutionPayload, ForkchoiceUpdateParams, ForkchoiceUpdateResponse, PayloadStatus,
};
use ethers::core::types::{H256, U64};
use ethers::prelude::ProviderError::CustomError;
use ethers::types::{Address, U256};
use reqwest::header::HeaderMap;
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
/// Types related to geth engine.
pub mod types;

/// Geth Engine
#[derive(Debug)]
pub struct GethEngine {
    _client: JsonRpcClient,
    geth_engine_secret: String,
    headers: Arc<RwLock<HeaderMap>>,
}

pub struct StateAdvanceTx {
    pub rlp: String,
    pub gas_price: u64,
    pub gas_used: u64,
}

impl GethEngine {
    /// Create a new GethEngine instance
    pub fn new(config: &GethEngineConfig) -> Self {
        Self {
            _client: JsonRpcClient::new(config.geth_engine_addr.clone()),
            geth_engine_secret: config.geth_engine_secret.clone(),
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
        finalized_blockhash: H256,
        parent_hash: H256,
        timestamp: U256,
        transactions: Vec<StateAdvanceTx>,
        gas_recipient: Option<Address>,
        finalize_new_block: bool,
    ) -> anyhow::Result<(U64, H256)> {
        //
        // ------------------- Do fork choice update 1 -------------------
        //

        let finalized_blockhash = format!("0x{:x}", finalized_blockhash);
        let gas_used = transactions
            .iter()
            .map(|tx| tx.gas_used)
            .collect::<Vec<u64>>();

        let forkchoice_params = ForkchoiceUpdateParams {
            transactions: transactions
                .iter()
                .map(|tx| serde_json::Value::from(tx.rlp.clone()))
                .collect(),
            timestamp: format!("0x{:x}", timestamp),
            prev_randao: "0xc130d5e63c61c935f6089e61140ca9136172677cf6aa5800dcc1cf0a02152a14"
                .to_string(),
            suggested_fee_recipient: format!("{:?}", gas_recipient.unwrap_or_default()),
            withdrawals: vec![],
            no_tx_pool: true,
            gas_prices: transactions.iter().map(|tx| tx.gas_price).collect(),
            gas_used: gas_used.clone(),
        };

        let blockhash = format!("0x{:x}", parent_hash);
        let forkchoice_request = vec![
            json!({
                "headBlockHash": blockhash,
                "safeBlockHash": blockhash,
                "finalizedBlockHash": finalized_blockhash,
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

        let new_finalized_blockhash = if finalize_new_block {
            new_blockhash.clone()
        } else {
            finalized_blockhash
        };

        let forkchoice_request = vec![
            json!({
                "headBlockHash": new_blockhash,
                "safeBlockHash": new_blockhash,
                "finalizedBlockHash": new_finalized_blockhash,
            }),
            json!(null),
        ];

        let _ = self
            .send_request("engine_forkchoiceUpdatedV2", forkchoice_request)
            .await?;

        Ok((block_number, block_hash))
    }
}
