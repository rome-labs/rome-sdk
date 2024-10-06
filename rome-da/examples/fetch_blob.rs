use dotenv::dotenv;
use reqwest::Url;
use rome_da::celestia::RomeDaClient;
use std::env;

/// example method to fetch a blob
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

    let height = 1944524;
    let res = client.fetch_blobs_by_height(height).await?;
    println!("{:?}", res);

    Ok(())
}