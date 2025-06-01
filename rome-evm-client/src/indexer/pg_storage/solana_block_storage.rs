use crate::error::ProgramResult;
use crate::indexer::{self, pg_storage::PgPool};
use async_trait::async_trait;
use diesel::{self, define_sql_function, sql_types, Connection, QueryableByName, RunQueryDsl};
use solana_program::clock::Slot;
use solana_transaction_status::UiConfirmedBlock;
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct SolanaBlockStorage {
    pool: PgPool,
}

impl SolanaBlockStorage {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

define_sql_function! {
    fn get_block(slot_number: sql_types::BigInt) -> Nullable<Bytea>;
}

define_sql_function! {
    fn get_last_slot() -> Nullable<BigInt>;
}

#[derive(QueryableByName, Debug)]
struct FinalizedSlotRow {
    #[diesel(sql_type = sql_types::BigInt)]
    slot_number: i64,

    #[diesel(sql_type = sql_types::Bytea)]
    block_data: Vec<u8>,
}

#[async_trait]
impl indexer::SolanaBlockStorage for SolanaBlockStorage {
    #[tracing::instrument(name = "pg_storage::store_blocks", skip(self))]
    async fn store_blocks(
        &self,
        blocks: BTreeMap<Slot, Arc<UiConfirmedBlock>>,
        finalized_slot: Slot,
    ) -> ProgramResult<()> {
        self.pool.get()?.transaction(|conn| {
            for (slot_number, block) in blocks {
                assert!(block.block_time.is_some());

                let slot_status = if slot_number <= finalized_slot {
                    "Finalized"
                } else {
                    "Confirmed"
                };

                diesel::sql_query(
                    "CALL set_block_with_status2($1, $2, $3, $4::slotstatus, $5, $6);",
                )
                .bind::<sql_types::BigInt, _>(slot_number as i64)
                .bind::<sql_types::BigInt, _>(block.parent_slot as i64)
                .bind::<sql_types::Bytea, _>(serde_json::to_vec(&block)?)
                .bind::<sql_types::Text, _>(slot_status)
                .bind::<sql_types::Text, _>(block.blockhash.clone())
                .bind::<sql_types::BigInt, _>(block.block_time.unwrap())
                .execute(conn)?;
            }

            Ok(())
        })
    }

    #[tracing::instrument(name = "pg_storage::update_finalized_blocks", skip(self))]
    async fn update_finalized_blocks(
        &self,
        blocks: BTreeMap<Slot, Arc<UiConfirmedBlock>>,
    ) -> ProgramResult<()> {
        self.pool.get()?.transaction(|conn| {
            for (slot_number, block) in blocks {
                diesel::sql_query("CALL update_finalized_block2($1, $2, $3, $4, $5);")
                    .bind::<sql_types::BigInt, _>(slot_number as i64)
                    .bind::<sql_types::BigInt, _>(block.parent_slot as i64)
                    .bind::<sql_types::Bytea, _>(serde_json::to_vec(&block)?)
                    .bind::<sql_types::Text, _>(block.blockhash.clone())
                    .bind::<sql_types::Text, _>(block.previous_blockhash.clone())
                    .execute(conn)?;
            }

            Ok(())
        })
    }

    #[tracing::instrument(name = "pg_storage::get_block", skip(self), fields(slot = ?slot_number))]
    async fn get_block(&self, slot_number: Slot) -> ProgramResult<Option<Arc<UiConfirmedBlock>>> {
        Ok(
            if let Some(value) = diesel::select(get_block(slot_number as i64))
                .get_result::<Option<Vec<u8>>>(&mut self.pool.get()?)?
            {
                Some(Arc::new(serde_json::from_slice::<UiConfirmedBlock>(
                    &value,
                )?))
            } else {
                None
            },
        )
    }

    async fn retain_from_slot(&self, _from_slot: Slot) -> ProgramResult<()> {
        Ok(())
    }

    #[tracing::instrument(name = "pg_storage::get_last_slot", skip(self))]
    async fn get_last_slot(&self) -> ProgramResult<Option<Slot>> {
        Ok(diesel::select(get_last_slot())
            .get_result::<Option<i64>>(&mut self.pool.get()?)?
            .map(|v| v as Slot))
    }

    #[tracing::instrument(name = "pg_storage::set_finalized_slot", skip(self), fields(slot = ?slot))]
    async fn set_finalized_slot(
        &self,
        slot: Slot,
    ) -> ProgramResult<BTreeMap<Slot, UiConfirmedBlock>> {
        let rows: Vec<FinalizedSlotRow> =
            diesel::sql_query("SELECT * FROM set_finalized_slot($1);")
                .bind::<sql_types::BigInt, _>(slot as i64)
                .load(&mut self.pool.get()?)?;

        let mut result = BTreeMap::new();
        for row in rows {
            result.insert(
                row.slot_number as Slot,
                serde_json::from_slice::<UiConfirmedBlock>(&row.block_data)?,
            );
        }

        Ok(result)
    }
}
