// TODO: replace serde_json::Value with actual types of ethereum transactions
// NOTE: don't use ethers-rs types, it's deprecated and un-compatible with other dependencies
use serde::{Deserialize, Serialize};

/// Forkchoice update parameters
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkchoiceUpdateParams {
    /// list of transactions
    pub transactions: Vec<serde_json::Value>,
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
