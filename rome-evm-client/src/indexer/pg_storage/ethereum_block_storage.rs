use crate::error::RomeEvmError::Custom;
use crate::error::{ProgramResult, RomeEvmError};
use crate::indexer::ethereum_block_storage::{
    EthBlockId, FinalizedBlock, ProducerParams, ReproduceParams,
};
use crate::indexer::parsers::block_parser::EthBlock;
use crate::indexer::pending_blocks::{PendingBlocks, PendingL1Block, PendingL2Block};
use crate::indexer::pg_storage::transaction_storage::TransactionStorage;
use crate::indexer::pg_storage::types::{ReceiptParams, SlotStatus};
use crate::indexer::produced_blocks::{BlockParams, ProducedBlocks};
use crate::indexer::{pg_storage::PgPool, BlockParseResult, BlockType, TxResult};
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
use solana_sdk::bs58;
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
#[diesel(table_name = schema::pending_transactions_with_slot_statuses2)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct PendingTx {
    slot_number: i64,
    slot_status: SlotStatus,
    slot_blockhash: String,
    slot_timestamp: Option<i64>,
    slot_block_idx: i32,
    gas_recipient: Option<String>,
    eth_blockhash: Option<String>,
    tx_idx: i32,
    rlp: Vec<u8>,
    tx_result: serde_json::Value,
}

#[derive(QueryableByName, Debug)]
struct ReproducedBlockRow {
    #[diesel(sql_type = BigInt)]
    slot_number: i64,

    #[diesel(sql_type = Nullable<BigInt>)]
    slot_timestamp: Option<i64>,

    #[diesel(sql_type = Integer)]
    slot_block_idx: i32,

    #[diesel(sql_type = Nullable<Text>)]
    gas_recipient: Option<String>,

    #[diesel(sql_type = Text)]
    blockhash: String,

    #[diesel(sql_type = Nullable<Text>)]
    parent_hash: Option<String>,

    #[diesel(sql_type = BigInt)]
    block_number: i64,

    #[diesel(sql_type = Integer)]
    tx_idx: i32,

    #[diesel(sql_type = Binary)]
    rlp: Vec<u8>,

    #[diesel(sql_type = Jsonb)]
    tx_result: serde_json::Value,
}

impl PendingL1Block {
    fn new_from_pending_tx(tx: &PendingTx) -> Self {
        Self {
            blockhash: H256::from_slice(
                &bs58::decode(&tx.slot_blockhash)
                    .into_vec()
                    .unwrap_or_else(|_| {
                        panic!(
                            "Failed to decode L1 blockhash for slot {:?}",
                            tx.slot_number
                        )
                    }),
            ),
            timestamp: tx.slot_timestamp.map(U256::from).unwrap_or_else(|| {
                panic!("Slot timestamp is not set for slot {:?}", tx.slot_number)
            }),
            l2_blocks: BTreeMap::new(),
        }
    }

    fn new_from_reproduced_block_row(row: &ReproducedBlockRow) -> Self {
        Self {
            blockhash: H256::from_str(&row.blockhash).unwrap_or_else(|_| {
                panic!(
                    "Failed to decode L1 blockhash for slot {:?}",
                    row.slot_number
                )
            }),
            timestamp: row.slot_timestamp.map(U256::from).unwrap_or_else(|| {
                panic!("Slot timestamp is not set for slot {:?}", row.slot_number)
            }),
            l2_blocks: BTreeMap::new(),
        }
    }
}

impl PendingL2Block {
    fn new_with_gas_recipient(gas_recipient: &Option<String>) -> Self {
        Self {
            transactions: BTreeMap::new(),
            gas_recipient: gas_recipient.as_ref().map(|gas_recipient| {
                ethers::abi::Address::from_str(gas_recipient).unwrap_or_else(|_| {
                    panic!(
                        "Failed to decode gas_recipient for L2 block. gas_recipient = {:?}",
                        gas_recipient
                    )
                })
            }),
        }
    }

    fn new_from_pending_tx(pending_tx: &PendingTx) -> Self {
        Self::new_with_gas_recipient(&pending_tx.gas_recipient)
    }

    fn new_from_reproduced_block_row(row: &ReproducedBlockRow) -> Self {
        Self::new_with_gas_recipient(&row.gas_recipient)
    }
}

impl PendingBlocks {
    fn append_pending_tx(&mut self, tx: &PendingTx) -> ProgramResult<()> {
        match self
            .0
            .entry(tx.slot_number as Slot)
            .or_insert_with(|| PendingL1Block::new_from_pending_tx(tx))
            .l2_blocks
            .entry(tx.slot_block_idx as usize)
            .or_insert_with(|| PendingL2Block::new_from_pending_tx(tx))
            .transactions
            .entry(tx.tx_idx as usize)
        {
            btree_map::Entry::Vacant(entry) => {
                entry.insert((
                    Transaction::decode(&Rlp::new(&tx.rlp))?,
                    serde_json::from_value(tx.tx_result.clone())?,
                ));
            }
            btree_map::Entry::Occupied(_) => {
                panic!("PendingBlock tx already occupied {:?}", tx)
            }
        }

        Ok(())
    }

    fn append_reproduced_block_row(&mut self, row: &ReproducedBlockRow) -> ProgramResult<()> {
        match self
            .0
            .entry(row.slot_number as Slot)
            .or_insert_with(|| PendingL1Block::new_from_reproduced_block_row(row))
            .l2_blocks
            .entry(row.slot_block_idx as usize)
            .or_insert_with(|| PendingL2Block::new_from_reproduced_block_row(row))
            .transactions
            .entry(row.tx_idx as usize)
        {
            btree_map::Entry::Vacant(entry) => {
                entry.insert((
                    Transaction::decode(&Rlp::new(&row.rlp))?,
                    serde_json::from_value(row.tx_result.clone())?,
                ));
            }
            btree_map::Entry::Occupied(_) => {
                panic!(
                    "DB data corrupted: Reproduce block already occupied {:?}",
                    row
                )
            }
        }

        Ok(())
    }
}

impl BlockParams {
    pub(crate) fn to_pg_literal(&self) -> String {
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
                    row
                )
            }),
            parent_hash: row.parent_hash.as_ref().map(|parent_hash| {
                H256::from_str(parent_hash).unwrap_or_else(|_| {
                    panic!(
                        "DB data corrupted: Failed to decode parent hash from {:?}",
                        row
                    )
                })
            }),
            number: U64::from(row.block_number),
            timestamp: row.slot_timestamp.map(U256::from).unwrap_or_else(|| {
                panic!(
                    "DB data corrupted: slot timestamp is not set for slot {:?}",
                    row
                )
            }),
        }
    }
}

impl ProducedBlocks {
    fn append_from_reproduced_block_row(&mut self, row: ReproducedBlockRow) {
        self.0
            .entry(row.slot_number as Slot)
            .or_default()
            .entry(row.block_number as usize)
            .or_insert_with(|| BlockParams::new_from_reproduced_block_row(&row));
    }
}

#[derive(QueryableByName, Debug)]
pub(crate) struct BlockHeaderRow {
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
}

#[derive(QueryableByName, Debug)]
pub(crate) struct BlockTransactionHashRow {
    #[diesel(sql_type = VarChar)]
    pub(crate) tx_hash: String,
}

#[derive(QueryableByName, Debug)]
pub(crate) struct BlockTransactionRow {
    #[diesel(sql_type = VarChar)]
    pub(crate) tx_hash: String,

    #[diesel(sql_type = Binary)]
    pub(crate) rlp: Vec<u8>,

    #[diesel(sql_type = Nullable<Jsonb>)]
    pub(crate) receipt_params: Option<serde_json::Value>,
}

impl EthBlock {
    pub(crate) fn from_block_header_row(row: &BlockHeaderRow) -> ProgramResult<Self> {
        Ok(Self {
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
                timestamp: row.slot_timestamp.map(U256::from).unwrap_or_else(|| {
                    panic!(
                        "DB data corrupted: slot timestamp is not set for block {:?}",
                        row.number,
                    )
                }),
            }),
        })
    }
}

mod schema {
    use diesel;

    diesel::table! {
        pending_transactions_with_slot_statuses2 (slot_number, slot_block_idx, tx_idx) {
            slot_number -> BigInt,
            slot_status -> crate::indexer::pg_storage::types::SlotStatusMapping,
            slot_blockhash -> Text,
            slot_timestamp -> Nullable<BigInt>,
            slot_block_idx -> Integer,
            gas_recipient -> Nullable<VarChar>,
            eth_blockhash -> Nullable<VarChar>,
            tx_idx -> Integer,
            rlp -> Bytea,
            tx_result -> Jsonb,
        }
    }
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
}

impl EthBlock {
    pub fn store_rollup_header_into_db(
        &self,
        slot_number: Slot,
        block_idx: usize,
        conn: &mut PgConnection,
    ) -> ProgramResult<()> {
        diesel::sql_query(
            "INSERT INTO eth_block (slot_number, slot_block_idx, block_gas_used, gas_recipient, slot_timestamp) \
                            VALUES ($1, $2, $3::NUMERIC, $4, $5) ON CONFLICT DO NOTHING;"
        )
            .bind::<BigInt, _>(slot_number as i64)
            .bind::<Integer, _>(block_idx as i32)
            .bind::<Text, _>(self.block_gas_used.to_string())
            .bind::<Nullable<VarChar>, _>(self.gas_recipient.map(|v| format!("0x{:x}", v)))
            .bind::<Nullable<BigInt>, _>(self.slot_timestamp)
            .execute(conn)?;

        Ok(())
    }

    pub fn store_single_state_header_into_db(
        &self,
        slot_number: Slot,
        block_idx: usize,
        conn: &mut PgConnection,
        parse_result: &BlockParseResult,
    ) -> ProgramResult<()> {
        diesel::sql_query(
            "INSERT INTO eth_block (slot_number, slot_block_idx, block_gas_used, gas_recipient, slot_timestamp, params) \
                            VALUES ($1, $2, $3::NUMERIC, $4, $5, $6::blockparams) ON CONFLICT DO NOTHING;"
        )
            .bind::<BigInt, _>(slot_number as i64)
            .bind::<Integer, _>(block_idx as i32)
            .bind::<Text, _>(self.block_gas_used.to_string())
            .bind::<Nullable<VarChar>, _>(self.gas_recipient.map(|v| format!("0x{:x}", v)))
            .bind::<Nullable<BigInt>, _>(self.slot_timestamp)
            .bind::<Text, _>(BlockParams {
                hash: H256::from_slice(
                    &bs58::decode(&parse_result.solana_blockhash)
                        .into_vec()
                        .expect("Failed to decode solana blockhash")
                ),
                parent_hash: Some(H256::from_slice(
                    &bs58::decode(&parse_result.parent_solana_blockhash)
                        .into_vec()
                        .expect("Failed to decode solana parent blockhash")
                )),
                number: U64::from(parse_result.slot_number),
                timestamp: parse_result.timestamp.map(U256::from).unwrap_or_default(),
            }.to_pg_literal())
            .execute(conn)?;

        Ok(())
    }

    pub fn store_txs_into_db(
        &self,
        slot_number: Slot,
        block_idx: usize,
        conn: &mut PgConnection,
    ) -> ProgramResult<()> {
        for (tx_idx, tx) in self.transactions.iter().enumerate() {
            diesel::sql_query(
                "INSERT INTO eth_block_txs (slot_number, slot_block_idx, tx_hash, tx_idx) \
                                VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING;",
            )
            .bind::<BigInt, _>(slot_number as i64)
            .bind::<Integer, _>(block_idx as i32)
            .bind::<VarChar, _>(tx.encode_hex())
            .bind::<Integer, _>(tx_idx as i32)
            .execute(conn)?;
        }

        Ok(())
    }
}

impl EthereumBlockStorage {
    pub fn new(pool: Arc<PgPool>) -> Self {
        Self {
            transaction_storage: TransactionStorage::new(pool.clone()),
            pool,
        }
    }
}

#[async_trait]
impl crate::indexer::EthereumBlockStorage for EthereumBlockStorage {
    async fn get_pending_blocks(&self) -> ProgramResult<Option<ProducerParams>> {
        use self::schema::pending_transactions_with_slot_statuses2::dsl::*;
        let mut finalized_blocks = BTreeMap::new();
        let mut produced_blocks = BTreeMap::new();
        let mut pending_blocks = PendingBlocks::default();

        for tx in pending_transactions_with_slot_statuses2
            .select(PendingTx::as_select())
            .load::<PendingTx>(&mut self.pool.get()?)?
        {
            let block_id: EthBlockId = (tx.slot_number as Slot, tx.slot_block_idx as usize);
            if tx.slot_status == SlotStatus::Finalized {
                finalized_blocks
                    .entry(block_id)
                    .or_insert_with(|| tx.eth_blockhash
                        .clone()
                        .map(|v| H256::from_str(&v)
                            .unwrap_or_else(
                                |_| panic!(
                                    "DB data corrupted: Failed to decode finalized blockhash {:?} for block {:?}",
                                    v, block_id
                                )
                            )
                        )
                    );
            }

            if let Some(blockhash_val) = tx.eth_blockhash {
                produced_blocks
                    .entry(block_id)
                    .or_insert_with(|| H256::from_str(&blockhash_val).unwrap_or_else(|_| {
                        panic!(
                            "DB data corrupted: Failed to decode produced blockhash {:?} for block {:?}",
                            blockhash_val, block_id,
                        )
                    }));
            } else {
                pending_blocks.append_pending_tx(&tx)?;
            }
        }

        if pending_blocks.is_empty() {
            Ok(None)
        } else {
            Ok(Some(ProducerParams {
                pending_blocks,
                parent_hash: produced_blocks.last_entry().map(|e| *e.get()),
                finalized_block: finalized_blocks.last_entry().map(|e| {
                    if let Some(blockhash_val) = e.get() {
                        FinalizedBlock::Produced(*blockhash_val)
                    } else {
                        FinalizedBlock::Pending(*e.key())
                    }
                }),
            }))
        }
    }

    async fn reproduce_blocks(
        &self,
        from_slot: Slot,
        to_slot: Slot,
    ) -> ProgramResult<Option<ReproduceParams>> {
        let rows: Vec<ReproducedBlockRow> =
            diesel::sql_query("SELECT * FROM reproduce_blocks2($1, $2)")
                .bind::<BigInt, _>(from_slot as i64)
                .bind::<BigInt, _>(to_slot as i64)
                .load(&mut self.pool.get()?)?;

        let mut pending_blocks = PendingBlocks::default();
        let mut expected_results = ProducedBlocks::default();

        for row in rows {
            pending_blocks.append_reproduced_block_row(&row)?;
            expected_results.append_from_reproduced_block_row(row);
        }

        Ok(if pending_blocks.is_empty() {
            None
        } else {
            Some(ReproduceParams {
                producer_params: ProducerParams {
                    parent_hash: expected_results.first().and_then(|v| v.parent_hash),
                    pending_blocks,
                    finalized_block: None,
                },
                expected_results,
            })
        })
    }

    async fn register_parse_results(
        &self,
        parse_results: BTreeMap<Slot, BlockParseResult>,
    ) -> ProgramResult<()> {
        if !parse_results.is_empty() {
            self.transaction_storage
                .register_parse_results(&parse_results)
                .await?;

            self.pool.get()?.transaction(|conn| -> ProgramResult<()> {
                for (slot_number, parse_result) in &parse_results {
                    for (block_idx, eth_block) in
                        parse_result.create_eth_blocks().iter().enumerate()
                    {
                        eth_block.store_rollup_header_into_db(*slot_number, block_idx, conn)?;
                        eth_block.store_txs_into_db(*slot_number, block_idx, conn)?;
                    }
                }

                Ok(())
            })?;
        }

        Ok(())
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
            diesel::sql_query("SELECT * FROM get_block_header2($1)")
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
        producer_params: &ProducerParams,
        produced_blocks: ProducedBlocks,
    ) -> ProgramResult<()> {
        self.pool
            .get()?
            .transaction::<(), RomeEvmError, _>(|conn| {
                for (slot_number, block_idx, block_params) in produced_blocks.iter() {
                    if producer_params
                        .pending_blocks
                        .contains(slot_number, block_idx)
                    {
                        diesel::sql_query("CALL block_produced($1, $2, $3)")
                            .bind::<BigInt, _>(*slot_number as i64)
                            .bind::<Integer, _>(*block_idx as i32)
                            .bind::<Text, _>(block_params.to_pg_literal())
                            .execute(conn)?;

                        tracing::info!(
                            "new block registered {:?}:{:?}",
                            (slot_number, block_idx),
                            block_params
                        );
                    }
                }

                Ok(())
            })?;

        self.transaction_storage
            .blocks_produced(&producer_params.pending_blocks, produced_blocks)
            .await?;

        Ok(())
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

    async fn clean_from_slot(&self, from_slot: Slot) -> ProgramResult<()> {
        diesel::sql_query("CALL clean_from_slot($1)")
            .bind::<BigInt, _>(from_slot as i64)
            .execute(&mut self.pool.get()?)?;

        Ok(())
    }
}
