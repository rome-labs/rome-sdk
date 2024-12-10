use crate::error::{ProgramResult, RomeEvmError};
use rome_solana::batch::{AdvanceTx, AtomicIxBatch, IxExecStepBatch};
use solana_sdk::signature::Keypair;
use std::sync::Arc;

pub struct CrossChainTx {
    instructions: AtomicIxBatch<'static>,
    complete: bool,
    payer: Arc<Keypair>,
    signers: Vec<Arc<Keypair>>,
}

impl CrossChainTx {
    pub fn new(
        instructions: AtomicIxBatch<'static>,
        payer: Arc<Keypair>,
        signers: Vec<Arc<Keypair>>,
    ) -> Self {
        Self {
            instructions,
            complete: false,
            payer,
            signers,
        }
    }
}

#[async_trait::async_trait]
impl<'a> AdvanceTx<'a> for CrossChainTx {
    type Error = RomeEvmError;

    fn advance(&mut self) -> ProgramResult<IxExecStepBatch<'a>> {
        if self.complete {
            return Ok(IxExecStepBatch::End);
        }

        self.complete = true;

        Ok(IxExecStepBatch::SingleWithSigners(
            self.instructions.clone(),
            self.signers.clone(),
        ))
    }

    fn payer(&self) -> Arc<Keypair> {
        self.payer.clone()
    }
}
