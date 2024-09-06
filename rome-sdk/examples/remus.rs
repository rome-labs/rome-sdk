use std::str::FromStr;

use anyhow::Context;
use ethers::signers::{LocalWallet, Signer};
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::{Address, NameOrAddress, Signature, TransactionRequest};
use rome_sdk::{EthSignedTxTuple, RemusTx, Rome, RomeConfig};

const CONFIG_PATH: &str = "./example.config.json";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Let's load the configuration
    let config = RomeConfig::load_json(CONFIG_PATH.parse()?).await?;

    // Let's create a new Rome instance with the configuration
    // and start the services
    let rome = Rome::new_with_config(config).await?;

    // Let's create 2 simple ethereum transfer transactions
    let tx1 = construct_tx().await?;
    let tx2 = construct_tx().await?;

    // A vector of transactions
    let txs = vec![tx1, tx2];

    // Create a remus transaction
    let remus_tx = RemusTx::new(txs);

    // Let's compose a cross rollup atomic transaction
    let rome_tx = rome.compose_cross_rollup_tx(remus_tx).await?;

    // send the transaction to the solana network
    let signature = rome.send_and_confirm_tx(rome_tx).await?;

    // print the signature and the explorer link
    println!("Signature: {:?}", signature);
    println!("https://explorer.solana.com/tx/{}", signature);

    // exit with success
    Ok(())
}

/// Construct a simple ethereum transfer transaction
pub async fn construct_tx() -> anyhow::Result<EthSignedTxTuple> {
    // create a local wallet
    let wallet: LocalWallet = "".parse().context("failed to parse private key")?;

    // create a simple transaction request
    let tx = TransactionRequest {
        from: Some(Address::random()),
        to: Some(NameOrAddress::from_str("haha.eth").unwrap()),
        chain_id: Some(1.into()),
        ..Default::default()
    };

    // convert the transaction request to a typed transaction
    let tx: TypedTransaction = tx.into();

    // sign the transaction and get the signature
    let sig: Signature = wallet
        .sign_transaction(&tx)
        .await
        .context("failed to sign transaction")?;

    Ok(EthSignedTxTuple::new(tx, sig))
}
