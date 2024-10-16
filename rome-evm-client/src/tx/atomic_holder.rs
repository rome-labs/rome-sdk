use rome_solana::batch::{AdvanceTx, IxExecStepBatch, OwnedAtomicIxBatch};
use rome_utils::holder::Holder;
use solana_program::pubkey::Pubkey;

use super::{builder::TxBuilder, TransmitTx};
use crate::error::{ProgramResult, RomeEvmError};
use async_trait::async_trait;
use ethers::types::{Bytes, TxHash};

pub struct AtomicTxHolder {
    step: Steps,
    transmit_tx: TransmitTx,
}

enum Steps {
    Transmit,
    Execute,
    Complete,
}

impl AtomicTxHolder {
    pub fn new(tx_builder: TxBuilder, holder: Holder, rlp: Bytes, hash: TxHash) -> Self {
        let transmit_tx = TransmitTx::new(tx_builder, holder, rlp, hash);
        Self {
            transmit_tx,
            step: Steps::Transmit,
        }
    }

    fn tx_data(&self) -> Vec<u8> {
        let mut data = vec![emulator::Instruction::DoTxHolder as u8];
        data.extend(self.transmit_tx.holder.get_index().to_le_bytes());
        data.extend(self.transmit_tx.hash.as_bytes());
        data.extend(self.transmit_tx.tx_builder.chain_id.to_le_bytes());

        data
    }
    fn ixs(&self, payer: &Pubkey) -> ProgramResult<OwnedAtomicIxBatch> {
        let data = self.tx_data();
        let emulation = self.transmit_tx.tx_builder.emulate(&data, payer)?;
        let ix = self.transmit_tx.tx_builder.build_ix(&emulation, data);

        Ok(OwnedAtomicIxBatch::new_composible_owned(ix))
    }
}

#[async_trait]
impl AdvanceTx<'_> for AtomicTxHolder {
    type Error = RomeEvmError;

    fn advance(&mut self, payer: &Pubkey) -> ProgramResult<IxExecStepBatch<'static>> {
        match &self.step {
            Steps::Transmit => {
                let ix = self.transmit_tx.advance(payer);

                if let Ok(IxExecStepBatch::End) = ix {
                    self.step = Steps::Execute;
                    self.advance(payer)
                } else {
                    ix
                }
            }
            Steps::Execute => {
                self.step = Steps::Complete;
                let ix = self.ixs(payer)?;

                Ok(IxExecStepBatch::Single(ix))
            }
            _ => Ok(IxExecStepBatch::End),
        }
    }
}
