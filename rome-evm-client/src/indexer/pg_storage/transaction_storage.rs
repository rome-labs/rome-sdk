use crate::error::ProgramResult;
use crate::error::RomeEvmError::Custom;
use crate::indexer::pending_blocks::PendingBlocks;
use crate::indexer::pg_storage::{types::ReceiptParams, PgPool};
use crate::indexer::produced_blocks::ProducedBlocks;
use crate::indexer::{self, BlockParseResult, TxResult};
use diesel::{
    self,
    sql_types::{BigInt, Binary, Bytea, Json, Jsonb, Nullable, Text, VarChar},
    RunQueryDsl,
};
use diesel::{Connection, QueryableByName};
use ethers::abi::AbiEncode;
use ethers::prelude::{Bloom, Log, OtherFields, Transaction, TransactionReceipt, TxHash, U64};
use ethers::types::U256;
use rlp::{Decodable, Rlp};
use solana_program::clock::Slot;
use std::collections::BTreeMap;
use std::ops::Add;
use std::sync::Arc;

pub struct TransactionStorage {
    pool: Arc<PgPool>,
}

impl TransactionStorage {
    pub fn new(pool: Arc<PgPool>) -> Self {
        Self { pool }
    }
}

#[derive(QueryableByName, Debug)]
struct TransactionRow {
    #[diesel(sql_type = BigInt)]
    slot_number: i64,

    #[diesel(sql_type = Binary)]
    rlp: Vec<u8>,

    #[diesel(sql_type = Jsonb)]
    tx_result: serde_json::Value,

    #[diesel(sql_type = Nullable<Jsonb>)]
    receipt_params: Option<serde_json::Value>,
}

fn new_receipt(
    tx: &Transaction,
    tx_result: TxResult,
    receipt_params: ReceiptParams,
) -> TransactionReceipt {
    let mut transaction_log_index = receipt_params.first_log_index;
    let transaction_index = U64::from(receipt_params.tx_index);

    TransactionReceipt {
        transaction_hash: tx.hash,
        transaction_index,
        transaction_type: tx.transaction_type,
        block_hash: Some(receipt_params.blockhash),
        block_number: Some(receipt_params.block_number),
        from: tx.from,
        to: tx.to,
        gas_used: Some(tx_result.gas_report.gas_value),
        cumulative_gas_used: receipt_params
            .block_gas_used
            .checked_add(tx_result.gas_report.gas_value)
            .expect("This must never happen - block gas overflow!"),
        contract_address: indexer::calc_contract_address(&tx.from, &tx.to, &tx.nonce),
        logs: tx_result
            .logs
            .iter()
            .enumerate()
            .map(|(log_index, log)| {
                let result = Log {
                    block_hash: Some(receipt_params.blockhash),
                    block_number: Some(receipt_params.block_number),
                    transaction_hash: Some(tx.hash),
                    transaction_index: Some(transaction_index),
                    log_index: Some(
                        U256::from(log_index)
                            .checked_add(transaction_log_index)
                            .expect("Unable to increment log index"),
                    ),
                    transaction_log_index: Some(transaction_log_index),
                    removed: Some(false),
                    ..log.clone()
                };
                transaction_log_index = transaction_log_index.add(U256::one());
                result
            })
            .collect::<Vec<Log>>(),
        status: Some(U64::from((tx_result.exit_reason.code == 0) as u64)),
        logs_bloom: Bloom::default(),
        root: None,
        effective_gas_price: if let Some(price) = tx.gas_price {
            Some(price)
        } else if let Some(price) = tx.max_priority_fee_per_gas {
            Some(price)
        } else {
            panic!("No gas_price nor max_priority_fee_per_gas defined")
        },
        deposit_nonce: None,
        l1_fee: None,
        l1_fee_scalar: None,
        l1_gas_price: None,
        l1_gas_used: None,
        other: OtherFields::default(),
    }
}

impl TransactionStorage {
    pub async fn register_parse_results(
        &self,
        parse_result: &BTreeMap<Slot, BlockParseResult>,
    ) -> ProgramResult<()> {
        self.pool.get()?.transaction(|conn| {
            for parse_result in parse_result.values() {
                for (tx, tx_result) in &parse_result.transactions {
                    let tx_hash = tx.hash.encode_hex();
                    diesel::sql_query(
                        "INSERT INTO evm_tx (tx_hash, rlp) VALUES ($1, $2) ON CONFLICT DO NOTHING;",
                    )
                        .bind::<VarChar, _>(tx_hash.clone())
                        .bind::<Bytea, _>(tx.rlp().as_ref())
                        .execute(conn)?;

                    diesel::sql_query(
                        "INSERT INTO evm_tx_result (slot_number, tx_hash, tx_result) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING;",
                    )
                        .bind::<BigInt, _>(parse_result.slot_number as i64)
                        .bind::<VarChar, _>(tx_hash)
                        .bind::<Json, _>(serde_json::to_value(tx_result)?)
                        .execute(conn)?;
                }
            }

            Ok(())
        })
    }

    fn get_transaction_internal(&self, tx_hash: &TxHash) -> ProgramResult<Vec<TransactionRow>> {
        Ok(diesel::sql_query("SELECT * FROM get_transaction($1)")
            .bind::<Text, _>(format!("0x{:x}", tx_hash))
            .load(&mut self.pool.get()?)?)
    }

    pub async fn get_transaction_receipt(
        &self,
        tx_hash: &TxHash,
    ) -> ProgramResult<Option<TransactionReceipt>> {
        let rows = self.get_transaction_internal(tx_hash)?;
        let mut tx = None;
        let mut results = BTreeMap::new();

        for row in rows {
            let tx = tx.get_or_insert(Transaction::decode(&Rlp::new(&row.rlp)).map_err(|_| {
                Custom(format!(
                    "Failed to decode transaction 0x{:x} from rlp",
                    tx_hash
                ))
            })?);

            tx.from = tx.recover_from()?;
            let Some(receipt_params) = row.receipt_params else {
                continue;
            };

            let receipt_params: ReceiptParams = serde_json::from_value(receipt_params)?;
            let tx_result: TxResult = serde_json::from_value(row.tx_result)?;
            results.insert(row.slot_number, new_receipt(tx, tx_result, receipt_params));
        }

        Ok(results.last_key_value().map(|(_, receipt)| receipt.clone()))
    }

    pub async fn get_transaction(&self, tx_hash: &TxHash) -> ProgramResult<Option<Transaction>> {
        let rows = self.get_transaction_internal(tx_hash)?;
        let mut tx = None;
        let mut results = BTreeMap::new();

        for row in rows {
            let tx = tx.get_or_insert(Transaction::decode(&Rlp::new(&row.rlp)).map_err(|_| {
                Custom(format!(
                    "Failed to decode transaction 0x{:x} from rlp",
                    tx_hash
                ))
            })?);

            tx.from = tx.recover_from()?;
            if let Some(receipt_params) = row.receipt_params {
                let receipt_params: ReceiptParams = serde_json::from_value(receipt_params)?;
                tx.block_hash = Some(receipt_params.blockhash);
                tx.block_number = Some(receipt_params.block_number);
                tx.transaction_index = Some(U64::from(receipt_params.tx_index));
            };

            results.insert(row.slot_number, tx.clone());
        }

        Ok(results.last_key_value().map(|(_, tx)| tx.clone()))
    }

    pub async fn blocks_produced(
        &self,
        pending_blocks: &PendingBlocks,
        produced_blocks: ProducedBlocks,
    ) -> ProgramResult<()> {
        self.pool.get()?.transaction(|conn| {
            for (slot_number, block_idx, block_params) in produced_blocks.iter() {
                if let Some(pending_block) = pending_blocks.get(slot_number, block_idx) {
                    let mut block_gas_used = U256::zero();
                    let mut log_index = U256::zero();
                    for (tx_index, (tx, tx_result)) in &pending_block.transactions {
                        let receipt_params = ReceiptParams {
                            blockhash: block_params.hash,
                            block_number: block_params.number,
                            tx_index: *tx_index,
                            block_gas_used,
                            first_log_index: log_index,
                        };
                        block_gas_used += tx_result.gas_report.gas_value;
                        log_index += U256::from(tx_result.logs.len());
                        diesel::sql_query("UPDATE evm_tx_result SET receipt_params = $1 WHERE slot_number = $2 AND tx_hash = $3;")
                            .bind::<Nullable<Jsonb>, _>(Some(serde_json::to_value(receipt_params)?))
                            .bind::<BigInt, _>(*slot_number as i64)
                            .bind::<VarChar, _>(tx.hash.encode_hex())
                            .execute(conn)?;
                    }
                } else {
                    tracing::warn!("Pending block {:?} not found", (slot_number, block_idx));
                }
            }

            Ok(())
        })
    }
}
