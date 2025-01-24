use crate::batch::{AdvanceTx, AtomicIxBatch, IxExecStepBatch};
use crate::indexers::clock::SolanaClock;
use crate::types::AsyncAtomicRpcClient;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_rpc_client_api::client_error::Result as ClientResult;
use solana_sdk::signature::{Keypair, Signature};
use solana_sdk::transaction::Transaction;
use std::sync::Arc;

/// A tower that manages functionalities of the Solana network
#[derive(Clone)]
pub struct SolanaTower {
    client: AsyncAtomicRpcClient,
    /// Solana Clock
    clock: SolanaClock,
}

impl SolanaTower {
    /// Create a new instance of [SolanaTower]
    pub fn new(client: AsyncAtomicRpcClient, clock: SolanaClock) -> Self {
        Self { client, clock }
    }

    /// Get the RPC client
    pub fn client(&self) -> &RpcClient {
        &self.client
    }

    /// Get the cloned RPC client
    pub fn client_cloned(&self) -> AsyncAtomicRpcClient {
        self.client.clone()
    }

    /// Get the Solana clock
    pub fn clock(&self) -> &SolanaClock {
        &self.clock
    }

    async fn to_tx<'a>(
        &self,
        ixs: &'a AtomicIxBatch<'a>,
        payer: &Keypair,
    ) -> ClientResult<Transaction> {
        let blockhash = self.client.get_latest_blockhash().await?;
        Ok(ixs.compose_solana_tx(payer, blockhash))
    }

    /// Send and confirm a transaction composed of [AtomicIxBatch]
    #[tracing::instrument(skip(self, ixs, payer))]
    pub async fn send_and_confirm<'a>(
        &self,
        ixs: &'a AtomicIxBatch<'a>,
        payer: &Keypair,
    ) -> ClientResult<Signature> {
        let tx = self.to_tx(ixs, payer).await?;
        tracing::info!("Sending tx: {:?}", tx.signatures[0]);

        self.client.send_and_confirm_transaction(&tx).await
    }

    /// Send and confirm a transaction composed of [AtomicIxBatch] with signers
    pub async fn send_and_confirm_with_signers<'a>(
        &self,
        ixs: &'a AtomicIxBatch<'a>,
        payer: &Keypair,
        signers: Vec<Arc<Keypair>>,
    ) -> ClientResult<Signature> {
        println!("send_and_confirm_with_signers");
        let blockhash = self.client.get_latest_blockhash().await?;
        let signers_slice: Vec<&Keypair> = signers.iter().map(|arc| arc.as_ref()).collect();

        let tx = ixs.compose_solana_tx_with_signers(payer, &signers_slice, blockhash);
        println!("Sending tx: {:?}", tx);
        tracing::info!("Sending tx: {:?}", tx.signatures[0]);

        self.client.send_and_confirm_transaction(&tx).await
    }

    /// Send, confirm and send again if necessary
    #[tracing::instrument(skip(self, ixs, payer))]
    pub async fn send_confirm_resend<'a>(
        &self,
        ixs: &'a AtomicIxBatch<'a>,
        payer: &Keypair,
    ) -> ClientResult<Signature> {
        let tx = self.to_tx(ixs, payer).await?;
        tracing::info!("Sending tx: {:?}", tx.signatures[0]);

        match self.client.send_and_confirm_transaction(&tx).await {
            Ok(sig) => Ok(sig),
            Err(err) => {
                tracing::error!("Failed to send tx: {:#?}, sending one more time", err);
                let tx = self.update_tx(&tx, payer).await?;
                self.client.send_and_confirm_transaction(&tx).await
            }
        }
    }

    async fn update_tx(&self, tx: &Transaction, payer: &Keypair) -> ClientResult<Transaction> {
        let blockhash = self.client.get_latest_blockhash().await?;
        let mut tx = tx.clone();
        tx.sign(&[payer], blockhash);

        Ok(tx)
    }

    /// parallelize send and confirm transactions composed from multiple [AtomicIxBatch]
    pub async fn send_and_confirm_parallel<'a>(
        &self,
        batch: &'a [AtomicIxBatch<'a>],
        payer: &Keypair,
    ) -> ClientResult<Vec<Signature>> {
        // create a batch of futures
        let futs = batch
            .iter()
            .map(|batch| self.send_confirm_resend(batch, payer));

        futures_util::future::join_all(futs)
            .await
            .into_iter()
            .collect::<ClientResult<Vec<_>>>()
    }

    /// parallelize send and confirm transactions without checking result
    pub async fn send_and_confirm_parallel_unchecked<'a>(
        &self,
        batch: &'a [AtomicIxBatch<'a>],
        payer: &Keypair,
    ) -> Vec<ClientResult<Signature>> {
        // create a batch of futures
        let futs = batch
            .iter()
            .map(|batch| self.send_and_confirm(batch, payer));

        futures_util::future::join_all(futs).await
    }

    #[tracing::instrument(skip(self, tx))]
    pub async fn send_and_confirm_tx_iterable<Error: std::fmt::Debug>(
        &self,
        tx: &mut dyn AdvanceTx<'_, Error = Error>,
    ) -> anyhow::Result<Vec<Signature>> {
        println!("send_and_confirm_tx_iterable\n");

        let payer = tx.payer();
        let mut sigs = Vec::new();
        let mut unchecked_sigs = Vec::new();

        loop {
            let tx = match tx.advance() {
                Ok(tx) => tx,
                Err(e) => return Err(anyhow::anyhow!("Failed to advance tx: {:?}", e)),
            };

            match tx {
                IxExecStepBatch::Single(tx) => {
                    let sig = self.send_and_confirm(&tx, &payer).await.map_err(|e| {
                        tracing::warn!("Failed to send and confirm single tx: {}", e);
                        e
                    })?;

                    tracing::info!("Single tx sig: {:?}", sig);

                    sigs.push(sig);
                }
                IxExecStepBatch::SingleWithSigners(tx, signers) => {
                    println!("SingleWithSigners");
                    let sig = self
                        .send_and_confirm_with_signers(&tx, &payer, signers)
                        .await
                        .map_err(|e| {
                            tracing::warn!("Failed to send and confirm single tx: {}", e);
                            e
                        })?;

                    tracing::info!("Single tx sig: {:?}", sig);

                    sigs.push(sig);
                }
                IxExecStepBatch::Parallel(batch) => {
                    let batch_sigs = self
                        .send_and_confirm_parallel(&batch, &payer)
                        .await
                        .map_err(|e| {
                            tracing::warn!("Failed to send and confirm txs in parallel: {}", e);
                            e
                        })?;

                    tracing::info!("Parallel sigs: {:#?}", batch_sigs);

                    sigs.extend(batch_sigs);
                }
                IxExecStepBatch::ParallelUnchecked(batch) => {
                    unchecked_sigs = self
                        .send_and_confirm_parallel_unchecked(&batch, &payer)
                        .await;
                }
                IxExecStepBatch::ConfirmationIterativeTx(confirm) => {
                    if confirm {
                        let batch = unchecked_sigs
                            .iter()
                            .filter(|res| res.is_ok())
                            .map(|a| *a.as_ref().unwrap())
                            .collect::<Vec<Signature>>();

                        tracing::info!("Parallel sigs: {:#?}", batch);
                        sigs.extend(batch);
                    } else {
                        let errs = unchecked_sigs
                            .iter()
                            .filter(|res| res.is_err())
                            .map(|a| a.as_ref().unwrap_err())
                            .collect::<Vec<_>>();

                        for e in &errs {
                            tracing::info!("Failed to send iterative tx: {:?}", e);
                        }

                        let error = if errs.is_empty() {
                            let mes = "tx execution is not completed or tx status unknown";
                            tracing::info!(mes);
                            mes.to_string()
                        } else {
                            format!("{}", errs.last().unwrap())
                        };
                        return Err(anyhow::anyhow!("Failed to send iterative tx: {}", error));
                    }
                }
                IxExecStepBatch::End => break,
            }
        }

        Ok(sigs)
    }
}
