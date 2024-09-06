/// Geth Engine Arguments
#[derive(clap::Args, Debug)]
pub struct GethEngineConfig {
    /// geth engine addr
    #[arg(short = 'g', long, default_value = "http://0.0.0.0:8551")]
    pub geth_engine_addr: url::Url,
    /// geth engine secret
    #[arg(short = 'g', long)]
    pub geth_engine_secret: String,
}
