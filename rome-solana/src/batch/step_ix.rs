use solana_sdk::pubkey::Pubkey;

use super::AtomicIxBatch;

// An execution step in a ix batch
pub enum IxExecStepBatch<'a> {
    // Single step
    Single(AtomicIxBatch<'a>),
    // Parallel steps
    Parallel(Vec<AtomicIxBatch<'a>>),
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
    fn advance(&mut self, payer: &Pubkey) -> Result<IxExecStepBatch<'a>, Self::Error>;
}
