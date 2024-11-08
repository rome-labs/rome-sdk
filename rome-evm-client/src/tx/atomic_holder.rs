use rome_solana::batch::{AdvanceTx, IxExecStepBatch, OwnedAtomicIxBatch};

use super::{builder::TxBuilder, TransmitTx};
use crate::{
    error::{ProgramResult, RomeEvmError}, Resource
};
use async_trait::async_trait;
use ethers::types::{Bytes, TxHash};
use solana_sdk::signature::Keypair;
use std::sync::Arc;

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
    pub fn new(tx_builder: TxBuilder, resource: Resource, rlp: Bytes, hash: TxHash) -> Self {
        let transmit_tx = TransmitTx::new(tx_builder, resource, rlp, hash);
        Self {
            transmit_tx,
            step: Steps::Transmit,
        }
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
        let emulation = self.transmit_tx.tx_builder.emulate(
            &data,
            &self.transmit_tx.resource.payer_key()
        )?;
        let ix = self.transmit_tx.tx_builder.build_ix(&emulation, data);

        Ok(OwnedAtomicIxBatch::new_composible_owned(ix))
    }
}

#[async_trait]
impl AdvanceTx<'_> for AtomicTxHolder {
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
                self.step = Steps::Complete;
                let ix = self.ixs()?;

                Ok(IxExecStepBatch::Single(ix))
            }
            _ => Ok(IxExecStepBatch::End),
        }
    }
    fn payer(&self) -> Arc<Keypair> {
        self.transmit_tx.payer()
    }
}
