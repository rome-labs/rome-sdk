use crate::error::{ProgramResult, RomeEvmError};
use crate::indexer::{
    block_parser::BlockParser, ethereum_block_storage::BlockData,
    solana_block_storage::SolanaBlockStorage, transaction_data::TransactionData,
    transaction_storage::TransactionStorage,
};
use ethers::types::{TxHash, U64};
use rome_solana::types::AsyncAtomicRpcClient;
use solana_sdk::{
    clock::Slot,
    commitment_config::{CommitmentConfig, CommitmentLevel},
    pubkey::Pubkey,
};
use std::sync::Arc;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::{oneshot, RwLock};
use tokio::time::Duration;

pub type BlockSender = UnboundedSender<Arc<BlockData>>;
pub type BlockReceiver = UnboundedReceiver<Arc<BlockData>>;

#[derive(Clone)]
pub struct Indexer {
    program_id: Pubkey,
    solana_block_storage: SolanaBlockStorage,
    transaction_storage: Arc<RwLock<TransactionStorage>>,
    commitment: CommitmentLevel,
    client: AsyncAtomicRpcClient,
    chain_id: u64,
}

impl Indexer {
    pub fn new(
        program_id: Pubkey,
        client: AsyncAtomicRpcClient,
        commitment: CommitmentLevel,
        chain_id: u64,
    ) -> Self {
        let solana_block_storage = SolanaBlockStorage::new(client.clone(), commitment);
        let transaction_storage = Arc::new(RwLock::new(TransactionStorage::new()));

        Self {
            program_id,
            solana_block_storage,
            transaction_storage,
            commitment,
            client,
            chain_id,
        }
    }

    pub async fn map_tx<Ret>(
        &self,
        tx_hash: TxHash,
        processor: impl FnOnce(&TransactionData) -> Option<Ret>,
    ) -> ProgramResult<Option<Ret>> {
        let result = if let Some(tx_data) = self
            .transaction_storage
            .read()
            .await
            .get_transaction(&tx_hash)
        {
            processor(tx_data)
        } else {
            None
        };

        Ok(result)
    }

    pub fn get_transaction_storage(&self) -> Arc<RwLock<TransactionStorage>> {
        self.transaction_storage.clone()
    }

    async fn process_blocks(
        &self,
        from_slot: u64,
        to_slot: u64,
        block_sender: &BlockSender,
    ) -> u64 {
        let mut current_slot = from_slot;
        while current_slot < to_slot {
            if let Err(err) = self
                .parse_block(U64::from(current_slot), block_sender)
                .await
            {
                tracing::warn!(
                    "Unable to parse block from slot {:?}: {:?}. Will retry",
                    current_slot,
                    err,
                );
                return current_slot;
            } else {
                current_slot += 1;
            }
        }

        to_slot
    }

    async fn process_blocks_until_in_sync(
        &self,
        mut from_slot: Slot,
        interval: Duration,
        block_sender: BlockSender,
    ) -> Slot {
        let mut interval = tokio::time::interval(interval);

        loop {
            let current_slot = self
                .client
                .get_slot_with_commitment(CommitmentConfig {
                    commitment: self.commitment,
                })
                .await;

            from_slot = match current_slot {
                Ok(to_slot) if from_slot < to_slot => {
                    tracing::debug!("Processing blocks from {:?} to {:?}", from_slot, to_slot);
                    self.process_blocks(from_slot, to_slot, &block_sender).await
                }
                Ok(to_slot) if from_slot == to_slot => break from_slot,
                Ok(to_slot) => {
                    let sleep_for_ms = (from_slot - to_slot) * 400;
                    tracing::info!("Indexer ahead sleeping for {sleep_for_ms}");
                    tokio::time::sleep(Duration::from_millis(sleep_for_ms)).await;
                    from_slot
                }
                Err(err) => {
                    tracing::warn!(
                        "Unable to get latest {:?} slot from Solana: {:?}",
                        self.commitment,
                        err
                    );
                    from_slot
                }
            };

            interval.tick().await;
        }
    }

    pub async fn start(
        &self,
        start_slot: Slot,
        interval_ms: u64,
        block_sender: BlockSender,
        idx_started_tx: Option<oneshot::Sender<()>>,
    ) {
        let duration = tokio::time::Duration::from_millis(interval_ms);
        let mut from_slot = start_slot;

        tracing::info!("Indexer starting from slot: {:?}", from_slot);

        from_slot = self
            .process_blocks_until_in_sync(from_slot, duration, block_sender.clone())
            .await;

        if let Some(idx_started_tx) = idx_started_tx {
            idx_started_tx
                .send(())
                .expect("Failed to send indexer started signal");
        }

        loop {
            from_slot = self
                .process_blocks_until_in_sync(from_slot, duration, block_sender.clone())
                .await;
        }
    }

    pub async fn parse_block(
        &self,
        block_number: U64,
        block_sender: &BlockSender,
    ) -> ProgramResult<Option<Arc<BlockData>>> {
        // Block not found in the cache - parse from Solana and save to cache
        let mut lock = self.transaction_storage.write().await;

        if let Some(block) =
            BlockParser::new(self.solana_block_storage.clone(), &mut lock, self.chain_id)
                .parse(Slot::from(block_number.as_u64()), &self.program_id, 32)
                .await?
        {
            block_sender
                .send(block.clone())
                .map_err(|_| RomeEvmError::TokioSendError)?;
            Ok(Some(block))
        } else {
            Ok(None)
        }
    }
}
