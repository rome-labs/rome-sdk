use crate::error::ProgramResult;
use crate::indexer::pg_storage::PgPool;
use diesel::r2d2::ConnectionManager;
use diesel::PgConnection;
use jsonrpsee_core::Serialize;
use r2d2::Pool;
use serde::Deserialize;
use std::time;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PgPoolConfig {
    database_url: String,
    max_connections: Option<u32>,
    connection_timeout_sec: Option<u64>,
}

const DEFAULT_MAX_CONNECTIONS: u32 = 32;
const DEFAULT_MAX_TIMEOUT_SEC: u64 = 60;

impl PgPoolConfig {
    pub fn init(&self) -> ProgramResult<PgPool> {
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
