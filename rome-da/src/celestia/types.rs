use ethers::types::{Transaction, U256, U64};
use serde::{Deserialize, Serialize};


#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DaSubmissionBlock {
    pub block_number: U64,
    pub timestamp: U256,
    pub transactions: Vec<Transaction>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BlobParam {
    pub namespace: String,
    pub data: String,
    pub share_version: i32,
    pub commitment: String,
    pub index: i32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SubscribeBlobResponse {
    #[serde(rename = "Height")]
    pub height: u64,

    #[serde(rename = "Blobs")]
    pub blobs: Vec<BlobParam>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TxConfig {
    pub gas_price: Option<f64>,
    pub is_gas_price_set: Option<bool>,
    pub gas: Option<u64>,
    pub key_name: Option<String>,
    pub signer_address: Option<String>,
    pub fee_granter_address: Option<String>,
}
