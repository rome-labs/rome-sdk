use {
    super::{atomic::advance_with_version, TransmitTx},
    crate::error::{ProgramResult, RomeEvmError},
    async_trait::async_trait,
    rome_solana::batch::{AdvanceTx, IxExecStepBatch, OwnedAtomicIxBatch, TxVersion},
    solana_sdk::signature::Keypair,
    std::sync::Arc,
};

enum Steps {
    Transmit,
    Execute,
    End,
}
pub struct AtomicTxHolder {
    step: Steps,
    pub transmit_tx: TransmitTx,
}

impl AtomicTxHolder {
    pub fn new(transmit_tx: TransmitTx, use_alt: bool) -> Self {
        let step = if use_alt {
            Steps::Execute
        } else {
            Steps::Transmit
        };

        Self { transmit_tx, step }
    }

    fn tx_data(&self) -> Vec<u8> {
        let mut data = vec![emulator::Instruction::DoTxHolder as u8];
        data.extend(self.transmit_tx.resource.holder());
        data.extend(self.transmit_tx.hash.as_bytes());
        data.extend(self.transmit_tx.tx_builder.chain_id.to_le_bytes());
        data.append(&mut self.transmit_tx.resource.fee_recipient());

        data
    }
    fn ixs(&self) -> ProgramResult<OwnedAtomicIxBatch> {
        let data = self.tx_data();
        let emulation = self
            .transmit_tx
            .tx_builder
            .emulate(&data, &self.transmit_tx.resource.payer_key())?;
        let ix = self.transmit_tx.tx_builder.build_ix(&emulation, data);

        Ok(OwnedAtomicIxBatch::new_composible_owned(ix))
    }
}

#[async_trait]
impl AdvanceTx<'_> for AtomicTxHolder {
    type Error = RomeEvmError;

    fn advance(&mut self) -> ProgramResult<IxExecStepBatch<'static>> {
        match &mut self.step {
            Steps::Transmit => {
                let ix = self.transmit_tx.advance();

                if let Ok(IxExecStepBatch::End) = ix {
                    self.step = Steps::Execute;
                    self.advance()
                } else {
                    ix
                }
            }
            Steps::Execute => {
                self.step = Steps::End;
                let ix = self.ixs()?;

                Ok(IxExecStepBatch::Single(ix, TxVersion::Legacy))
            }
            _ => Ok(IxExecStepBatch::End),
        }
    }
    advance_with_version!();
    fn payer(&self) -> Arc<Keypair> {
        self.transmit_tx.payer()
    }
}
