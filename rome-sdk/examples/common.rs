use anyhow::Context;
use ethers::signers::{LocalWallet, Signer};
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::{Address, TransactionRequest};
use rome_sdk::{EthSignedTxTuple, Rome};

pub const CONFIG_PATH: &str = "./example.config.json";
pub const EXAMPLE_WALLET: &str = "ed234d0929176fc58f699be15c7f606f745223d93ceb3b4042e55e825484c043";
pub const TO_ADDRESS: &str = "0xb94f5374fce5edbc8e2a8697c15331677e6ebf0b";

pub fn create_wallet() -> LocalWallet {
    EXAMPLE_WALLET.parse().unwrap()
}

/// Sign a transaction with the wallet and return the [EthSignedTxTuple]
pub async fn sign_tx(
    wallet: &LocalWallet,
    tx: TypedTransaction,
) -> anyhow::Result<EthSignedTxTuple> {
    let sig = wallet
        .sign_transaction(&tx)
        .await
        .context("failed to sign transaction")?;

    Ok(EthSignedTxTuple::new(tx, sig))
}

/// Construct a simple ethereum transfer transaction
pub async fn construct_transfer_tx(
    rome: &Rome,
    wallet: &LocalWallet,
    chain_id: u64,
) -> anyhow::Result<EthSignedTxTuple> {
    // From wallet
    let from = wallet.address();

    // To address
    let to = TO_ADDRESS.parse::<Address>().unwrap();

    // nonce
    let nonce = rome
        .transaction_count(from, chain_id)
        .context("failed to get transaction count")?;

    // create a simple transaction request
    let tx = TransactionRequest {
        from: Some(from),
        to: Some(to.into()),
        chain_id: Some(chain_id.into()),
        value: Some(100.into()),
        nonce: Some(nonce.into()),
        ..Default::default()
    };

    sign_tx(wallet, tx.into()).await
}

fn main() {
    panic!("This is not an executable example")
}
