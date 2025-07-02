use {
    super::AtomicIxBatch,
    solana_sdk::{address_lookup_table::AddressLookupTableAccount, signer::keypair::Keypair},
    std::sync::Arc,
};

#[derive(Clone)]
pub enum TxVersion {
    Legacy,
    V0(Vec<AddressLookupTableAccount>),
}
// An execution step in a ix batch
pub enum IxExecStepBatch<'a> {
    // Single step
    Single(AtomicIxBatch<'a>, TxVersion),
    // Single step with signers
    SingleWithSigners(AtomicIxBatch<'a>, Vec<Arc<Keypair>>),
    // Parallel steps
    Parallel(Vec<AtomicIxBatch<'a>>, TxVersion),
    // Parallel steps with check tx state
    ParallelUnchecked(Vec<AtomicIxBatch<'a>>, TxVersion),
    // confirmation of iterative tx
    ConfirmationIterativeTx(bool),
    // wait for the next slot following the specified slot
    WaitNextSlot(u64),
    // End of the execution steps
    End,
}

pub trait AdvanceTx<'a>: Send + Sync {
    type Error: std::fmt::Debug;
    fn advance(&mut self) -> Result<IxExecStepBatch<'a>, Self::Error>;
    fn advance_with_version(
        &mut self,
        version: TxVersion,
    ) -> Result<IxExecStepBatch<'a>, Self::Error>;
    fn payer(&self) -> Arc<Keypair>;
}
