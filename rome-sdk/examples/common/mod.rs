use anyhow::Context;
use ethers::signers::{LocalWallet, Signer};
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::{Address, TransactionRequest};
use rome_sdk::{EthSignedTxTuple, Rome};
use solana_sdk::instruction::Instruction;
use solana_sdk::{
    pubkey::Pubkey,
    signature::{Keypair, Signer as SolSigner},
    system_instruction,
};

pub const CONFIG_PATH: &str = "./example.config.json";

// pub const EXAMPLE_WALLET_ADDRESS: &str = "0xae600d1f94680ef43ab12f8d618f8aafc208fe25";
pub const EXAMPLE_WALLET: &str = "ed234d0929176fc58f699be15c7f606f745223d93ceb3b4042e55e825484c043";
pub const TO_ADDRESS: &str = "0xb94f5374fce5edbc8e2a8697c15331677e6ebf0b";

// Pubkey: 511XBCFYhf1ZhuJnQtcgrXm5kJu4bzeswb2J381uWNrp
pub const SOLANA_KEYPAIR: &[u8] = &[
    226, 151, 143, 204, 1, 174, 174, 21, 150, 60, 121, 217, 202, 54, 218, 207, 247, 94, 133, 33,
    22, 6, 229, 7, 138, 44, 54, 209, 52, 174, 47, 219, 59, 111, 86, 2, 175, 177, 3, 81, 104, 72,
    51, 185, 227, 209, 55, 95, 193, 139, 155, 96, 151, 121, 26, 95, 240, 71, 6, 97, 30, 127, 224,
    197,
];

pub fn create_wallet() -> LocalWallet {
    EXAMPLE_WALLET.parse().unwrap()
}

#[allow(dead_code)]
pub fn create_solana_payer() -> Keypair {
    Keypair::from_bytes(SOLANA_KEYPAIR).unwrap()
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

    // create a legacy transaction request
    let mut tx = TransactionRequest {
        to: Some(to.into()),
        from: Some(from),
        nonce: Some(nonce.into()),
        chain_id: Some(chain_id.into()),
        gas_price: Some(1.into()),
        value: Some(100.into()),
        ..Default::default()
    };

    // estimate gas
    tx.gas = Some(rome.estimate_gas(&tx).context("failed to estimate gas")?);

    // convert the tx into a legacy tx
    let tx = TypedTransaction::Legacy(tx);

    // sign the transaction
    sign_tx(wallet, tx).await
}

#[allow(dead_code)]
/// Constructs a solana transfer instruction
pub fn construct_solana_transfer_ix(sender: &Keypair) -> Instruction {
    let recipient = Pubkey::new_unique();
    let lamports = 1_000_000; // 0.001 SOL

    let transfer_instruction = system_instruction::transfer(&sender.pubkey(), &recipient, lamports);

    transfer_instruction
}
