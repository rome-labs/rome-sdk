use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::signature::{Keypair, Signature};

use crate::batch::AtomicIxBatch;
use crate::indexers::clock::SolanaClock;
use crate::types::AtomicRpcClient;

/// A tower that manages functionalities of the Solana network
#[derive(Clone)]
pub struct SolanaTower {
    client: AtomicRpcClient,
    /// Solana Clock
    clock: SolanaClock,
}

impl SolanaTower {
    pub fn new(client: AtomicRpcClient, clock: SolanaClock) -> Self {
        Self { client, clock }
    }

    /// Get the RPC client
    pub fn client(&self) -> &RpcClient {
        &self.client
    }

    /// Get the Solana clock
    pub fn clock(&self) -> &SolanaClock {
        &self.clock
    }

    /// Send and confirm a transaction composed of [AtomicIxBatch]
    pub async fn send_and_confirm<'a>(
        &self,
        batch: &'a AtomicIxBatch<'a>,
        payer: &Keypair,
    ) -> anyhow::Result<Signature> {
        let blockhash = self.clock.get_current_blockhash().await;
        let tx = batch.compose_solana_tx(payer, blockhash);

        // TODO: create a retry logic
        self.client
            .send_and_confirm_transaction(&tx)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send transaction: {:?}", e))
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
            .map(|batch| self.send_and_confirm(batch, payer));

        // wait for all futures to complete
        futures_util::future::join_all(futs)
            .await
            .into_iter()
            .collect()
    }

    /// sequential send and confirm transactions composed from multiple [AtomicIxBatch]
    pub async fn send_and_confirm_sequential<'a>(
        &self,
        batch: &'a [AtomicIxBatch<'a>],
        payer: &Keypair,
    ) -> anyhow::Result<Vec<Signature>> {
        let mut sigs = Vec::with_capacity(batch.len());

        for batch in batch {
            let sig = self.send_and_confirm(batch, payer).await?;
            sigs.push(sig);
        }

        Ok(sigs)
    }
}
