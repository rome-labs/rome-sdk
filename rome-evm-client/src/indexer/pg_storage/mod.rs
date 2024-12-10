use diesel::r2d2::ConnectionManager;
use diesel::PgConnection;
use r2d2::Pool;
use serde::{Deserialize, Serialize};
use std::time;

mod solana_block_storage;
mod types;

pub type PgPool = Pool<ConnectionManager<PgConnection>>;
use crate::error::ProgramResult;
pub use solana_block_storage::SolanaBlockStorage;

#[derive(Debug, Serialize, Deserialize)]
pub struct PgPoolConfig {
    database_url: String,
    max_connections: Option<u32>,
    connection_timeout_sec: Option<u64>,
}

const DEFAULT_MAX_CONNECTIONS: u32 = 32;
const DEFAULT_MAX_TIMEOUT_SEC: u64 = 60;

impl PgPoolConfig {
    pub fn create_pool(&self) -> ProgramResult<PgPool> {
        let manager = ConnectionManager::<PgConnection>::new(self.database_url.clone());
        Ok(Pool::builder()
            .max_size(self.max_connections.unwrap_or(DEFAULT_MAX_CONNECTIONS))
            .connection_timeout(time::Duration::new(
                self.connection_timeout_sec
                    .unwrap_or(DEFAULT_MAX_TIMEOUT_SEC),
                0,
            ))
            .build(manager)?)
    }
}
