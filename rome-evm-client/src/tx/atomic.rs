use super::builder::TxBuilder;
use crate::error::{ProgramResult, RomeEvmError};
use async_trait::async_trait;
use emulator::Emulation;
use rome_solana::batch::{AdvanceTx, IxExecStepBatch, OwnedAtomicIxBatch};
use solana_sdk::pubkey::Pubkey;
use std::borrow::Cow;
// use super::AdvanceTx;

/// A simple atomic transaction
pub struct AtomicTx<'a> {
    tx_builder: TxBuilder,
    rlp: Cow<'a, [u8]>,
    pub ix: Option<OwnedAtomicIxBatch>,
    pub emulation: Option<Emulation>,
    complete: bool,
}

impl AtomicTx<'static> {
    /// Creates a new AtomicTx from owned [Vec<u8>].
    pub fn new_owned(tx_builder: TxBuilder, rlp: Vec<u8>) -> Self {
        Self {
            tx_builder,
            rlp: Cow::Owned(rlp),
            ix: None,
            emulation: None,
            complete: false,
        }
    }
}

impl<'a> AtomicTx<'a> {
    /// Creates a new AtomicTx from a reference to a [u8].
    pub fn new_ref(tx_builder: TxBuilder, rlp: &'a [u8]) -> Self {
        Self {
            tx_builder,
            rlp: Cow::Borrowed(rlp),
            ix: None,
            emulation: None,
            complete: false,
        }
    }

    pub fn tx_data(&self) -> Vec<u8> {
        let mut data = vec![emulator::Instruction::DoTx as u8];
        data.extend_from_slice(self.rlp.as_ref());
        data
    }

    pub fn ix(&mut self, payer: &Pubkey) -> ProgramResult<()> {
        let data = self.tx_data();
        self.emulation = Some(self.tx_builder.emulate(&data, payer)?);

        let ix = self
            .tx_builder
            .build_ix(self.emulation.as_ref().unwrap(), data);
        self.ix = Some(OwnedAtomicIxBatch::new_composible_owned(ix));

        Ok(())
    }
}

#[async_trait]
impl AdvanceTx<'_> for AtomicTx<'_> {
    type Error = RomeEvmError;

    fn advance(&mut self, payer: &Pubkey) -> ProgramResult<IxExecStepBatch<'static>> {
        if self.complete {
            return Ok(IxExecStepBatch::End);
        }

        self.complete = true;

        if self.ix.is_none() {
            self.ix(payer)?;
        }
        let ix = self.ix.take().unwrap();

        Ok(IxExecStepBatch::Single(ix))
    }
}
