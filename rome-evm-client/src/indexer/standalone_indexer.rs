use crate::indexer::{
    BlockProducer, EthereumBlockStorage, RollupIndexer, SolanaBlockLoader, SolanaBlockStorage,
};
use rome_solana::types::AsyncAtomicRpcClient;
use solana_program::clock::Slot;
use solana_program::pubkey::Pubkey;
use solana_sdk::commitment_config::CommitmentLevel;
use std::sync::Arc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

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
    ) -> JoinHandle<()> {
        let solana_block_loader = SolanaBlockLoader::new(
            self.solana_block_storage.clone(),
            self.solana_client,
            self.commitment_level,
            self.rome_evm_pubkey,
        );
        let indexer = RollupIndexer::new(
            self.rome_evm_pubkey,
            self.solana_block_storage.clone(),
            self.ethereum_block_storage.clone(),
            self.block_producer,
            max_slot_history,
        );

        tokio::spawn(async move {
            let block_loader_jh = tokio::spawn(async move {
                solana_block_loader
                    .start(start_slot, indexing_interval_ms, idx_started_oneshot)
                    .await
            });

            let indexer_jh =
                tokio::spawn(async move { indexer.start(start_slot, indexing_interval_ms).await });

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
}
