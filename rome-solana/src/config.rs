use std::str::FromStr;

use solana_sdk::commitment_config::{CommitmentConfig, CommitmentLevel};
use url::Url;

/// Config to be used for solana rpc client.
#[derive(clap::Args, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SolanaConfig {
    /// The RPC URL of the solana node.
    #[clap(long, default_value_t = Url::from_str("http://localhost:8899").unwrap())]
    pub rpc_url: Url,
    /// The commitment level of the data.
    #[clap(long, default_value_t = CommitmentLevel::Confirmed)]
    pub commitment: CommitmentLevel,
}

impl Default for SolanaConfig {
    fn default() -> Self {
        Self {
            rpc_url: Url::from_str("http://localhost:8899").unwrap(),
            commitment: CommitmentLevel::Confirmed,
        }
    }
}

impl From<SolanaConfig> for solana_client::nonblocking::rpc_client::RpcClient {
    fn from(val: SolanaConfig) -> Self {
        solana_client::nonblocking::rpc_client::RpcClient::new_with_commitment(
            val.rpc_url.to_string(),
            CommitmentConfig {
                commitment: val.commitment,
            },
        )
    }
}

impl From<SolanaConfig> for solana_client::rpc_client::RpcClient {
    fn from(val: SolanaConfig) -> Self {
        solana_client::rpc_client::RpcClient::new_with_commitment(
            val.rpc_url.to_string(),
            CommitmentConfig {
                commitment: val.commitment,
            },
        )
    }
}
