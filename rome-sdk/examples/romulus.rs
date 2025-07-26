mod common;

use std::sync::Arc;

use rome_sdk::{Rome, RomeConfig, RomulusTx};

const CHAIN_ID_FIRST: u64 = 121212;
const CHAIN_ID_SECOND: u64 = 121213;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize the subscriber
    tracing_subscriber::fmt::init();

    // Load the configuration
    let config = RomeConfig::load_json(common::CONFIG_PATH.parse()?).await?;

    // Create a new Rome instance with the configuration
    let rome = Rome::new_with_config(config).await?;

    // Create ethereum wallet and transfer transactions
    let wallet = common::create_wallet();
    let eth_tx1 = common::construct_transfer_tx(&rome, &wallet, CHAIN_ID_FIRST).await?;
    let eth_tx2 = common::construct_transfer_tx(&rome, &wallet, CHAIN_ID_SECOND).await?;
    let eth_txs = vec![eth_tx1, eth_tx2];

    // Create Solana wallet and transfer instruction
    let sender = common::create_solana_payer();
    let sol_ixs = vec![common::construct_solana_transfer_ix(&sender)];
    let signers = vec![Arc::new(sender)];

    let romulus_tx = RomulusTx::new(eth_txs, sol_ixs);

    // Compose a cross chain atomic transaction
    let mut rome_tx = rome.compose_cross_chain_tx(romulus_tx, signers).await?;

    // Send the transaction to the solana network
    let signature = rome.send_and_confirm(&mut *rome_tx).await?;

    // Print the signature and the explorer link
    tracing::info!("Signature: {:?}", signature);
    tracing::info!(
        "https://explorer.solana.com/tx/{}?cluster={}",
        signature,
        "devnet",
    );

    // Exit with success
    Ok(())
}
