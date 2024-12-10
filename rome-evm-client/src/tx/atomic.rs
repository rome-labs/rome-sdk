use super::builder::TxBuilder;
use crate::{
    error::{ProgramResult, RomeEvmError},
    Resource,
};
use async_trait::async_trait;
use emulator::Emulation;
use rome_solana::batch::{AdvanceTx, IxExecStepBatch, OwnedAtomicIxBatch};
use solana_sdk::signature::Keypair;
use std::sync::Arc;

/// A simple atomic transaction
pub struct AtomicTx {
    tx_builder: TxBuilder,
    rlp: Vec<u8>,
    pub ix: Option<OwnedAtomicIxBatch>,
    pub emulation: Option<Emulation>,
    complete: bool,
    pub resource: Resource,
}

impl AtomicTx {
    pub fn new(tx_builder: TxBuilder, rlp: Vec<u8>, resource: Resource) -> Self {
        Self {
            tx_builder,
            rlp,
            ix: None,
            emulation: None,
            complete: false,
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

#[async_trait]
impl AdvanceTx<'_> for AtomicTx {
    type Error = RomeEvmError;

    fn advance(&mut self) -> ProgramResult<IxExecStepBatch<'static>> {
        if self.complete {
            return Ok(IxExecStepBatch::End);
        }

        self.complete = true;

        if self.ix.is_none() {
            self.ix()?;
        }
        let ix = self.ix.take().unwrap();

        Ok(IxExecStepBatch::Single(ix))
    }
    fn payer(&self) -> Arc<Keypair> {
        self.resource.payer()
    }
}
