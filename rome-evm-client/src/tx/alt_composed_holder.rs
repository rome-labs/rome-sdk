use {
    super::{AltTx, TransmitTx},
    crate::error::{ProgramResult, RomeEvmError},
    async_trait::async_trait,
    rome_solana::batch::{AdvanceTx, IxExecStepBatch, TxVersion},
    solana_sdk::signature::Keypair,
    std::sync::Arc,
};

enum Steps {
    AltAndTransmit,
    Execute(TxVersion),
}
/// An atomic or iterative transaction without holder and using an address lookup table
pub struct AltComposedHolder {
    alt_tx: AltTx,
    transmit_tx: TransmitTx,
    iterable_tx: Box<dyn AdvanceTx<'static, Error = RomeEvmError>>,
    step: Steps,
}
impl AltComposedHolder {
    pub fn new(
        alt_tx: AltTx,
        transmit_tx: TransmitTx,
        iterable_tx: Box<dyn AdvanceTx<'static, Error = RomeEvmError>>,
    ) -> ProgramResult<Self> {
        Ok(Self {
            alt_tx,
            transmit_tx,
            iterable_tx,
            step: Steps::AltAndTransmit,
        })
    }
}
#[async_trait]
impl AdvanceTx<'_> for AltComposedHolder {
    type Error = RomeEvmError;

    fn advance(&mut self) -> ProgramResult<IxExecStepBatch<'static>> {
        match &mut self.step {
            Steps::AltAndTransmit => {
                let alt_ix = self.alt_tx.advance()?;
                let transmit_ix = self.transmit_tx.advance()?;
                let ix = join_parallel(alt_ix, transmit_ix)?;

                if let IxExecStepBatch::End = ix {
                    let alt_account = self.alt_tx.alt_account_()?;
                    self.step = Steps::Execute(TxVersion::V0(vec![alt_account]));
                    self.advance()
                } else {
                    Ok(ix)
                }
            }
            Steps::Execute(version) => self.iterable_tx.advance_with_version(version.clone()),
        }
    }
    fn advance_with_version(
        &mut self,
        _: TxVersion,
    ) -> Result<IxExecStepBatch<'static>, Self::Error> {
        unreachable!()
    }
    fn payer(&self) -> Arc<Keypair> {
        self.iterable_tx.payer()
    }
}

pub fn join_parallel(
    mut alt_ix: IxExecStepBatch<'static>,
    transmit_ix: IxExecStepBatch<'static>,
) -> ProgramResult<IxExecStepBatch<'static>> {
    match &transmit_ix {
        IxExecStepBatch::Parallel(batch1, _) => match &mut alt_ix {
            IxExecStepBatch::Parallel(batch2, _) => {
                batch2.append(&mut batch1.clone());
                Ok(alt_ix)
            }
            IxExecStepBatch::End | IxExecStepBatch::WaitNextSlot(_) => Ok(transmit_ix),
            _ => unreachable!(),
        },
        IxExecStepBatch::End => Ok(alt_ix),
        _ => unreachable!(),
    }
}
