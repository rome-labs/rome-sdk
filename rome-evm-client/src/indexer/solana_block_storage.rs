use std::collections::BTreeMap;
use {
    crate::error::ProgramResult, async_trait::async_trait, solana_sdk::clock::Slot,
    solana_transaction_status::UiConfirmedBlock, std::sync::Arc,
};

#[async_trait]
pub trait SolanaBlockStorage: Send + Sync {
    /// Persists multiple blocks (a batch of Solana blocks) into storage.
    async fn store_blocks(
        &self,
        blocks: BTreeMap<Slot, Arc<UiConfirmedBlock>>,
        finalized_slot: Slot,
    ) -> ProgramResult<()>;

    /// Updates blocks marked as finalized in storage
    async fn update_finalized_blocks(
        &self,
        blocks: BTreeMap<Slot, Arc<UiConfirmedBlock>>,
    ) -> ProgramResult<()>;

    /// Retrieves a specific block by its slot number.
    async fn get_block(&self, slot_number: Slot) -> ProgramResult<Option<Arc<UiConfirmedBlock>>>;

    /// Removes older blocks from storage, retaining only those from the specified slot onwards.
    /// This can save storage space by pruning historical blocks that are no longer relevant.
    async fn retain_from_slot(&self, from_slot: Slot) -> ProgramResult<()>;

    /// Fetches the slot number of the latest or most recently stored block.
    async fn get_last_slot(&self) -> ProgramResult<Option<Slot>>;

    /// Marks a specific slot as finalized.
    async fn set_finalized_slot(
        &self,
        slot: Slot,
    ) -> ProgramResult<BTreeMap<Slot, UiConfirmedBlock>>;
}
