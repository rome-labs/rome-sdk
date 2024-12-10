use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Context;
use rome_evm_client::resources::PayerConfig;
use rome_solana::config::SolanaConfig;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
/// Rome interface configuration
pub struct RomeConfig {
    /// Config to solana rpc
    #[serde(flatten)]
    pub solana_config: SolanaConfig,

    /// Maps chain id to rollup program address
    pub rollups: HashMap<u64, String>,

    /// Path to payer key-pair file
    pub payers: Vec<PayerConfig>,
}

impl RomeConfig {
    /// Load the configuration from the default path
    pub async fn load_json(path: PathBuf) -> anyhow::Result<Self> {
        let file = tokio::fs::read_to_string(path)
            .await
            .context("Failed to read config file")?;

        serde_json::from_str(&file).context("Failed to parse config file")
    }

    /// Load the configuration from the default path
    pub async fn load_yml(path: PathBuf) -> anyhow::Result<Self> {
        let file = tokio::fs::read_to_string(path)
            .await
            .context("Failed to read config file")?;

        serde_yaml::from_str(&file).context("Failed to parse config file")
    }
}
