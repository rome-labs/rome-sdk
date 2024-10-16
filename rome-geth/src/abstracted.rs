use std::time::Duration;

use url::Url;

use crate::indexers::pending_txs::GethPendingTxsIndexer;
use crate::types::GethTxPoolTx;
use rome_utils::services::{Poller, ServiceRunner};

use super::types::GethTxPoolSender;

/// Returns the next queued or pending transaction from the rollup mempool
pub async fn get_next_txn(addr: Url) -> anyhow::Result<GethTxPoolTx> {
    let payload = GethPendingTxsIndexer::generate_payload();
    let payload = serde_json::to_value(&payload).unwrap();

    let client = reqwest::Client::new();
    let addr = addr.as_str();

    let mut interval = tokio::time::interval(Duration::from_millis(100));

    loop {
        let res = Poller::poll_step(&client, addr, &payload).await?;

        if let Some(pending_txs) = GethPendingTxsIndexer::get_txs(res) {
            if let Some(tx) = pending_txs.into_iter().next() {
                return Ok(tx);
            }
        }

        interval.tick().await;
    }
}

/// Subscribe to the rollup mempool
pub async fn subscribe_to_rollup(
    addr: Url,
    poll_interval_ms: u64,
    rollup_tx: GethTxPoolSender,
) -> anyhow::Result<()> {
    let indexer = GethPendingTxsIndexer {
        geth_http_addr: addr,
        poll_interval_ms,
    };

    let service_runner = ServiceRunner::default();
    indexer.listen(rollup_tx, service_runner).await
}
