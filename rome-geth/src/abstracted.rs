use super::types::GethTxPoolSender;
use crate::indexers::pending_txs::GethPendingTxsIndexer;
use rome_utils::services::ServiceRunner;
use url::Url;

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
