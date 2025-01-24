use crate::error::ProgramResult;
use crate::indexer::{self, pg_storage::PgPool};
use async_trait::async_trait;
use diesel::{self, define_sql_function, sql_types, Connection, RunQueryDsl};
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

#[async_trait]
impl indexer::SolanaBlockStorage for SolanaBlockStorage {
    async fn store_blocks(
        &self,
        blocks: BTreeMap<Slot, Arc<UiConfirmedBlock>>,
    ) -> ProgramResult<()> {
        self.pool.get()?.transaction(|conn| {
            for (slot_number, block) in blocks {
                diesel::sql_query("CALL set_block($1, $2, $3);")
                    .bind::<sql_types::BigInt, _>(slot_number as i64)
                    .bind::<sql_types::BigInt, _>(block.parent_slot as i64)
                    .bind::<sql_types::Bytea, _>(serde_json::to_vec(&block)?)
                    .execute(conn)?;
            }

            Ok(())
        })
    }

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

    async fn get_last_slot(&self) -> ProgramResult<Option<Slot>> {
        Ok(diesel::select(get_last_slot())
            .get_result::<Option<i64>>(&mut self.pool.get()?)?
            .map(|v| v as Slot))
    }
}
