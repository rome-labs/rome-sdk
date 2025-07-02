use crate::error::RomeEvmError::Custom;
use crate::indexer::{ProgramResult, RollupIndexer, SolanaBlockLoader};
use solana_program::clock::Slot;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

// Implementation of indexer which includes SolanaBlockLoader and rollup indexer
// No additional services needed (e. g. no rome-solana-relayer)
pub struct StandaloneIndexer {
    pub solana_block_loader: Option<SolanaBlockLoader>,
    pub rollup_indexer: Option<RollupIndexer>,
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
            let mut futures = vec![];
            let mut future_names = vec![];

            if let Some(rollup_indexer) = self.rollup_indexer {
                future_names.push("RollupIndexer");
                futures.push(rollup_indexer.start(
                    start_slot,
                    indexing_interval_ms,
                    reorg_event_rx,
                ));
            }

            if let Some(block_loader) = self.solana_block_loader {
                future_names.push("SolanaBlockLoader");
                futures.push(block_loader.start_loading(
                    start_slot,
                    indexing_interval_ms,
                    idx_started_oneshot,
                    reorg_event_tx,
                ));
            } else {
                if let Some(idx_started_oneshot) = idx_started_oneshot {
                    idx_started_oneshot
                        .send(())
                        .expect("Failed to send SolanaBlockLoader started signal");
                }
            }

            for (res, future_name) in futures::future::join_all(futures)
                .await
                .into_iter()
                .zip(future_names)
            {
                match res {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => return Err(err),
                    Err(err) => {
                        return Err(Custom(format!(
                            "Failed to await on {:?}: {:?}",
                            future_name, err
                        )))
                    }
                }
            }

            Ok(())
        })
    }

    pub fn start_recovery(
        self,
        start_slot: Slot,
        end_slot: Option<Slot>,
    ) -> JoinHandle<ProgramResult<()>> {
        if let Some(rollup_indexer) = self.rollup_indexer {
            let (reorg_event_tx, mut reorg_event_rx) = tokio::sync::mpsc::unbounded_channel();

            let ethereum_block_storage = rollup_indexer.ethereum_block_storage().clone();
            tokio::spawn(async move {
                let check_jh = self
                    .solana_block_loader
                    .expect("SolanaBlockLoader must be configured for recovery")
                    .start_checking(start_slot, end_slot, reorg_event_tx);

                let reorg_processor_jh = tokio::spawn(async move {
                    while let Some(updated_slot) = reorg_event_rx.recv().await {
                        if let Err(err) = ethereum_block_storage.clean_from_slot(updated_slot).await
                        {
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
        } else {
            panic!("No rollup indexer configured")
        }
    }
}
