pub const DEFAULT_ENGINE_ADDR: &str = "http://0.0.0.0:8551";

/// Geth Engine Arguments
#[derive(clap::Args, Debug, serde::Serialize, serde::Deserialize)]
pub struct GethEngineConfig {
    /// geth engine addr
    #[arg(short = 'g', long, default_value = DEFAULT_ENGINE_ADDR)]
    #[serde(default = "default_geth_engine_addr")]
    pub geth_engine_addr: url::Url,
    /// geth engine secret
    #[arg(short = 'g', long)]
    pub geth_engine_secret: String,
}

fn default_geth_engine_addr() -> url::Url {
    url::Url::parse(DEFAULT_ENGINE_ADDR).unwrap()
}
