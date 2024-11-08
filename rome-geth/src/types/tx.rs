use ethers::types::transaction::eip2930::AccessList;
use ethers::types::{
    transaction::eip2718::TypedTransaction, Address, Bytes, Eip1559TransactionRequest,
    Eip2930TransactionRequest, NameOrAddress, TransactionRequest, TxHash, H256, U256, U64,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use rome_utils::jsonrpc::JsonRpcResponse;

/// Channel to send transactions to the mempool
pub type GethTxPoolSender = tokio::sync::mpsc::UnboundedSender<Arc<JsonRpcResponse<GethTxPoolResult>>>;

/// Channel to receive transactions from the mempool
pub type GethTxPoolReceiver = tokio::sync::mpsc::UnboundedReceiver<Arc<JsonRpcResponse<GethTxPoolResult>>>;

/// Represents a transaction in the Geth transaction pool.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GethTxPoolTx {
    /// The hash of the block containing this transaction.
    pub block_hash: Option<H256>,
    /// The number of the block containing this transaction.
    pub block_number: Option<U64>,
    /// The chain ID of the transaction.
    pub chain_id: Option<U64>,
    /// The address of the sender of this transaction.
    pub from: Address,
    /// The gas provided by the sender.
    pub gas: Option<U256>,
    /// The gas price provided by the sender in wei.
    pub gas_price: Option<U256>,
    /// The maximum priority fee per gas the sender is willing to pay.
    pub max_priority_fee_per_gas: Option<U256>,
    /// The maximum fee per gas the sender is willing to pay.
    pub max_fee_per_gas: Option<U256>,
    /// The hash of this transaction.
    pub hash: TxHash,
    /// The input data of the transaction.
    pub input: Option<Bytes>,
    /// The nonce of the transaction.
    pub nonce: U256,
    /// The R component of the signature.
    pub r: U256,
    /// The S component of the signature.
    pub s: U256,
    /// The address of the receiver of this transaction.
    pub to: Option<Address>,
    /// The transaction index within the block.
    pub transaction_index: Option<U64>,
    /// The transaction type.
    #[serde(rename = "type")]
    pub type_field: U64,
    /// The recovery identifier.
    pub v: U64,
    /// The value transferred in the transaction.
    pub value: Option<U256>,
}

/// Response from `txpool_content` method
#[derive(Debug, Deserialize, Serialize)]
pub struct GethTxPoolResult {
    /// address to nonce map for pending transactions
    pub pending: HashMap<String, BTreeMap<u64, GethTxPoolTx>>,
    /// address to nonce map for queued transactions
    pub queued: HashMap<String, BTreeMap<u64, GethTxPoolTx>>,
}

#[derive(Debug)]
pub enum PoolTxConversionError {
    UnknownTypeValue(u64),
}

fn try_into_legacy(value: &GethTxPoolTx) -> Result<TypedTransaction, PoolTxConversionError> {
    Ok(TypedTransaction::Legacy(TransactionRequest {
        from: Some(value.from),
        to: value.to.map(NameOrAddress::Address),
        gas: value.gas,
        gas_price: value.gas_price,
        value: value.value,
        data: value.input.clone(),
        nonce: Some(value.nonce),
        chain_id: value.chain_id,
    }))
}

fn try_into_eip2930(value: &GethTxPoolTx) -> Result<TypedTransaction, PoolTxConversionError> {
    Ok(TypedTransaction::Eip2930(Eip2930TransactionRequest {
        tx: TransactionRequest {
            from: Some(value.from),
            to: value.to.map(NameOrAddress::Address),
            gas: value.gas,
            gas_price: value.gas_price,
            value: value.value,
            data: value.input.clone(),
            nonce: Some(value.nonce),
            chain_id: value.chain_id,
        },
        access_list: AccessList::default(),
    }))
}

fn try_into_eip1559(value: &GethTxPoolTx) -> Result<TypedTransaction, PoolTxConversionError> {
    Ok(TypedTransaction::Eip1559(Eip1559TransactionRequest {
        from: Some(value.from),
        to: value.to.map(NameOrAddress::Address),
        gas: value.gas,
        value: value.value,
        data: value.input.clone(),
        nonce: Some(value.nonce),
        chain_id: value.chain_id,
        max_priority_fee_per_gas: value.max_priority_fee_per_gas,
        max_fee_per_gas: value.max_fee_per_gas,
        access_list: AccessList::default(),
    }))
}

impl TryFrom<&GethTxPoolTx> for TypedTransaction {
    type Error = PoolTxConversionError;

    fn try_from(value: &GethTxPoolTx) -> Result<Self, Self::Error> {
        match value.type_field.as_u64() {
            0 => try_into_legacy(value),
            1 => try_into_eip2930(value),
            2 => try_into_eip1559(value),
            unknown => Err(PoolTxConversionError::UnknownTypeValue(unknown)),
        }
    }
}
