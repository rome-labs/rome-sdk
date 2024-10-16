use crate::batch::{AdvanceTx, AtomicIxBatch, IxExecStepBatch};
use crate::indexers::clock::SolanaClock;
use crate::types::AsyncAtomicRpcClient;
use solana_client::client_error::ClientErrorKind;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_rpc_client_api::client_error::Result as ClientResult;
use solana_rpc_client_api::{
    client_error,
    request::{RpcError::RpcResponseError, RpcResponseErrorData::SendTransactionPreflightFailure},
    response::RpcSimulateTransactionResult,
};
use solana_sdk::signature::{Keypair, Signature};
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

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

    async fn to_tx<'a>(&self, ixs: &'a AtomicIxBatch<'a>, payer: &Keypair) -> Transaction {
        let blockhash = self.clock.get_current_blockhash().await;
        ixs.compose_solana_tx(payer, blockhash)
    }

    /// Send and confirm a transaction composed of [AtomicIxBatch]
    #[tracing::instrument(skip(self, ixs, payer))]
    pub async fn send_and_confirm<'a>(
        &self,
        ixs: &'a AtomicIxBatch<'a>,
        payer: &Keypair,
    ) -> ClientResult<Signature> {
        let tx = self.to_tx(ixs, payer).await;
        tracing::info!("Sending tx: {:?}", tx.signatures[0]);

        self.client.send_and_confirm_transaction(&tx).await
    }

    /// Send, confirm and simulate if necessary to check result of the iterative tx
    #[tracing::instrument(skip(self, ixs, payer))]
    pub async fn send_confirm_simulate<'a>(
        &self,
        ixs: &'a AtomicIxBatch<'a>,
        payer: &Keypair,
    ) -> anyhow::Result<Option<Signature>> {
        let tx = self.to_tx(ixs, payer).await;
        tracing::info!("Sending tx: {:?}", tx.signatures[0]);

        match self.client.send_and_confirm_transaction(&tx).await {
            Ok(sig) => Ok(Some(sig)),
            Err(err) => self
                .check_preflight_or_simulate(&tx, err, payer)
                .await
                .map_err(|e| e.into()),
        }
    }

    async fn update_tx(&self, tx: &Transaction, payer: &Keypair) -> ClientResult<Transaction> {
        let blockhash = self.client.get_latest_blockhash().await?;
        let mut tx = tx.clone();
        tx.sign(&[payer], blockhash);

        Ok(tx)
    }
    async fn check_preflight_or_simulate(
        &self,
        tx: &Transaction,
        err: client_error::Error,
        payer: &Keypair,
    ) -> ClientResult<Option<Signature>> {
        let simulate = match &err {
            client_error::Error {
                kind:
                    ClientErrorKind::RpcError(RpcResponseError {
                        data: SendTransactionPreflightFailure(simulate),
                        ..
                    }),
                ..
            } => simulate.clone(),
            _ => {
                let tx = self.update_tx(tx, payer).await?;
                self.client.simulate_transaction(&tx).await?.value
            }
        };

        if Self::unnecessary_tx(&simulate) {
            Ok(None)
        } else {
            tracing::error!("Failed to send tx: {:#?}", err);
            Err(err)
        }
    }

    // TODO: this filter belongs in rome-evm-client
    pub fn unnecessary_tx(simulate: &RpcSimulateTransactionResult) -> bool {
        match simulate {
            RpcSimulateTransactionResult {
                err: Some(solana_sdk::transaction::TransactionError::InstructionError(_, _)),
                logs: Some(log),
                ..
            } => log
                .iter()
                .any(|a| a.starts_with("Program log: Error: UnnecessaryIteration")),
            _ => false,
        }
    }

    /// parallelize send and confirm transactions composed from multiple [AtomicIxBatch]
    pub async fn send_and_confirm_parallel<'a>(
        &self,
        batch: &'a [AtomicIxBatch<'a>],
        payer: &Keypair,
    ) -> anyhow::Result<Vec<Signature>> {
        // create a batch of futures
        let futs = batch
            .iter()
            .map(|batch| self.send_confirm_simulate(batch, payer));

        Ok(futures_util::future::join_all(futs)
            .await
            .into_iter()
            .collect::<anyhow::Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect())
    }

    #[tracing::instrument(skip(self, tx, payer))]
    pub async fn send_and_confirm_tx_iterable<Error: std::fmt::Debug>(
        &self,
        tx: &mut dyn AdvanceTx<'_, Error = Error>,
        payer: &Keypair,
    ) -> anyhow::Result<Vec<Signature>> {
        let payer_key = payer.pubkey();
        let mut sigs = Vec::new();

        loop {
            let tx = match tx.advance(&payer_key) {
                Ok(tx) => tx,
                Err(e) => return Err(anyhow::anyhow!("Failed to advance tx: {:?}", e)),
            };

            match tx {
                IxExecStepBatch::Single(tx) => {
                    let sig = self.send_and_confirm(&tx, payer).await.map_err(|e| {
                        tracing::warn!("Failed to send and confirm single tx: {}", e);
                        e
                    })?;

                    tracing::info!("Single tx sig: {:?}", sig);

                    sigs.push(sig);
                }
                IxExecStepBatch::Parallel(batch) => {
                    let batch_sigs = self
                        .send_and_confirm_parallel(&batch, payer)
                        .await
                        .map_err(|e| {
                            tracing::warn!("Failed to send and confirm txs in parallel: {}", e);
                            e
                        })?;

                    tracing::info!("Parallel sigs: {:#?}", batch_sigs);

                    sigs.extend(batch_sigs);
                }
                IxExecStepBatch::End => break,
            }
        }

        Ok(sigs)
    }
}
