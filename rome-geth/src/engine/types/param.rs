// TODO: replace serde_json::Value with actual types of ethereum transactions
// NOTE: don't use ethers-rs types, it's deprecated and un-compatible with other dependencies
use bigdecimal::BigDecimal;
use serde::{Deserialize, Serialize};

/// Forkchoice update parameters
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkchoiceUpdateParams {
    /// list of transactions
    pub transactions: Vec<serde_json::Value>,
    /// gas prices for transactions
    pub gas_prices: Vec<u64>,
    /// gas used for transactions
    pub gas_used: Vec<u64>,
    /// timestamp
    pub timestamp: String,
    /// Previous randao value
    pub prev_randao: String,
    /// Suggestions for the fee recipient
    pub suggested_fee_recipient: String,
    /// Withdrawals
    pub withdrawals: Vec<serde_json::Value>,
    /// No Tx Pool
    pub no_tx_pool: bool,
}

/// Payload status
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PayloadStatus {
    /// Status of the payload
    pub status: String,
    /// Latest valid hash
    pub latest_valid_hash: String,
    /// Validation error
    pub validation_error: Option<String>,
}

/// Fork choice response
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkchoiceUpdateResponse {
    /// Payload status
    pub payload_status: PayloadStatus,
    /// Payload ID
    pub payload_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionPayload {
    pub parent_hash: String,
    pub fee_recipient: String,
    pub state_root: String,
    pub receipts_root: String,
    pub logs_bloom: String,
    pub prev_randao: String,
    pub block_number: u64,
    pub gas_limit: u64,
    pub gas_used: u64,
    pub rome_gas_used: Option<Vec<u64>>,
    pub timestamp: u64,
    pub extra_data: String,

    #[serde(with = "bigdecimal::serde::json_num_option")]
    pub base_fee_per_gas: Option<BigDecimal>,
    pub block_hash: String,
    pub transactions: Vec<serde_json::Value>,
    pub withdrawals: Option<Vec<serde_json::Value>>,
    pub blob_gas_used: Option<u64>,
    pub excess_blob_gas: Option<u64>,
}
