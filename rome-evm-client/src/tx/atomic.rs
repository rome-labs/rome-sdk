use {
    super::builder::TxBuilder,
    crate::{
        error::{ProgramResult, RomeEvmError},
        Resource,
    },
    async_trait::async_trait,
    emulator::Emulation,
    rome_solana::batch::{AdvanceTx, IxExecStepBatch, OwnedAtomicIxBatch, TxVersion},
    solana_sdk::signature::Keypair,
    std::sync::Arc,
};

enum Steps {
    Execute,
    End,
}
/// A simple atomic transaction
pub struct AtomicTx {
    pub tx_builder: TxBuilder,
    rlp: Vec<u8>,
    pub ix: Option<OwnedAtomicIxBatch>,
    pub emulation: Option<Emulation>,
    step: Steps,
    pub resource: Arc<Resource>,
}

impl AtomicTx {
    pub fn new(tx_builder: TxBuilder, rlp: Vec<u8>, resource: Arc<Resource>) -> Self {
        Self {
            tx_builder,
            rlp,
            ix: None,
            emulation: None,
            step: Steps::Execute,
            resource,
        }
    }
}

impl AtomicTx {
    pub fn tx_data(&self) -> Vec<u8> {
        let mut data = vec![emulator::Instruction::DoTx as u8];
        data.append(&mut self.resource.fee_recipient());
        data.extend_from_slice(self.rlp.as_ref());
        data
    }

    pub fn ix(&mut self) -> ProgramResult<()> {
        let data = self.tx_data();
        self.emulation = Some(self.tx_builder.emulate(&data, &self.resource.payer_key())?);

        let ix = self
            .tx_builder
            .build_ix(self.emulation.as_ref().unwrap(), data);
        self.ix = Some(OwnedAtomicIxBatch::new_composible_owned(ix));

        Ok(())
    }
}

#[macro_export]
macro_rules! advance_with_version {
    {} => {
        fn advance_with_version(&mut self, version: TxVersion) -> Result<IxExecStepBatch<'static>, Self::Error> {
            let ix = self.advance();
            match ix {
                Ok(IxExecStepBatch::Single(ix_, _)) => Ok(IxExecStepBatch::Single(ix_, version)),
                _ => ix,
            }
        }
    }
}
pub(crate) use advance_with_version;

#[async_trait]
impl AdvanceTx<'_> for AtomicTx {
    type Error = RomeEvmError;

    fn advance(&mut self) -> ProgramResult<IxExecStepBatch<'static>> {
        match &mut self.step {
            Steps::Execute => {
                if self.ix.is_none() {
                    self.ix()?;
                }
                let ix = self.ix.take().unwrap();

                self.step = Steps::End;
                Ok(IxExecStepBatch::Single(ix, TxVersion::Legacy))
            }
            _ => Ok(IxExecStepBatch::End),
        }
    }
    advance_with_version!();
    fn payer(&self) -> Arc<Keypair> {
        self.resource.payer()
    }
}
