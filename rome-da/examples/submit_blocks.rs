use dotenv::dotenv;
use ethers::types::{U256, U64};
use reqwest::Url;
use rome_da::celestia::types::DaSubmissionBlock;
use rome_da::celestia::RomeDaClient;
use std::env;

/// example method to submit a blob
#[tokio::main]
pub async fn main() -> anyhow::Result<()> {
    dotenv().ok();
    tracing_subscriber::fmt::init();

    let celestia_url: Url = env::var("CELESTIA_URL")
        .expect("CELESTIA_URL is not set")
        .parse()
        .expect("CELESTIA_URL should be a valid url");

    let celestia_ws_url = env::var("CELESTIA_WS_URL").expect("CELESTIA_WS_URL is not set");
    let celestia_token = env::var("CELESTIA_TOKEN").expect("CELESTIA_TOKEN is not set");
    let chain_id: u64 = env::var("CHAIN_ID")
        .expect("CHAIN_ID is not set")
        .parse()
        .expect("CHAIN_ID should be a number");

    let client = RomeDaClient::new(celestia_url, celestia_ws_url, celestia_token, chain_id)?;

    let blocks = vec![DaSubmissionBlock {
        block_number: U64::from(9009),
        timestamp: U256::from(10000),
        transactions: vec![],
    }];
    let res = client.submit_blocks(&blocks).await?;
    println!("{:?}", res);

    Ok(())
}
