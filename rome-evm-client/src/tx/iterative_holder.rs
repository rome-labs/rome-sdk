use rome_solana::batch::{AdvanceTx, IxExecStepBatch, OwnedAtomicIxBatch};

use super::{builder::TxBuilder, utils::MULTIPLE_ITERATIONS, TransmitTx};
use crate::{
    error::{ProgramResult, RomeEvmError},
    Resource,
};
use async_trait::async_trait;
use emulator::Emulation;
use ethers::types::{Bytes, TxHash};
use solana_sdk::signature::Keypair;
use std::sync::Arc;

pub struct IterativeTxHolder {
    transmit_tx: TransmitTx,
    step: Steps,
    session: u64,
}

enum Steps {
    Transmit,
    Execute,
    Confirm,
    Complete,
}
impl IterativeTxHolder {
    pub fn new(tx_builder: TxBuilder, resource: Resource, rlp: Bytes, hash: TxHash) -> Self {
        let transmit_tx = TransmitTx::new(tx_builder, resource, rlp, hash);
        Self {
            transmit_tx,
            step: Steps::Transmit,
            session: rand::random(),
        }
    }

    fn emulation_data(&self) -> Vec<u8> {
        let mut data = vec![emulator::Instruction::DoTxHolderIterative as u8];
        data.extend(self.session.to_le_bytes());
        data.extend(self.transmit_tx.resource.holder());
        data.extend(self.transmit_tx.hash.as_bytes());
        data.extend(self.transmit_tx.tx_builder.chain_id.to_le_bytes());
        data.append(&mut self.transmit_tx.resource.fee_recipient());

        data
    }

    // in case of iterative instruction the emulation data and tx data are different
    fn tx_data(&self, emulation: &Emulation, unique: u64) -> ProgramResult<Vec<u8>> {
        let mut data = vec![emulator::Instruction::DoTxHolderIterative as u8];
        data.extend(unique.to_le_bytes());
        data.extend(self.session.to_le_bytes());
        data.extend(self.transmit_tx.resource.holder());
        data.extend(self.transmit_tx.hash.as_bytes());
        data.extend(self.transmit_tx.tx_builder.chain_id.to_le_bytes());
        data.append(&mut self.transmit_tx.resource.fee_recipient());
        data.append(&mut emulation.lock_overrides.clone());

        Ok(data)
    }

    fn ixs(&self) -> ProgramResult<Vec<OwnedAtomicIxBatch>> {
        let data = self.emulation_data();
        let emulation = self
            .transmit_tx
            .tx_builder
            .emulate(&data, &self.transmit_tx.resource.payer_key())?;

        let vm = emulation.vm.as_ref().expect("vm expected");
        let count = (vm.iteration_count as f64 * MULTIPLE_ITERATIONS) as u64;

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
    fn advance(&mut self) -> ProgramResult<IxExecStepBatch<'static>> {
        match &self.step {
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
                self.step = Steps::Confirm;
                let ixs = self.ixs()?;

                Ok(IxExecStepBatch::ParallelUnchecked(ixs))
            }
            Steps::Confirm => {
                self.step = Steps::Complete;

                match self.transmit_tx.tx_builder.confirm_tx_iterative(
                    self.transmit_tx.resource.holder_index(),
                    self.transmit_tx.hash,
                    &self.transmit_tx.resource.payer_key(),
                    self.session,
                ) {
                    Ok(confirm) => Ok(IxExecStepBatch::ConfirmationIterativeTx(confirm)),
                    Err(e) => {
                        tracing::error!(
                            "Failed to get status of iterative tx, tx_hash: {}, error: {}",
                            self.transmit_tx.hash,
                            e
                        );
                        Ok(IxExecStepBatch::ConfirmationIterativeTx(false))
                    }
                }
            }
            _ => Ok(IxExecStepBatch::End),
        }
    }
    fn payer(&self) -> Arc<Keypair> {
        self.transmit_tx.payer()
    }
}
