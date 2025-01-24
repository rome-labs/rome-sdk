use crate::indexer::pg_storage;

#[derive(Clone, serde::Serialize, serde::Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EthereumStorageConfig {
    PgStorage(pg_storage::config::PgPoolConfig),
    InMemoryStorage,
}
