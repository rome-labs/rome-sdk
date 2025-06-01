use crate::indexer::block_producers::{
    EngineAPIBlockProducer, EngineAPIBlockProducerConfig, SingleStateBlockProducer,
    SingleStateBlockProducerConfig,
};
use crate::indexer::parsers::block_parser::BlockParseMode;
use crate::indexer::relayer::RelayerSolanaBlockStorage;
use crate::indexer::{
    inmemory, pg_storage, BlockParser, BlockProducer, EthereumBlockStorage, ProgramResult,
    SolanaBlockStorage,
};
use crate::indexer::{MultiplexedSolanaClient, SolanaBlockLoader};
use serde::{Deserialize, Deserializer};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_program::pubkey::Pubkey;
use solana_sdk::clock::Slot;
use solana_sdk::commitment_config::CommitmentLevel;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

fn deserialize_pubkey_from_string<'de, D>(deserializer: D) -> Result<Pubkey, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    s.parse::<Pubkey>().map_err(serde::de::Error::custom)
}

fn deserialize_optional_pubkey_from_string<'de, D>(
    deserializer: D,
) -> Result<Option<Pubkey>, D::Error>
where
    D: Deserializer<'de>,
{
    // Разрешаем как null, так и строку.
    let maybe_str = Option::<String>::deserialize(deserializer)?;

    match maybe_str {
        // null → None
        None => Ok(None),

        // "" или строка из одних пробелов → None
        Some(ref s) if s.trim().is_empty() => Ok(None),

        // непустая строка → пытаемся распарсить Pubkey
        Some(s) => s
            .parse::<Pubkey>()
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct BlockParserConfig {
    #[serde(
        default,
        deserialize_with = "deserialize_optional_pubkey_from_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub program_id: Option<Pubkey>,
    pub chain_id: u64,
    pub parse_mode: BlockParseMode,
    pub enable_deposit_slot: Option<Slot>,
}

impl BlockParserConfig {
    pub fn init(
        &self,
        solana_block_storage: Arc<dyn SolanaBlockStorage>,
        program_id: Option<Pubkey>,
    ) -> Arc<RwLock<BlockParser>> {
        tracing::info!("Initializing block parser...");
        Arc::new(RwLock::new(BlockParser {
            solana_block_storage,
            program_id: program_id.unwrap_or(self.program_id.expect(
                "program_id is required either in block_loader or in block_parser config section",
            )),
            chain_id: self.chain_id,
            parse_mode: self.parse_mode,
            enable_deposit_slot: self.enable_deposit_slot,
        }))
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
        tracing::info!("Initializing block producer...");
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
pub struct MultiplexedSolanaClientConfig {
    pub providers: Vec<String>,
}

impl MultiplexedSolanaClientConfig {
    pub fn init(&self) -> Arc<MultiplexedSolanaClient> {
        tracing::info!("Initializing multiplexed solana client...");
        Arc::new(MultiplexedSolanaClient {
            providers: self
                .providers
                .iter()
                .map(|url| Arc::new(RpcClient::new(url.clone())))
                .collect::<Vec<_>>(),
        })
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub struct SolanaBlockLoaderConfig {
    #[serde(deserialize_with = "deserialize_pubkey_from_string")]
    pub program_id: Pubkey,

    pub batch_size: Option<usize>,
    pub block_retries: Option<usize>,
    pub tx_retries: Option<usize>,
    pub retry_int_sec: Option<u64>,
    pub commitment: CommitmentLevel,
    pub client: MultiplexedSolanaClientConfig,
}

impl SolanaBlockLoaderConfig {
    const DEFAULT_BATCH_SIZE: usize = 128;
    const DEFAULT_BLOCK_RETRIES: usize = 100;
    const DEFAULT_TX_RETRIES: usize = 100;
    const DEFAULT_RETRY_INT_SEC: u64 = 1;

    pub fn init(&self, solana_block_storage: Arc<dyn SolanaBlockStorage>) -> SolanaBlockLoader {
        tracing::info!("Initializing solana block loader...");
        SolanaBlockLoader {
            program_id: self.program_id,
            batch_size: self.batch_size.unwrap_or(Self::DEFAULT_BATCH_SIZE) as Slot,
            block_retries: self.block_retries.unwrap_or(Self::DEFAULT_BLOCK_RETRIES),
            tx_retries: self.tx_retries.unwrap_or(Self::DEFAULT_TX_RETRIES),
            retry_int: Duration::from_secs(
                self.retry_int_sec.unwrap_or(Self::DEFAULT_RETRY_INT_SEC),
            ),
            solana_block_storage,
            commitment: self.commitment,
            client: self.client.init(),
        }
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
        tracing::info!("Initializing ethereum block storage...");
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
    Relayer {
        relayer_url: String,
        connection: pg_storage::config::PgPoolConfig,
    },
}

impl SolanaStorageConfig {
    pub async fn init(&self) -> ProgramResult<Arc<dyn SolanaBlockStorage>> {
        tracing::info!("Initializing solana block storage...");
        match self {
            SolanaStorageConfig::PgStorage { connection } => Ok(Arc::new(
                pg_storage::SolanaBlockStorage::new(connection.init()?),
            )),
            SolanaStorageConfig::Relayer {
                relayer_url,
                connection,
            } => RelayerSolanaBlockStorage::new(relayer_url.clone(), connection.init()?).await,
        }
    }
}
