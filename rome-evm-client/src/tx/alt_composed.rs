use {
    super::AltTx,
    crate::error::{ProgramResult, RomeEvmError},
    async_trait::async_trait,
    rome_solana::batch::{AdvanceTx, IxExecStepBatch, TxVersion},
    solana_sdk::signature::Keypair,
    std::sync::Arc,
};

enum Steps {
    Alt,
    Execute(TxVersion),
}
/// An atomic or iterative transaction without holder and using an address lookup table
pub struct AltComposed {
    alt_tx: AltTx,
    iterable_tx: Box<dyn AdvanceTx<'static, Error = RomeEvmError>>,
    step: Steps,
}
impl AltComposed {
    pub fn new(
        alt_tx: AltTx,
        iterable_tx: Box<dyn AdvanceTx<'static, Error = RomeEvmError>>,
    ) -> ProgramResult<Self> {
        Ok(Self {
            alt_tx,
            iterable_tx,
            step: Steps::Alt,
        })
    }
}

#[async_trait]
impl AdvanceTx<'_> for AltComposed {
    type Error = RomeEvmError;

    fn advance(&mut self) -> ProgramResult<IxExecStepBatch<'static>> {
        match &mut self.step {
            Steps::Alt => {
                let ix = self.alt_tx.advance();

                if let Ok(IxExecStepBatch::End) = ix {
                    let alt_account = self.alt_tx.alt_account_()?;
                    self.step = Steps::Execute(TxVersion::V0(vec![alt_account]));

                    self.advance()
                } else {
                    ix
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
