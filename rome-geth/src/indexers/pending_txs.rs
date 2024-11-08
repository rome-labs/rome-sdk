use std::sync::Arc;
use anyhow::bail;

use crate::types::{GethTxPoolResult, GethTxPoolSender};
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
    /// Generate the payload to get the pending transactions from the geth node
    #[inline]
    pub fn generate_payload() -> JsonRpcRequestOwned {
        JsonRpcRequest::new_with_params_owned("txpool_content", vec![])
    }

    /// Generate a constant poller for the geth node
    pub fn generate_constant_poller(
        &self,
        tx: GethTxPoolSender,
    ) -> ConstantPoller<Arc<JsonRpcResponse<GethTxPoolResult>>> {
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
        let poller = self.generate_constant_poller(geth_tx);
        let poller = Arc::new(Poller::new_constant(poller));

        let poller_jh = tokio::spawn(poller.listen_with_service_runner(service_runner));

        tokio::select! {
            res = poller_jh => {
                bail!("Geth Polling failed: {:?}", res);
            }
        }
    }
}
