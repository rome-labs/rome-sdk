use async_trait::async_trait;
use {
    crate::error::ProgramResult,
    solana_sdk::{commitment_config::CommitmentLevel, slot_hashes::Slot},
    solana_transaction_status::UiConfirmedBlock,
    std::sync::Arc,
};

pub struct BlockWithCommitment {
    #[allow(dead_code)]
    pub commitment_level: CommitmentLevel,
    pub block: UiConfirmedBlock,
}

#[async_trait]
pub trait SolanaBlockStorage: Send + Sync {
    async fn set_block(
        &self,
        slot_number: Slot,
        block: Arc<BlockWithCommitment>,
    ) -> ProgramResult<()>;

    async fn get_block(&self, slot_number: Slot)
        -> ProgramResult<Option<Arc<BlockWithCommitment>>>;

    async fn remove_blocks_before(&self, slot_slot: Slot) -> ProgramResult<()>;
}
