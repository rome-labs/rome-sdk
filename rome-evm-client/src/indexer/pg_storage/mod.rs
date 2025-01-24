use diesel::r2d2::ConnectionManager;
use diesel::PgConnection;
use r2d2::Pool;
pub mod config;
mod ethereum_block_storage;
mod solana_block_storage;
mod transaction_storage;
mod types;

pub type PgPool = Pool<ConnectionManager<PgConnection>>;
pub use ethereum_block_storage::EthereumBlockStorage;
pub use solana_block_storage::SolanaBlockStorage;
