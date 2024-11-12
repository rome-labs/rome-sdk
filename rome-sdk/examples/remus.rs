mod common;

use rome_sdk::{RemusTx, Rome, RomeConfig};

const CHAIN_ID_FIRST: u64 = 200001;
const CHAIN_ID_SECOND: u64 = 1003;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Let's load the configuration
    let config = RomeConfig::load_json(common::CONFIG_PATH.parse()?).await?;

    // create ethereum wallet
    let wallet = common::create_wallet();

    // Let's create a new Rome instance with the configuration
    // and start the services
    let rome = Rome::new_with_config(config).await?;

    // Let's create 2 simple ethereum transfer transactions
    let tx1 = common::construct_transfer_tx(&rome, &wallet, CHAIN_ID_FIRST).await?;
    let tx2 = common::construct_transfer_tx(&rome, &wallet, CHAIN_ID_SECOND).await?;

    // A vector of transactions
    let txs = vec![tx1, tx2];

    // Create a remus transaction
    let remus_tx = RemusTx::new(txs);

    // Let's compose a cross rollup atomic transaction
    let mut rome_tx = rome.compose_cross_rollup_tx(remus_tx).await?;

    // send the transaction to the solana network
    let signature = rome.send_and_confirm(&mut *rome_tx).await?;

    // print the signature and the explorer link
    println!("Signature: {:?}", signature);
    println!("https://explorer.solana.com/tx/{}", signature);

    // exit with success
    Ok(())
}
