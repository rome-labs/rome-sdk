use crate::error::RomeEvmError::Custom;
use crate::indexer::{ProgramResult, RollupIndexer, SolanaBlockLoader};
use solana_program::clock::Slot;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

// Implementation of indexer which includes SolanaBlockLoader and rollup indexer
// No additional services needed (e. g. no rome-solana-relayer)
pub struct StandaloneIndexer {
    pub solana_block_loader: Option<SolanaBlockLoader>,
    pub rollup_indexer: RollupIndexer,
}

impl StandaloneIndexer {
    pub fn start_indexing(
        self,
        start_slot: Option<Slot>,
        idx_started_oneshot: Option<oneshot::Sender<()>>,
        indexing_interval_ms: u64,
    ) -> JoinHandle<ProgramResult<()>> {
        let (reorg_event_tx, reorg_event_rx) = tokio::sync::mpsc::unbounded_channel();

        tokio::spawn(async move {
            let indexer_jh =
                self.rollup_indexer
                    .start(start_slot, indexing_interval_ms, reorg_event_rx);

            if let Some(loader) = self.solana_block_loader {
                let block_loader_jh = loader.start_loading(
                    start_slot,
                    indexing_interval_ms,
                    idx_started_oneshot,
                    reorg_event_tx,
                );

                tokio::select! {
                    res = block_loader_jh => { res? }
                    res = indexer_jh => { res? }
                }
            } else {
                if let Some(idx_started_oneshot) = idx_started_oneshot {
                    idx_started_oneshot
                        .send(())
                        .expect("Failed to send SolanaBlockLoader started signal");
                }

                indexer_jh.await?
            }
        })
    }

    pub fn start_recovery(
        self,
        start_slot: Slot,
        end_slot: Option<Slot>,
    ) -> JoinHandle<ProgramResult<()>> {
        let (reorg_event_tx, mut reorg_event_rx) = tokio::sync::mpsc::unbounded_channel();

        let ethereum_block_storage = self.rollup_indexer.ethereum_block_storage().clone();
        tokio::spawn(async move {
            let check_jh = self.solana_block_loader.expect("").start_checking(
                start_slot,
                end_slot,
                reorg_event_tx,
            );

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
                res = check_jh => { res? }
                res = reorg_processor_jh => { res? }
            }
        })
    }
}
