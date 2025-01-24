use crate::error::ProgramResult;
use crate::indexer;
use async_trait::async_trait;
use solana_program::clock::Slot;
use solana_transaction_status::UiConfirmedBlock;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct SolanaBlockStorage {
    block_cache: Arc<RwLock<BTreeMap<Slot, Arc<UiConfirmedBlock>>>>,
}

impl SolanaBlockStorage {
    pub fn new() -> Self {
        Self {
            block_cache: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }
}

impl Default for SolanaBlockStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl indexer::SolanaBlockStorage for SolanaBlockStorage {
    async fn store_blocks(
        &self,
        blocks: BTreeMap<Slot, Arc<UiConfirmedBlock>>,
    ) -> ProgramResult<()> {
        let mut lock = self.block_cache.write().await;
        for (slot_number, block) in blocks {
            lock.insert(slot_number, block);
        }

        Ok(())
    }

    async fn get_block(&self, slot_number: Slot) -> ProgramResult<Option<Arc<UiConfirmedBlock>>> {
        Ok({
            let lock = self.block_cache.read().await;
            lock.get(&slot_number).cloned()
        })
    }

    async fn retain_from_slot(&self, from_slot: Slot) -> ProgramResult<()> {
        let mut lock = self.block_cache.write().await;
        lock.retain(|slot, _| *slot >= from_slot);
        Ok(())
    }

    async fn get_last_slot(&self) -> ProgramResult<Option<Slot>> {
        Ok(self
            .block_cache
            .read()
            .await
            .iter()
            .last()
            .map(|(slot, _)| *slot))
    }
}
