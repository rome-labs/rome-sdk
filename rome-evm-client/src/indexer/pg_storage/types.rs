use diesel_derive_enum::DbEnum;
use ethers::prelude::{H256, U256, U64};
use jsonrpsee_core::Serialize;
use serde::Deserialize;

#[derive(DbEnum, Debug, PartialEq)]
pub enum SlotStatus {
    #[db_rename = "Processed"]
    Processed,
    #[db_rename = "Confirmed"]
    Confirmed,
    #[db_rename = "Finalized"]
    Finalized,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ReceiptParams {
    pub blockhash: H256,
    pub block_number: U64,
    pub tx_index: usize,
    pub block_gas_used: U256,
    pub first_log_index: U256,
}
