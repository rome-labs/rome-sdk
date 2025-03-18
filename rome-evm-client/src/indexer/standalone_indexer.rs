use crate::error::RomeEvmError::Custom;
use crate::indexer::{
    BlockProducer, EthereumBlockStorage, RollupIndexer, SolanaBlockLoader, SolanaBlockStorage,
};
use rome_solana::types::AsyncAtomicRpcClient;
use solana_program::clock::Slot;
use solana_program::pubkey::Pubkey;
use solana_sdk::commitment_config::CommitmentLevel;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const BLOCK_RETRIES: usize = 10;
const TX_RETRIES: usize = 10;
const RETRY_INTERVAL: Duration = Duration::from_secs(10);

// Implementation of indexer which includes SolanaBlockLoader and rollup indexer
// No additional services needed (e. g. no rome-solana-relayer)
pub struct StandaloneIndexer<
    S: SolanaBlockStorage + 'static,
    E: EthereumBlockStorage + 'static,
    B: BlockProducer + 'static,
> {
    pub solana_client: AsyncAtomicRpcClient,
    pub commitment_level: CommitmentLevel,
    pub rome_evm_pubkey: Pubkey,
    pub solana_block_storage: Arc<S>,
    pub ethereum_block_storage: Arc<E>,
    pub block_producer: B,
}

impl<
        S: SolanaBlockStorage + 'static,
        E: EthereumBlockStorage + 'static,
        B: BlockProducer + 'static,
    > StandaloneIndexer<S, E, B>
{
    pub fn start_indexing(
        self,
        start_slot: Option<Slot>,
        idx_started_oneshot: Option<oneshot::Sender<()>>,
        indexing_interval_ms: u64,
        max_slot_history: Option<Slot>,
        block_loader_batch_size: Slot,
    ) -> JoinHandle<()> {
        let (reorg_event_tx, reorg_event_rx) = tokio::sync::mpsc::unbounded_channel();
        let solana_block_loader = SolanaBlockLoader {
            solana_block_storage: self.solana_block_storage.clone(),
            client: self.solana_client,
            commitment: self.commitment_level,
            program_id: self.rome_evm_pubkey,
            reorg_event_tx,
            batch_size: block_loader_batch_size,
            block_retries: BLOCK_RETRIES,
            tx_retries: TX_RETRIES,
            retry_int: RETRY_INTERVAL,
        };

        let indexer = RollupIndexer::new(
            self.rome_evm_pubkey,
            self.solana_block_storage.clone(),
            self.ethereum_block_storage.clone(),
            self.block_producer,
            max_slot_history,
        );

        tokio::spawn(async move {
            let block_loader_jh = solana_block_loader.start_loading(
                start_slot,
                indexing_interval_ms,
                idx_started_oneshot,
            );

            let indexer_jh = indexer.start(start_slot, indexing_interval_ms, reorg_event_rx);

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

    pub fn start_recovery(
        self,
        start_slot: Slot,
        end_slot: Option<Slot>,
        block_loader_batch_size: Slot,
    ) -> JoinHandle<()> {
        let (reorg_event_tx, mut reorg_event_rx) = tokio::sync::mpsc::unbounded_channel();
        let solana_block_loader = SolanaBlockLoader {
            solana_block_storage: self.solana_block_storage.clone(),
            client: self.solana_client,
            commitment: self.commitment_level,
            program_id: self.rome_evm_pubkey,
            reorg_event_tx,
            batch_size: block_loader_batch_size,
            block_retries: BLOCK_RETRIES,
            tx_retries: TX_RETRIES,
            retry_int: RETRY_INTERVAL,
        };

        let ethereum_block_storage = self.ethereum_block_storage.clone();
        tokio::spawn(async move {
            let check_jh = solana_block_loader.start_checking(start_slot, end_slot);

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
