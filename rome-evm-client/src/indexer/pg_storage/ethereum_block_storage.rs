use crate::error::RomeEvmError::Custom;
use crate::error::{ProgramResult, RomeEvmError};
use crate::indexer::ethereum_block_storage::ReproduceBlocks;
use crate::indexer::parsers::block_parser::EthBlock;
use crate::indexer::pg_storage::transaction_storage::TransactionStorage;
use crate::indexer::{
    self, pg_storage::PgPool, BlockParams, BlockParseResult, BlockType, PendingBlock,
    PendingBlocks, ProducedBlocks, ReceiptParams, TxResult,
};
use async_trait::async_trait;
use diesel::deserialize::FromSql;
use diesel::pg::{Pg, PgValue};
use diesel::prelude::*;
use diesel::serialize::{Output, ToSql};
use diesel::sql_types::{BigInt, Binary, Integer, Jsonb, Nullable, Text, VarChar};
use diesel::{
    self, define_sql_function, deserialize, serialize, sql_types, Connection, Queryable,
    RunQueryDsl, Selectable,
};
use ethers::abi::AbiEncode;
use ethers::prelude::{Block, Transaction, TransactionReceipt, TxHash, H256, U256, U64};
use ethers::types::Address;
use rlp::{Decodable, Rlp};
use solana_program::clock::Slot;
use std::collections::btree_map;
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::io::Write;
use std::str::FromStr;
use std::sync::Arc;

impl FromSql<Jsonb, Pg> for TxResult {
    fn from_sql(bytes: PgValue) -> deserialize::Result<Self> {
        serde_json::from_slice(bytes.as_bytes()).map_err(Into::into)
    }
}

impl ToSql<Jsonb, Pg> for TxResult {
    fn to_sql<'b>(&'b self, out: &mut Output<'b, '_, Pg>) -> serialize::Result {
        let json = serde_json::to_string(self)?;
        out.write_all(json.as_bytes())?;
        Ok(serialize::IsNull::No)
    }
}

#[derive(Queryable, Selectable, Debug)]
#[diesel(table_name = schema::pending_transactions)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct PendingTx {
    slot_number: i64,
    parent_slot: i64,
    slot_block_idx: Option<i32>,
    gas_recipient: Option<String>,
    slot_timestamp: Option<i64>,
    tx_idx: Option<i32>,
    rlp: Option<Vec<u8>>,
    tx_result: Option<serde_json::Value>,
}

#[derive(QueryableByName, Debug)]
struct ReproducedBlockRow {
    #[diesel(sql_type = BigInt)]
    slot_number: i64,

    #[diesel(sql_type = Integer)]
    slot_block_idx: i32,

    #[diesel(sql_type = Nullable<Text>)]
    gas_recipient: Option<String>,

    #[diesel(sql_type = Nullable<BigInt>)]
    slot_timestamp: Option<i64>,

    #[diesel(sql_type = Text)]
    blockhash: String,

    #[diesel(sql_type = Nullable<Text>)]
    parent_hash: Option<String>,

    #[diesel(sql_type = BigInt)]
    block_number: i64,

    #[diesel(sql_type = Text)]
    block_timestamp: String,

    #[diesel(sql_type = Integer)]
    tx_idx: i32,

    #[diesel(sql_type = Binary)]
    rlp: Vec<u8>,

    #[diesel(sql_type = Jsonb)]
    tx_result: serde_json::Value,
}

impl PendingBlock {
    fn new_from_pending_tx(pending_tx: &PendingTx) -> Self {
        PendingBlock {
            transactions: BTreeMap::new(),
            gas_recipient: pending_tx.gas_recipient.as_ref().map(|gas_recipient| {
                ethers::abi::Address::from_str(gas_recipient).unwrap_or_else(|_| {
                    panic!(
                        "DB data corrupted: Failed to decode gas_recipient from {:?}",
                        pending_tx
                    )
                })
            }),
            slot_timestamp: Some(pending_tx.slot_timestamp.unwrap_or_default()),
        }
    }

    fn new_from_reproduced_block_row(row: &ReproducedBlockRow) -> Self {
        PendingBlock {
            transactions: BTreeMap::new(),
            gas_recipient: row.gas_recipient.as_ref().map(|gas_recipient| {
                ethers::abi::Address::from_str(gas_recipient).unwrap_or_else(|_| {
                    panic!(
                        "DB data corrupted: Failed to decode gas_recipient from {:?}",
                        row
                    )
                })
            }),
            slot_timestamp: row.slot_timestamp,
        }
    }
}

impl BlockParams {
    fn to_pg_literal(&self) -> String {
        let parent_hash = match self.parent_hash {
            Some(ph) => ph.encode_hex(),
            None => "NULL".to_string(),
        };

        format!(
            "({},{},{},{})",
            self.hash.encode_hex(),
            parent_hash,
            self.number,
            self.timestamp
        )
    }

    fn new_from_reproduced_block_row(row: &ReproducedBlockRow) -> Self {
        BlockParams {
            hash: H256::from_str(&row.blockhash).unwrap_or_else(|_| {
                panic!(
                    "DB data corrupted: Failed to decode blockhash from {:?}",
                    row.blockhash
                )
            }),
            parent_hash: row.parent_hash.as_ref().map(|parent_hash| {
                H256::from_str(parent_hash).unwrap_or_else(|_| {
                    panic!(
                        "DB data corrupted: Failed to decode parent hash from {:?}",
                        parent_hash
                    )
                })
            }),
            number: U64::from(row.block_number),
            timestamp: U256::from_dec_str(&row.block_timestamp).unwrap_or_else(|_| {
                panic!(
                    "DB data corrupted: Failed to decode block timestamp from {:?}",
                    row.block_timestamp
                )
            }),
        }
    }
}

#[derive(QueryableByName, Debug)]
struct BlockHeaderRow {
    #[diesel(sql_type = BigInt)]
    slot_number: i64,

    #[diesel(sql_type = Integer)]
    slot_block_idx: i32,

    #[diesel(sql_type = Text)]
    block_gas_used: String,

    #[diesel(sql_type = Nullable<VarChar>)]
    gas_recipient: Option<String>,

    #[diesel(sql_type = Nullable<BigInt>)]
    slot_timestamp: Option<i64>,

    #[diesel(sql_type = VarChar)]
    blockhash: String,

    #[diesel(sql_type = Nullable<VarChar>)]
    parent_hash: Option<String>,

    #[diesel(sql_type = BigInt)]
    number: i64,

    #[diesel(sql_type = Text)]
    block_timestamp: String,
}

#[derive(QueryableByName, Debug)]
struct BlockTransactionHashRow {
    #[diesel(sql_type = VarChar)]
    tx_hash: String,
}

#[derive(QueryableByName, Debug)]
struct BlockTransactionRow {
    #[diesel(sql_type = VarChar)]
    tx_hash: String,

    #[diesel(sql_type = Binary)]
    rlp: Vec<u8>,

    #[diesel(sql_type = Nullable<Jsonb>)]
    receipt_params: Option<serde_json::Value>,
}

impl EthBlock {
    fn from_block_header_row(row: &BlockHeaderRow) -> ProgramResult<Self> {
        Ok(Self {
            id: (row.slot_number as Slot, row.slot_block_idx as usize),
            block_gas_used: U256::from_dec_str(&row.block_gas_used).unwrap_or_else(|_| {
                panic!(
                    "DB data corrupted: Failed to parse block_gas_used from {:?}",
                    row
                )
            }),
            transactions: vec![],
            gas_recipient: row.gas_recipient.as_ref().map(|v| {
                Address::from_str(v).unwrap_or_else(|_| {
                    panic!(
                        "DB data corrupted: Failed to parse gas_recipient from {:?}",
                        row
                    )
                })
            }),
            slot_timestamp: row.slot_timestamp,
            block_params: Some(BlockParams {
                hash: H256::from_str(&row.blockhash).unwrap_or_else(|_| {
                    panic!(
                        "DB data corrupted: Failed to parse blockhash from {:?}",
                        row
                    )
                }),
                parent_hash: row.parent_hash.as_ref().map(|v| {
                    H256::from_str(v).unwrap_or_else(|_| {
                        panic!(
                            "DB data corrupted: Failed to parse parent_hash from {:?}",
                            row
                        )
                    })
                }),
                number: U64::from(row.number),
                timestamp: U256::from_dec_str(&row.block_timestamp).unwrap_or_else(|_| {
                    panic!(
                        "DB data corrupted: Failed to parse timestamp from {:?}",
                        row
                    )
                }),
            }),
        })
    }
}

mod schema {
    use diesel;

    diesel::table! {
        pending_transactions (slot_number, slot_block_idx, tx_idx) {
            slot_number -> BigInt,
            parent_slot -> BigInt,
            slot_block_idx -> Nullable<Integer>,
            gas_recipient -> Nullable<VarChar>,
            slot_timestamp -> Nullable<BigInt>,
            tx_idx -> Nullable<Integer>,
            rlp -> Nullable<Bytea>,
            tx_result -> Nullable<Jsonb>,
        }
    }
}

define_sql_function! {
    fn get_last_produced_block(slot: BigInt) -> Nullable<VarChar>;
}

define_sql_function! {
    fn latest_eth_block() -> Nullable<BigInt>;
}

define_sql_function! {
    fn get_block_number(hash: sql_types::VarChar) -> Nullable<BigInt>;
}

define_sql_function! {
    fn get_max_slot_produced() -> Nullable<BigInt>;
}

define_sql_function! {
    fn get_slot_for_eth_block(block_number: BigInt) -> Nullable<BigInt>;
}

pub struct EthereumBlockStorage {
    pool: Arc<PgPool>,
    transaction_storage: TransactionStorage,
    chain_id: u64,
}

impl EthereumBlockStorage {
    pub fn new(pool: Arc<PgPool>, chain_id: u64) -> Self {
        Self {
            transaction_storage: TransactionStorage::new(pool.clone()),
            pool,
            chain_id,
        }
    }

    fn get_last_produced_block(&self, slot: Slot) -> ProgramResult<Option<H256>> {
        let res = diesel::select(get_last_produced_block(slot as i64))
            .get_result::<Option<String>>(&mut self.pool.get()?)?;

        if let Some(res) = res {
            Ok(Some(H256::from_str(&res).unwrap_or_else(|_| {
                panic!(
                    "DB data corrupted: Failed to decode last produced block from {:?} on slot {:?}",
                    res, slot,
                )
            })))
        } else {
            Ok(None)
        }
    }
}

#[async_trait]
impl indexer::EthereumBlockStorage for EthereumBlockStorage {
    async fn get_pending_blocks(&self) -> ProgramResult<Option<(Option<H256>, PendingBlocks)>> {
        use self::schema::pending_transactions::dsl::*;
        let mut parent_slots = BTreeMap::new();
        let mut pending_blocks = PendingBlocks::new();

        for tx in pending_transactions
            .select(PendingTx::as_select())
            .load::<PendingTx>(&mut self.pool.get()?)?
        {
            parent_slots.insert(tx.slot_number as Slot, tx.parent_slot as Slot);

            if let (
                Some(slot_block_idx_val),
                Some(tx_idx_val),
                Some(rlp_val),
                Some(tx_result_val),
            ) = (&tx.slot_block_idx, &tx.tx_idx, &tx.rlp, &tx.tx_result)
            {
                let pending_block = pending_blocks
                    .entry((tx.slot_number as Slot, *slot_block_idx_val as usize))
                    .or_insert_with(|| PendingBlock::new_from_pending_tx(&tx));

                match pending_block.transactions.entry(*tx_idx_val as usize) {
                    btree_map::Entry::Vacant(entry) => {
                        entry.insert((
                            Transaction::decode(&Rlp::new(rlp_val))?,
                            serde_json::from_value(tx_result_val.clone())?,
                        ));
                    }
                    btree_map::Entry::Occupied(_) => {
                        panic!("DB data corrupted: Already occupied {:?}", tx)
                    }
                }
            }
        }

        if pending_blocks.is_empty() {
            Ok(None)
        } else if let Some((_, parent)) = parent_slots.pop_first() {
            Ok(Some((
                self.get_last_produced_block(parent)?,
                pending_blocks,
            )))
        } else {
            Ok(Some((None, pending_blocks)))
        }
    }

    async fn reproduce_blocks(
        &self,
        from_slot: Slot,
        to_slot: Slot,
    ) -> ProgramResult<Option<(Option<H256>, ReproduceBlocks)>> {
        let rows: Vec<ReproducedBlockRow> =
            diesel::sql_query("SELECT * FROM reproduce_blocks($1, $2)")
                .bind::<BigInt, _>(from_slot as i64)
                .bind::<BigInt, _>(to_slot as i64)
                .load(&mut self.pool.get()?)?;

        let mut result = ReproduceBlocks::new();
        for row in rows {
            let (pending_block, _) = result
                .entry((row.slot_number as Slot, row.slot_block_idx as usize))
                .or_insert_with(|| {
                    (
                        PendingBlock::new_from_reproduced_block_row(&row),
                        BlockParams::new_from_reproduced_block_row(&row),
                    )
                });

            match pending_block.transactions.entry(row.tx_idx as usize) {
                btree_map::Entry::Vacant(entry) => {
                    entry.insert((
                        Transaction::decode(&Rlp::new(&row.rlp))?,
                        serde_json::from_value(row.tx_result)?,
                    ));
                }
                btree_map::Entry::Occupied(_) => {
                    panic!(
                        "DB data corrupted: Reproduce block already occupied {:?}",
                        row
                    )
                }
            }
        }

        Ok(if result.is_empty() {
            None
        } else {
            Some((
                result.first_entry().and_then(|e| e.get().1.parent_hash),
                result,
            ))
        })
    }

    async fn register_parse_results(
        &self,
        parse_results: BTreeMap<Slot, BlockParseResult>,
    ) -> ProgramResult<Option<(Option<H256>, PendingBlocks)>> {
        if !parse_results.is_empty() {
            self.transaction_storage
                .register_parse_results(&parse_results)
                .await?;

            self.pool.get()?.transaction(|conn| -> ProgramResult<()> {
                for parse_result in parse_results.values() {
                    for eth_block in parse_result.create_eth_blocks() {
                        diesel::sql_query(
                            "INSERT INTO eth_block (slot_number, slot_block_idx, block_gas_used, gas_recipient, slot_timestamp) \
                            VALUES ($1, $2, $3::NUMERIC, $4, $5) ON CONFLICT DO NOTHING;"
                        )
                            .bind::<BigInt, _>(eth_block.id.0 as i64)
                            .bind::<Integer, _>(eth_block.id.1 as i32)
                            .bind::<Text, _>(eth_block.block_gas_used.to_string())
                            .bind::<Nullable<VarChar>, _>(eth_block.gas_recipient.map(|v| format!("0x{:x}", v)))
                            .bind::<Nullable<BigInt>, _>(eth_block.slot_timestamp)
                            .execute(conn)?;

                        for (tx_idx, tx) in eth_block.transactions.iter().enumerate() {
                            diesel::sql_query(
                                "INSERT INTO eth_block_txs (slot_number, slot_block_idx, tx_hash, tx_idx) \
                                VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING;"
                            )
                                .bind::<BigInt, _>(eth_block.id.0 as i64)
                                .bind::<Integer, _>(eth_block.id.1 as i32)
                                .bind::<VarChar, _>(tx.encode_hex())
                                .bind::<Integer, _>(tx_idx as i32)
                                .execute(conn)?;
                        }
                    }
                }

                Ok(())
            })?;
        }

        self.get_pending_blocks().await
    }

    async fn latest_block(&self) -> ProgramResult<Option<U64>> {
        Ok(diesel::select(latest_eth_block())
            .get_result::<Option<i64>>(&mut self.pool.get()?)?
            .map(|v| v.into()))
    }

    async fn get_block_number(&self, hash: &H256) -> ProgramResult<Option<U64>> {
        Ok(diesel::select(get_block_number(hash.encode_hex()))
            .get_result::<Option<i64>>(&mut self.pool.get()?)?
            .map(|v| v.into()))
    }

    async fn get_block_by_number(
        &self,
        number: U64,
        full_transactions: bool,
    ) -> ProgramResult<Option<BlockType>> {
        let block_header: Vec<BlockHeaderRow> =
            diesel::sql_query("SELECT * FROM get_block_header($1)")
                .bind::<BigInt, _>(number.as_u64() as i64)
                .load(&mut self.pool.get()?)?;

        if block_header.is_empty() {
            return Ok(None);
        }

        if block_header.len() > 1 {
            return Err(Custom(format!(
                "More than one block with number {:?}",
                number
            )));
        }

        let block_header = EthBlock::from_block_header_row(block_header.first().unwrap())?;

        Ok(Some(if full_transactions {
            let rows: Vec<BlockTransactionRow> =
                diesel::sql_query("SELECT * FROM get_block_transactions($1)")
                    .bind::<BigInt, _>(number.as_u64() as i64)
                    .load(&mut self.pool.get()?)?;

            let mut txs = Vec::with_capacity(rows.len());
            for row in rows {
                let mut tx = Transaction::decode(&Rlp::new(&row.rlp)).map_err(|_| {
                    Custom(format!(
                        "Failed to decode transaction {} from rlp",
                        row.tx_hash
                    ))
                })?;

                if let Some(receipt_params) = row.receipt_params {
                    let receipt_params: ReceiptParams = serde_json::from_value(receipt_params)?;
                    tx.block_hash = Some(receipt_params.blockhash);
                    tx.block_number = Some(receipt_params.block_number);
                    tx.transaction_index = Some(U64::from(receipt_params.tx_index));
                };

                txs.push(tx);
            }

            BlockType::BlockWithTransactions(Block::<Transaction> {
                transactions: txs,
                ..block_header.get_block_base()
            })
        } else {
            let txs: Vec<BlockTransactionHashRow> =
                diesel::sql_query("SELECT * FROM get_block_transaction_hashes($1)")
                    .bind::<BigInt, _>(number.as_u64() as i64)
                    .load(&mut self.pool.get()?)?;

            let txs = txs
                    .iter()
                    .map(|tx| {
                        H256::from_str(&tx.tx_hash).unwrap_or_else(|_| {
                            panic!(
                                "DB data corrupted: Failed to parse tx hash from {:?} on block {number}",
                                tx.tx_hash
                            )
                        })
                    })
                    .collect::<Vec<_>>();

            BlockType::BlockWithHashes(Block::<TxHash> {
                transactions: txs,
                ..block_header.get_block_base()
            })
        }))
    }

    async fn blocks_produced(
        &self,
        pending_blocks: PendingBlocks,
        produced_blocks: ProducedBlocks,
    ) -> ProgramResult<()> {
        self.pool
            .get()?
            .transaction::<(), RomeEvmError, _>(|conn| {
                for block_id in pending_blocks.keys() {
                    if let Some(block_params) = produced_blocks.get(block_id) {
                        diesel::sql_query("CALL block_produced($1, $2, $3)")
                            .bind::<BigInt, _>(block_id.0 as i64)
                            .bind::<Integer, _>(block_id.1 as i32)
                            .bind::<Text, _>(block_params.to_pg_literal())
                            .execute(conn)?;

                        tracing::info!("new block registered {:?}:{:?}", block_id, block_params);
                    }
                }

                Ok(())
            })?;

        self.transaction_storage
            .blocks_produced(pending_blocks, produced_blocks)
            .await?;

        Ok(())
    }

    fn chain(&self) -> u64 {
        self.chain_id
    }

    async fn get_max_slot_produced(&self) -> ProgramResult<Option<Slot>> {
        Ok(diesel::select(get_max_slot_produced())
            .get_result::<Option<i64>>(&mut self.pool.get()?)?
            .map(|v| v as Slot))
    }

    async fn retain_from_slot(&self, _from_slot: Slot) -> ProgramResult<()> {
        Ok(())
    }

    async fn get_transaction_receipt(
        &self,
        tx_hash: &H256,
    ) -> ProgramResult<Option<TransactionReceipt>> {
        self.transaction_storage
            .get_transaction_receipt(tx_hash)
            .await
    }

    async fn get_transaction(&self, tx_hash: &H256) -> ProgramResult<Option<Transaction>> {
        self.transaction_storage.get_transaction(tx_hash).await
    }

    async fn get_slot_for_eth_block(&self, block_number: U64) -> ProgramResult<Option<Slot>> {
        Ok(
            diesel::select(get_slot_for_eth_block(block_number.as_u64() as i64))
                .get_result::<Option<i64>>(&mut self.pool.get()?)?
                .map(|res| res as Slot),
        )
    }
}
