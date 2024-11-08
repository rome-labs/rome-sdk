use self::claim::EngineClaim;
use self::config::GethEngineConfig;
use crate::engine::types::param::{
    ForkchoiceUpdateParams, ForkchoiceUpdateResponse, PayloadStatus,
};
use anyhow::anyhow;
use ethers::core::types::Transaction;
use ethers::prelude::ProviderError::CustomError;
use reqwest::header::HeaderMap;
use rome_evm_client::indexer::tx_parser::GasReport;
use rome_utils::auth::AuthState;
use rome_utils::jsonrpc::{JsonRpcClient, JsonRpcRequest};
use serde_json::{from_value, json};
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

impl GethEngine {
    /// Create a new GethEngine instance
    pub fn new(config: GethEngineConfig) -> anyhow::Result<Self> {
        Ok(Self {
            _client: JsonRpcClient::new(config.geth_engine_addr),
            geth_engine_secret: config.geth_engine_secret,
            headers: Arc::new(RwLock::new(HeaderMap::default())),
        })
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

    async fn send_request(
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
        transactions: &Vec<(Transaction, GasReport)>,
        timestamp: u64,
    ) -> anyhow::Result<()> {
        // Fetch the latest block hash from geth client
        let get_block_req_params: Vec<serde_json::Value> = vec!["latest".into(), true.into()];
        let get_block_res = self
            .send_request("eth_getBlockByNumber", get_block_req_params)
            .await;
        let blockhash = get_block_res?
            .get("hash")
            .expect("Failed to get block hash")
            .clone();

        //
        // ------------------- Do fork choice update 1 -------------------
        //

        // Prev randao and suggested fee recipient are set to random values
        let hex_timestamp = format!("0x{:x}", timestamp);

        let gas_prices: Vec<u64> = transactions
            .iter()
            .filter_map(|(tx, _)| tx.gas_price)
            .map(|price| price.as_u64())
            .collect();

        let gas_reports: Vec<&GasReport> = transactions
            .iter()
            .map(|(_, gas_report)| gas_report)
            .collect();

        let first_report = gas_reports
            .first()
            .ok_or(anyhow!("Gas reports are empty"))?;
        let fee_recipient = first_report.gas_recipient.unwrap_or_default();
        for report in &gas_reports {
            if report.gas_recipient.unwrap_or_default() != fee_recipient {
                return Err(anyhow!("Transactions must have the same fee recipient"));
            }
        }

        let gas_used: Vec<u64> = gas_reports
            .iter()
            .map(|gas_report| gas_report.gas_value.as_u64())
            .collect();

        let forkchoice_params = ForkchoiceUpdateParams {
            transactions: transactions
                .iter()
                .map(|(tx, _)| serde_json::Value::from(tx.rlp().to_string()))
                .collect(),
            timestamp: hex_timestamp,
            prev_randao: "0xc130d5e63c61c935f6089e61140ca9136172677cf6aa5800dcc1cf0a02152a14"
                .to_string(),
            suggested_fee_recipient: format!("{:?}", fee_recipient),
            withdrawals: vec![],
            no_tx_pool: true,
            gas_prices,
            gas_used: gas_used.clone(),
        };
        let forkchoice_request = vec![
            json!({
                "headBlockHash": blockhash,
                "safeBlockHash": blockhash,
                "finalizedBlockHash": blockhash,
            }),
            serde_json::to_value(forkchoice_params).unwrap(),
        ];
        let fcu1_res = self.forkchoice_update(forkchoice_request).await;

        //
        // ------------------------- Get Payload -------------------------
        //

        let parsed_fcu_res: ForkchoiceUpdateResponse = from_value(fcu1_res)?;
        let payload_id = parsed_fcu_res.payload_id;
        // tracing::info!("Payload ID: {:#?}", payload_id);

        let get_payload_request = vec![json!(payload_id)];
        let payload_res = self.get_payload(get_payload_request).await;

        //
        // ------------------------- Send Payload -------------------------
        //

        let execution_payload = payload_res
            .get("executionPayload")
            .expect("Failed to get execution payload");
        let mut execution_payload = execution_payload.clone();
        execution_payload["romeGasUsed"] = json!(gas_used);

        let new_payload = vec![execution_payload.clone()];
        let send_payload_res = self.send_new_payload(new_payload).await;

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
        let fcu2_res = self.forkchoice_update(forkchoice_request).await;
        println!("Forkchoice response: {:?}", fcu2_res);

        Ok(())
    }

    /// Send a forkchoice update to the engine
    pub async fn forkchoice_update(&self, params: Vec<serde_json::Value>) -> serde_json::Value {
        let res = self
            .send_request("engine_forkchoiceUpdatedV2", params)
            .await;
        match res {
            Ok(response) => response,
            Err(e) => {
                panic!("Failed to do forkchoice_update: {:?}", e);
            }
        }
    }

    /// Get payload from the engine
    pub async fn get_payload(&self, params: Vec<serde_json::Value>) -> serde_json::Value {
        let res = self.send_request("engine_getPayloadV2", params).await;
        match res {
            Ok(response) => response,
            Err(e) => {
                panic!("Failed to get payload: {:?}", e);
            }
        }
    }

    /// Send new payload to the engine
    pub async fn send_new_payload(&self, params: Vec<serde_json::Value>) -> serde_json::Value {
        let res = self.send_request("engine_newPayloadV2", params).await;
        match res {
            Ok(response) => response,
            Err(e) => {
                panic!("Failed to send new payload: {:?}", e);
            }
        }
    }
}
