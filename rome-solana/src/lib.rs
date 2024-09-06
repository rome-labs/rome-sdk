/// Solana config
pub mod config;
/// Geyser interface
pub mod geyser;
/// Indexing strategies for data from solana node.
pub mod indexers;
/// Types related to solana.
pub mod types;

pub mod batch;

pub mod tower;

/// Max size for a transaction in bytes
pub const SOLANA_MAX_TX_SIZE: usize = 1232;
/// Max compute units for a transaction
pub const SOLANA_MAX_TX_COMPUTE_UNITS: usize = 200_000;

/// Base cost for processing the instruction
pub const SOLANA_IX_BASE_CU: usize = 500;
/// Cost per byte in the instruction
pub const SOLANA_COST_PER_BYTE: usize = 10;
