use std::sync::Arc;
use solana_sdk::signer::keypair::Keypair;
use super::AtomicIxBatch;

// An execution step in a ix batch
pub enum IxExecStepBatch<'a> {
    // Single step
    Single(AtomicIxBatch<'a>),
    // Parallel steps
    Parallel(Vec<AtomicIxBatch<'a>>),
    // Parallel steps with check tx state
    ParallelUnchecked(Vec<AtomicIxBatch<'a>>),
    // confirmation of iterative tx
    ConfirmationIterativeTx(bool),
    // End of the execution steps
    End,
}

/// A set of [IxBatch] executed in steps
// pub trait StepIxIterable<'a>: Send + Sync {
//     type Error: std::fmt::Debug;
//
//     fn step(&self, index: usize, payer: &Pubkey) -> Result<IxExecStepBatch<'a>, Self::Error>;
// }

pub trait AdvanceTx<'a>: Send + Sync {
    type Error: std::fmt::Debug;
    fn advance(&mut self) -> Result<IxExecStepBatch<'a>, Self::Error>;
    fn payer(&self) -> Arc<Keypair>;
}
