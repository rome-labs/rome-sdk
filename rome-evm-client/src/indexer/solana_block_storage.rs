use std::collections::BTreeMap;
use {
    crate::error::ProgramResult, async_trait::async_trait, solana_sdk::clock::Slot,
    solana_transaction_status::UiConfirmedBlock, std::sync::Arc,
};

#[async_trait]
pub trait SolanaBlockStorage: Send + Sync {
    async fn store_blocks(
        &self,
        blocks: BTreeMap<Slot, Arc<UiConfirmedBlock>>,
        finalized_slot: Slot,
    ) -> ProgramResult<()>;

    async fn update_finalized_blocks(
        &self,
        blocks: BTreeMap<Slot, Arc<UiConfirmedBlock>>,
    ) -> ProgramResult<()>;

    async fn get_block(&self, slot_number: Slot) -> ProgramResult<Option<Arc<UiConfirmedBlock>>>;

    async fn retain_from_slot(&self, from_slot: Slot) -> ProgramResult<()>;

    async fn get_last_slot(&self) -> ProgramResult<Option<Slot>>;

    async fn set_finalized_slot(
        &self,
        slot: Slot,
    ) -> ProgramResult<BTreeMap<Slot, UiConfirmedBlock>>;
}
