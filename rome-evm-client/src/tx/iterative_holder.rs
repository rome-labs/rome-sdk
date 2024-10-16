use rome_solana::batch::{AdvanceTx, IxExecStepBatch, OwnedAtomicIxBatch};
use rome_utils::holder::Holder;
use solana_program::pubkey::Pubkey;

use super::{
    builder::TxBuilder,
    utils::{ro_lock_overrides, MULTIPLE_ITERATIONS},
    TransmitTx,
};
use crate::error::{ProgramResult, RomeEvmError};
use async_trait::async_trait;
use emulator::Emulation;
use ethers::types::{Bytes, TxHash};

pub struct IterativeTxHolder {
    transmit_tx: TransmitTx,
    step: Steps,
}

enum Steps {
    Transmit,
    Execute,
    Complete,
}
impl IterativeTxHolder {
    pub fn new(tx_builder: TxBuilder, holder: Holder, rlp: Bytes, hash: TxHash) -> Self {
        let transmit_tx = TransmitTx::new(tx_builder, holder, rlp, hash);
        Self {
            transmit_tx,
            step: Steps::Transmit,
        }
    }

    fn emulation_data(&self) -> Vec<u8> {
        let mut data = vec![emulator::Instruction::DoTxHolderIterative as u8];
        data.extend(self.transmit_tx.holder.get_index().to_le_bytes());
        data.extend(self.transmit_tx.hash.as_bytes());
        data.extend(self.transmit_tx.tx_builder.chain_id.to_le_bytes());

        data
    }

    // in case of iterative instruction the emulation data and tx data are different
    fn tx_data(&self, emulation: &Emulation, unique: u64) -> ProgramResult<Vec<u8>> {
        let mut data = vec![emulator::Instruction::DoTxHolderIterative as u8];
        data.extend(unique.to_le_bytes());
        data.extend(self.transmit_tx.holder.get_index().to_le_bytes());
        data.extend(self.transmit_tx.hash.as_bytes());
        data.extend(self.transmit_tx.tx_builder.chain_id.to_le_bytes());
        let mut overrides = ro_lock_overrides(emulation)?;
        data.append(&mut overrides);

        Ok(data)
    }

    fn ixs(&self, payer: &Pubkey) -> ProgramResult<Vec<OwnedAtomicIxBatch>> {
        let data = self.emulation_data();
        let emulation = self.transmit_tx.tx_builder.emulate(&data, payer)?;

        let vm = emulation.vm.as_ref().expect("vm expected");
        let count = vm.iteration_count * MULTIPLE_ITERATIONS;

        let ixs = (0..count)
            .map(|unique| self.tx_data(&emulation, unique))
            .collect::<ProgramResult<Vec<_>>>()?
            .into_iter()
            .map(|data| self.transmit_tx.tx_builder.build_ix(&emulation, data))
            .collect();

        Ok(OwnedAtomicIxBatch::new_composible_batches_owned(ixs))
    }
}

#[async_trait]
impl AdvanceTx<'_> for IterativeTxHolder {
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
                let ixs = self.ixs(payer)?;

                Ok(IxExecStepBatch::Parallel(ixs))
            }
            _ => Ok(IxExecStepBatch::End),
        }
    }
}
