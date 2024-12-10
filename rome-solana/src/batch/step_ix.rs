use super::AtomicIxBatch;
use solana_sdk::signer::keypair::Keypair;
use std::sync::Arc;

// An execution step in a ix batch
pub enum IxExecStepBatch<'a> {
    // Single step
    Single(AtomicIxBatch<'a>),
    // Single step with signers
    SingleWithSigners(AtomicIxBatch<'a>, Vec<Arc<Keypair>>),
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
