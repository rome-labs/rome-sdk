use {
    crate::{
        error::{Result, RomeEvmError, RomeEvmError::*},
        indexer::{
            block_parser::BlockParser, ethereum_block_storage::BlockData,
            solana_block_storage::SolanaBlockStorage, transaction_data::TransactionData,
            transaction_storage::TransactionStorage,
        },
    },
    ethers::types::{TxHash, U64},
    solana_client::rpc_client::RpcClient,
    solana_sdk::{
        clock::Slot,
        commitment_config::{CommitmentConfig, CommitmentLevel},
        pubkey::Pubkey,
    },
    std::{
        ops::DerefMut,
        sync::{Arc, RwLock},
    },
    tokio::{
        self,
        sync::mpsc::{UnboundedReceiver, UnboundedSender},
    },
};

pub type BlockSender = UnboundedSender<Arc<BlockData>>;
pub type BlockReceiver = UnboundedReceiver<Arc<BlockData>>;

#[derive(Clone)]
pub struct Indexer {
    program_id: Pubkey,
    solana_block_storage: SolanaBlockStorage,
    transaction_storage: Arc<RwLock<TransactionStorage>>,
    commitment: CommitmentLevel,
    client: Arc<RpcClient>,
}

fn parse_block(
    program_id: &Pubkey,
    block_number: U64,
    block_sender: &BlockSender,
    solana_block_storage: &SolanaBlockStorage,
    transaction_storage: &Arc<RwLock<TransactionStorage>>,
) -> Result<Option<Arc<BlockData>>> {
    // Block not found in the cache - parse from Solana and save to cache
    let mut lock = transaction_storage.write().map_err(|err| {
        tracing::warn!("Unable to get transaction_storage lock: {:?}", err);
        InternalError
    })?;

    if let Some(block) = BlockParser::new(solana_block_storage.clone(), lock.deref_mut()).parse(
        Slot::from(block_number.as_u64()),
        &program_id,
        32,
    )? {
        block_sender
            .send(block.clone())
            .map_err(|_| RomeEvmError::TokioSendError)?;
        Ok(Some(block))
    } else {
        Ok(None)
    }
}

impl Indexer {
    pub fn new(program_id: Pubkey, client: Arc<RpcClient>, commitment: CommitmentLevel) -> Self {
        let solana_block_storage = SolanaBlockStorage::new(client.clone(), commitment);
        let transaction_storage = Arc::new(RwLock::new(TransactionStorage::new()));

        Self {
            program_id,
            solana_block_storage,
            transaction_storage,
            commitment,
            client,
        }
    }

    pub fn map_tx<Ret>(
        &self,
        tx_hash: TxHash,
        processor: impl FnOnce(&TransactionData) -> Option<Ret>,
    ) -> Result<Option<Ret>> {
        let result =
            if let Some(tx_data) = self.transaction_storage.read()?.get_transaction(&tx_hash) {
                processor(tx_data)
            } else {
                None
            };

        Ok(result)
    }

    pub fn get_transaction_storage(&self) -> Arc<RwLock<TransactionStorage>> {
        Arc::clone(&self.transaction_storage)
    }

    fn process_blocks(&self, from_slot: u64, to_slot: u64, block_sender: &BlockSender) -> u64 {
        let mut current_slot = from_slot;
        while current_slot < to_slot {
            if let Err(err) = parse_block(
                &self.program_id,
                U64::from(current_slot),
                block_sender,
                &self.solana_block_storage,
                &self.transaction_storage,
            ) {
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
        interval: &mut tokio::time::Interval,
        block_sender: &BlockSender,
    ) -> Slot {
        loop {
            from_slot = match self.client.get_slot_with_commitment(CommitmentConfig {
                commitment: self.commitment,
            }) {
                Ok(to_slot) => {
                    if from_slot > to_slot {
                        interval.tick().await;
                        continue;
                    } else if from_slot == to_slot {
                        interval.tick().await;
                        return from_slot;
                    }

                    self.process_blocks(from_slot, to_slot, block_sender)
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

    pub async fn start<T: FnOnce()>(
        &self,
        start_slot: Slot,
        interval_ms: u64,
        block_sender: &BlockSender,
        on_started: T,
    ) {
        let duration = tokio::time::Duration::from_millis(interval_ms);
        let mut interval = tokio::time::interval(duration);
        let mut from_slot = start_slot;

        from_slot = self
            .process_blocks_until_in_sync(from_slot, &mut interval, block_sender)
            .await;
        on_started();
        loop {
            from_slot = self
                .process_blocks_until_in_sync(from_slot, &mut interval, block_sender)
                .await;
        }
    }
}
