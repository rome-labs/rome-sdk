use crate::batch::{AdvanceTx, AtomicIxBatch, IxExecStepBatch, TxVersion};
use crate::indexers::clock::SolanaClock;
use crate::types::AsyncAtomicRpcClient;
use solana_client::{
    nonblocking::rpc_client::RpcClient,
    rpc_config::RpcSendTransactionConfig,
};
use solana_rpc_client_api::client_error::Result as ClientResult;
use solana_sdk::{
    signature::{Keypair, Signature},
    transaction::VersionedTransaction,
};
use std::sync::Arc;
use std::time::Duration;

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
        ver: &TxVersion,
    ) -> ClientResult<VersionedTransaction> {
        let blockhash = self.client.get_latest_blockhash().await?;

        let tx = match ver {
            TxVersion::Legacy => ixs.compose_legacy_solana_tx(payer, blockhash),
            TxVersion::V0(alt) => ixs.compose_v0_solana_tx(payer, blockhash, alt)?,
        };

        Ok(tx)
    }

    /// Send and confirm a transaction composed of [AtomicIxBatch]
    #[tracing::instrument(skip(self, ixs, payer, ver))]
    pub async fn send_and_confirm<'a>(
        &self,
        ixs: &'a AtomicIxBatch<'a>,
        payer: &Keypair,
        ver: &TxVersion,
    ) -> ClientResult<Signature> {
        let tx = self.to_tx(ixs, payer, ver).await?;
        tracing::info!("Sending tx: {:?}", tx.signatures[0]);

        self.client.send_and_confirm_transaction_with_spinner_and_config(
            &tx,
            self.client.commitment(),
            RpcSendTransactionConfig {
                skip_preflight : true,
                preflight_commitment: None,
                ..RpcSendTransactionConfig::default()
            }
        ).await
    }

    /// parallelize send and confirm transactions composed from multiple [AtomicIxBatch]
    pub async fn send_and_confirm_parallel<'a>(
        &self,
        batch: &'a [AtomicIxBatch<'a>],
        payer: &Keypair,
        ver: &TxVersion,
    ) -> ClientResult<Vec<Signature>> {
        // create a batch of futures
        let futs = batch
            .iter()
            .map(|batch| self.send_and_confirm(batch, payer, ver));

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
        ver: &TxVersion,
    ) -> Vec<ClientResult<Signature>> {
        // create a batch of futures
        let futs = batch
            .iter()
            .map(|batch| self.send_and_confirm(batch, payer, ver));

        futures_util::future::join_all(futs).await
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

        let tx = ixs.compose_legacy_solana_tx_with_signers(payer, &signers_slice, blockhash);
        println!("Sending tx: {:?}", tx);
        tracing::info!("Sending tx: {:?}", tx.signatures[0]);

        self.client.send_and_confirm_transaction(&tx).await
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
                IxExecStepBatch::Single(tx, ver) => {
                    let sig = self
                        .send_and_confirm(&tx, &payer, &ver)
                        .await
                        .map_err(|e| {
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
                IxExecStepBatch::Parallel(batch, ver) => {
                    let batch_sigs = self
                        .send_and_confirm_parallel(&batch, &payer, &ver)
                        .await
                        .map_err(|e| {
                            tracing::warn!("Failed to send and confirm txs in parallel: {}", e);
                            e
                        })?;

                    tracing::info!("Parallel sigs: {:#?}", batch_sigs);

                    sigs.extend(batch_sigs);
                }
                IxExecStepBatch::ParallelUnchecked(batch, ver) => {
                    unchecked_sigs = self
                        .send_and_confirm_parallel_unchecked(&batch, &payer, &ver)
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
                IxExecStepBatch::WaitNextSlot(slot) => loop {
                    if self.client.get_slot().await? > slot {
                        break;
                    } else {
                        let ms = Duration::from_millis(100);
                        tokio::time::sleep(ms).await;
                    }
                },
                IxExecStepBatch::End => break,
            }
        }

        Ok(sigs)
    }
}
