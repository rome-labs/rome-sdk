mod common;

use rome_sdk::{EthSignedTxTuple, RheaTx, Rome, RomeConfig};

const CHAIN_ID: u64 = 1001;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Let's load the configuration
    let config = RomeConfig::load_json(common::CONFIG_PATH.parse()?).await?;

    // create ethereum wallet
    let wallet = common::create_wallet();

    // Let's create a new Rome instance with the configuration
    // and start the services
    let rome = Rome::new_with_config(config).await?;

    // Let's create a simple ethereum transfer transaction
    let tx: EthSignedTxTuple = common::construct_transfer_tx(&rome, &wallet, CHAIN_ID).await?;

    // Create a rhea transaction
    let rhea_tx = RheaTx::new(tx);

    // Let's compose a simple rollup transaction
    let rome_tx = rome.compose_rollup_tx(rhea_tx).await?;

    // send the transaction to the solana network
    let signature = rome.send_and_confirm(&*rome_tx).await?;

    // print the signature and the explorer link
    println!("Signature: {:?}", signature);
    println!("https://explorer.solana.com/tx/{}", signature);

    // exit with success
    Ok(())
}
