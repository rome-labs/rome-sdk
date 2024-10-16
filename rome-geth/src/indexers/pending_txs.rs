use std::collections::HashSet;
use std::sync::Arc;

use anyhow::bail;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::types::{GethTxPoolResult, GethTxPoolSender, GethTxPoolTx};
use rome_utils::jsonrpc::{JsonRpcRequest, JsonRpcRequestOwned, JsonRpcResponse};
use rome_utils::services::{ConstantPoller, Poller, ServiceRunner};

/// Polls the geth node for pending transactions and sends them to the
/// output channel
///
/// Method used to poll the geth node is `txpool_content`
#[derive(clap::Args, Debug, serde::Serialize, serde::Deserialize)]
pub struct GethPendingTxsIndexer {
    /// address of the geth node
    #[arg(short = 'g', long, default_value_t = default_geth_http_addr())]
    #[serde(default = "default_geth_http_addr")]
    pub geth_http_addr: url::Url,

    /// poll interval milliseconds
    #[arg(short = 'i', long, default_value_t = default_poll_interval_ms())]
    #[serde(default = "default_poll_interval_ms")]
    pub poll_interval_ms: u64,
}

fn default_geth_http_addr() -> url::Url {
    url::Url::parse("http://0.0.0.0:8545").unwrap()
}

fn default_poll_interval_ms() -> u64 {
    100
}

impl GethPendingTxsIndexer {
    /// Get pending transactions from the geth response.
    pub fn get_txs(res: JsonRpcResponse<GethTxPoolResult>) -> Option<Vec<GethTxPoolTx>> {
        let Some(result) = res.result else {
            tracing::warn!("Error in response: {:?}", res.error);
            return None;
        };

        let queued = result
            .queued
            .into_values()
            .flat_map(|tx| tx.into_values().collect::<Vec<_>>());

        let pending = result
            .pending
            .into_values()
            .flat_map(|tx| tx.into_values().collect::<Vec<_>>());

        let txs = queued.chain(pending).collect::<Vec<_>>();

        Some(txs)
    }

    /// Process the channel and send the transactions to the mempool_tx channel
    pub async fn process_channel(
        mut rx: UnboundedReceiver<JsonRpcResponse<GethTxPoolResult>>,
        mempool_tx: GethTxPoolSender,
    ) -> ! {
        let mut sent_txs = HashSet::<String>::new();

        while let Some(res) = rx.recv().await {
            let Some(pending_txs) = Self::get_txs(res) else {
                continue;
            };

            pending_txs.into_iter().for_each(|tx| {
                if sent_txs.contains(&tx.hash.to_string()) {
                    return;
                }

                tracing::trace!("Geth Pending transactions: {:?}", tx);

                sent_txs.insert(tx.hash.to_string());
                mempool_tx.send(tx).expect("Mempool channel closed");
            });
        }

        panic!("Geth Poller channel closed");
    }

    /// Generate the payload to get the pending transactions from the geth node
    #[inline]
    pub fn generate_payload() -> JsonRpcRequestOwned {
        JsonRpcRequest::new_with_params_owned("txpool_content", vec![])
    }

    /// Generate a constant poller for the geth node
    pub fn generate_constant_poller(
        &self,
        tx: UnboundedSender<JsonRpcResponse<GethTxPoolResult>>,
    ) -> ConstantPoller<JsonRpcResponse<GethTxPoolResult>> {
        ConstantPoller {
            tx,
            addr: self.geth_http_addr.clone(),
            payload: serde_json::to_value(Self::generate_payload())
                .expect("Failed to serialize payload"),
            interval: std::time::Duration::from_millis(self.poll_interval_ms),
        }
    }

    /// start polling the geth node for pending transactions
    /// and send them to the mempool_tx channel
    pub async fn listen(
        self,
        geth_tx: GethTxPoolSender,
        service_runner: ServiceRunner,
    ) -> anyhow::Result<()> {
        let (poller_tx, poller_rx) = tokio::sync::mpsc::unbounded_channel();

        let poller = self.generate_constant_poller(poller_tx);
        let poller = Arc::new(Poller::new_constant(poller));

        let poller_jh = tokio::spawn(poller.listen_with_service_runner(service_runner));
        let processor_jh = tokio::spawn(Self::process_channel(poller_rx, geth_tx));

        tokio::select! {
            res = poller_jh => {
                bail!("Geth Polling failed: {:?}", res);
            }
            res = processor_jh => {
                bail!("Bundler failed: {:?}", res);
            },
        }
    }
}
