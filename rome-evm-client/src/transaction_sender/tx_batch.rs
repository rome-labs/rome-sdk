use {
    crate::{
        error::{Result, RomeEvmError::*},
        transaction_sender::utils::{build_solana_tx, preflight_error},
    },
    solana_client::rpc_client::RpcClient,
    solana_program::instruction::Instruction,
    solana_sdk::{
        signature::{Keypair, Signature},
        transaction::Transaction,
    },
    std::{sync::Arc, time::Duration},
    tokio::select,
    tokio_util::sync::CancellationToken,
};

const SEND_BATCH_ATTEMPTS: u8 = 3;
const SEND_BATCH_TICK: u64 = 3;

pub struct TxBatch {
    rpc_client: Arc<RpcClient>,
    transactions: Vec<Transaction>,
    keypair: Arc<Keypair>,
}

impl TxBatch {
    pub fn new(
        rpc_client: Arc<RpcClient>,
        instructions: &[Instruction],
        keypair: Arc<Keypair>,
    ) -> Result<Self> {
        let recent_blockhash = rpc_client.get_latest_blockhash()?;
        Ok(Self {
            rpc_client,
            transactions: instructions
                .iter()
                .filter_map(|ix| build_solana_tx(recent_blockhash, &keypair, &[ix.clone()]).ok())
                .collect::<Vec<Transaction>>(),
            keypair: keypair.clone(),
        })
    }

    fn send_tx_preflight_check(&self, tx: &Transaction) -> Result<Option<Signature>> {
        match self.rpc_client.send_transaction(tx) {
            Ok(sig) => Ok(Some(sig)),
            Err(e) => {
                preflight_error(e)?;
                return Ok(None);
            }
        }
    }

    fn update_transaction(&self, tx: &Transaction) -> Result<Transaction> {
        let blockhash = self.rpc_client.get_latest_blockhash()?;
        let mut tx = tx.clone();
        tx.sign(&[&self.keypair], blockhash);

        Ok(tx)
    }

    pub async fn send(&self, token: CancellationToken) -> Result<Vec<Signature>> {
        let mut interval = tokio::time::interval(Duration::from_secs(SEND_BATCH_TICK));

        let send = |to_confirm: &mut Vec<Signature>| -> Result<bool> {
            for tx in &self.transactions {
                if let Some(sig) = self.send_tx_preflight_check(tx)? {
                    to_confirm.push(sig);
                } else {
                    return Ok(true);
                };
            }
            return Ok(false);
        };

        let confirm = |to_confirm: &mut Vec<Signature>| -> Result<bool> {
            let mut completed = true;
            for (sig, tx) in to_confirm.iter_mut().zip(&self.transactions) {
                if !self.rpc_client.confirm_transaction(sig)? {
                    let tx = self.update_transaction(tx)?;
                    if let Some(sig_) = self.send_tx_preflight_check(&tx)? {
                        *sig = sig_;
                    } else {
                        return Ok(true);
                    };
                    completed = false;
                }
            }
            Ok(completed)
        };

        let mut to_confirm = vec![];
        let mut attempt = 0;

        loop {
            select! {
            _ = token.cancelled() => {
                    tracing::info!("Shutdown transaction batch sender");
                    break;
                },
            _ = interval.tick() => {
                    let completed = if attempt < SEND_BATCH_ATTEMPTS {
                        if to_confirm.is_empty() {
                            send(&mut to_confirm)?
                        } else {
                            confirm(&mut to_confirm)?
                        }
                    } else {
                        break;
                    };

                    if completed {
                        return Ok(to_confirm)
                    };

                    attempt += 1;
                }
            }
        }

        Err(NotEnoughAttempts)
    }
}
