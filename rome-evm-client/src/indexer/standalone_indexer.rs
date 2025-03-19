use crate::error::RomeEvmError::Custom;
use crate::indexer::{RollupIndexer, SolanaBlockLoader};
use solana_program::clock::Slot;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

// Implementation of indexer which includes SolanaBlockLoader and rollup indexer
// No additional services needed (e. g. no rome-solana-relayer)
pub struct StandaloneIndexer {
    pub solana_block_loader: SolanaBlockLoader,
    pub rollup_indexer: RollupIndexer,
}

impl StandaloneIndexer {
    pub fn start_indexing(
        self,
        start_slot: Option<Slot>,
        idx_started_oneshot: Option<oneshot::Sender<()>>,
        indexing_interval_ms: u64,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let (reorg_event_tx, reorg_event_rx) = tokio::sync::mpsc::unbounded_channel();

            let block_loader_jh = self.solana_block_loader.start_loading(
                start_slot,
                indexing_interval_ms,
                idx_started_oneshot,
                reorg_event_tx,
            );

            let indexer_jh =
                self.rollup_indexer
                    .start(start_slot, indexing_interval_ms, reorg_event_rx);

            tokio::select! {
                res = block_loader_jh => {
                    tracing::error!("Block loader exited: {:?}", res)
                }
                res = indexer_jh => {
                    tracing::error!("Indexer exited: {:?}", res)
                }
            }
        })
    }

    pub fn start_recovery(self, start_slot: Slot, end_slot: Option<Slot>) -> JoinHandle<()> {
        let (reorg_event_tx, mut reorg_event_rx) = tokio::sync::mpsc::unbounded_channel();

        let ethereum_block_storage = self.rollup_indexer.ethereum_block_storage().clone();
        tokio::spawn(async move {
            let check_jh =
                self.solana_block_loader
                    .start_checking(start_slot, end_slot, reorg_event_tx);

            let reorg_processor_jh = tokio::spawn(async move {
                while let Some(updated_slot) = reorg_event_rx.recv().await {
                    if let Err(err) = ethereum_block_storage.clean_from_slot(updated_slot).await {
                        return Err(Custom(format!("clean_from_slot error: {:?}", err)));
                    } else {
                        tracing::info!(
                            "EthereumBlockStorage cleaned from slot: {:?}",
                            updated_slot
                        );
                    }
                }

                println!("reorg_processor_jh exit");
                Ok(())
            });

            tokio::select! {
                res = check_jh => {
                    tracing::error!("Block loader exited: {:?}", res)
                }
                res = reorg_processor_jh => {
                    tracing::error!("Indexer exited: {:?}", res)
                }
            }
        })
    }
}
