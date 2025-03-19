use crate::indexer::block_producers::{
    EngineAPIBlockProducer, EngineAPIBlockProducerConfig, SingleStateBlockProducer,
    SingleStateBlockProducerConfig,
};
use crate::indexer::parsers::block_parser::BlockParseMode;
use crate::indexer::{
    inmemory, pg_storage, BlockParser, BlockProducer, EthereumBlockStorage, ProgramResult,
    SolanaBlockStorage,
};
use serde::{Deserialize, Deserializer};
use solana_program::pubkey::Pubkey;
use std::sync::Arc;
use tokio::sync::RwLock;

fn deserialize_pubkey_from_string<'de, D>(deserializer: D) -> Result<Pubkey, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    s.parse::<Pubkey>().map_err(serde::de::Error::custom)
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct BlockParserConfig {
    #[serde(deserialize_with = "deserialize_pubkey_from_string")]
    pub program_id: Pubkey,
    pub chain_id: u64,
    pub parse_mode: BlockParseMode,
}

impl BlockParserConfig {
    pub fn init(
        &self,
        solana_block_storage: Arc<dyn SolanaBlockStorage>,
    ) -> Arc<RwLock<BlockParser>> {
        Arc::new(RwLock::new(BlockParser::new(
            solana_block_storage,
            self.program_id,
            self.chain_id,
            self.parse_mode,
        )))
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BlockProducerConfig {
    EngineApi(EngineAPIBlockProducerConfig),
    SingleState(SingleStateBlockProducerConfig),
}

impl BlockProducerConfig {
    pub fn init(&self) -> ProgramResult<Arc<dyn BlockProducer>> {
        Ok(match self {
            BlockProducerConfig::EngineApi(config) => {
                Arc::new(EngineAPIBlockProducer::try_from(config)?)
            }
            BlockProducerConfig::SingleState(config) => {
                Arc::new(SingleStateBlockProducer::try_from(config)?)
            }
        })
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EthereumStorageConfig {
    PgStorage {
        connection: pg_storage::config::PgPoolConfig,
    },
    InMemory,
}

impl EthereumStorageConfig {
    pub fn init(&self) -> ProgramResult<Arc<dyn EthereumBlockStorage>> {
        Ok(match self {
            EthereumStorageConfig::PgStorage { connection } => Arc::new(
                pg_storage::EthereumBlockStorage::new(Arc::new(connection.init()?)),
            ),
            EthereumStorageConfig::InMemory => Arc::new(inmemory::EthereumBlockStorage),
        })
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SolanaStorageConfig {
    PgStorage {
        connection: pg_storage::config::PgPoolConfig,
    },
}

impl SolanaStorageConfig {
    pub fn init(&self) -> ProgramResult<Arc<dyn SolanaBlockStorage>> {
        match self {
            SolanaStorageConfig::PgStorage { connection } => Ok(Arc::new(
                pg_storage::SolanaBlockStorage::new(connection.init()?),
            )),
        }
    }
}
