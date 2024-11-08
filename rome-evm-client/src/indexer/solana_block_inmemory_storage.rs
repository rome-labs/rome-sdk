use crate::error::ProgramResult;
use crate::indexer::solana_block_storage::{BlockWithCommitment, SolanaBlockStorage};
use async_trait::async_trait;
use solana_program::clock::Slot;
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

#[derive(Clone)]
pub struct SolanaBlockInMemoryStorage {
    block_cache: Arc<RwLock<BTreeMap<Slot, Arc<BlockWithCommitment>>>>,
}

impl SolanaBlockInMemoryStorage {
    pub fn new() -> Self {
        Self {
            block_cache: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }
}

#[async_trait]
impl SolanaBlockStorage for SolanaBlockInMemoryStorage {
    async fn set_block(
        &self,
        slot_number: Slot,
        block: Arc<BlockWithCommitment>,
    ) -> ProgramResult<()> {
        let mut lock = self.block_cache.write()?;
        lock.insert(slot_number, block);
        Ok(())
    }

    async fn get_block(
        &self,
        slot_number: Slot,
    ) -> ProgramResult<Option<Arc<BlockWithCommitment>>> {
        Ok({
            let lock = self.block_cache.read()?;
            lock.get(&slot_number).cloned()
        })
    }

    async fn remove_blocks_before(&self, slot_number: Slot) -> ProgramResult<()> {
        let mut lock = self.block_cache.write()?;
        lock.retain(|slot, _| *slot >= slot_number);
        Ok(())
    }
}
