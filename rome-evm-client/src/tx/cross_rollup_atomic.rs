use crate::error::{ProgramResult, RomeEvmError};
use rome_solana::batch::{AdvanceTx, AtomicIxBatch, IxExecStepBatch, TxVersion};
use solana_sdk::signature::Keypair;
use std::sync::Arc;

pub struct CrossRollupTx {
    instructions: AtomicIxBatch<'static>,
    complete: bool,
    payer: Arc<Keypair>,
}

impl CrossRollupTx {
    pub fn new(instructions: AtomicIxBatch<'static>, payer: Arc<Keypair>) -> Self {
        Self {
            instructions,
            complete: false,
            payer,
        }
    }
}

#[async_trait::async_trait]
impl<'a> AdvanceTx<'a> for CrossRollupTx {
    type Error = RomeEvmError;

    fn advance(&mut self) -> ProgramResult<IxExecStepBatch<'a>> {
        if self.complete {
            return Ok(IxExecStepBatch::End);
        }

        self.complete = true;

        Ok(IxExecStepBatch::Single(
            self.instructions.clone(),
            TxVersion::Legacy,
        ))
    }

    fn advance_with_version(
        &mut self,
        _: TxVersion,
    ) -> Result<IxExecStepBatch<'static>, Self::Error> {
        unreachable!()
    }
    fn payer(&self) -> Arc<Keypair> {
        self.payer.clone()
    }
}
