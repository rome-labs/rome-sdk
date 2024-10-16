use super::{
    builder::TxBuilder,
    utils::{ro_lock_overrides, MULTIPLE_ITERATIONS},
};
use crate::error::{ProgramResult, RomeEvmError};
use async_trait::async_trait;
use emulator::Emulation;
use ethers::types::Bytes;
use rome_solana::batch::{AdvanceTx, IxExecStepBatch, OwnedAtomicIxBatch};
use rome_utils::holder::Holder;
use solana_program::pubkey::Pubkey;

pub struct IterativeTx {
    tx_builder: TxBuilder,
    rlp: Bytes,
    holder: Holder,
    pub ixs: Option<Vec<OwnedAtomicIxBatch>>,
    complete: bool,
}

impl IterativeTx {
    pub fn new(tx_builder: TxBuilder, holder: Holder, rlp: Bytes) -> ProgramResult<Self> {
        Ok(Self {
            tx_builder,
            rlp,
            holder,
            ixs: None,
            complete: false,
        })
    }

    fn emulation_data(&self) -> Vec<u8> {
        let mut data = vec![emulator::Instruction::DoTxIterative as u8];
        data.extend(self.holder.get_index().to_le_bytes());
        data.extend_from_slice(self.rlp.as_ref());

        data
    }

    fn tx_data(&self, emulation: &Emulation, unique: u64) -> ProgramResult<Vec<u8>> {
        let mut overrides = ro_lock_overrides(emulation)?;
        let overrides_len = overrides.len() as u64;

        let mut data = vec![emulator::Instruction::DoTxIterative as u8];
        data.extend(unique.to_le_bytes());
        data.extend(self.holder.get_index().to_le_bytes());
        data.extend(overrides_len.to_le_bytes());
        data.append(&mut overrides);
        data.extend_from_slice(self.rlp.as_ref());

        Ok(data)
    }

    pub fn ixs(&mut self, payer: &Pubkey) -> ProgramResult<()> {
        let data = self.emulation_data();
        let emulation = self.tx_builder.emulate(&data, payer)?;

        let vm = emulation.vm.as_ref().expect("vm expected");
        let count = vm.iteration_count * MULTIPLE_ITERATIONS;

        let ixs = (0..count)
            .map(|unique| self.tx_data(&emulation, unique))
            .collect::<ProgramResult<Vec<_>>>()?
            .into_iter()
            .map(|data| self.tx_builder.build_ix(&emulation, data))
            .collect();

        self.ixs = Some(OwnedAtomicIxBatch::new_composible_batches_owned(ixs));

        Ok(())
    }

    /// Take the holder and drop the rest of the transaction
    pub fn take_holder(self) -> Holder {
        self.holder
    }
}

#[async_trait]
impl AdvanceTx<'_> for IterativeTx {
    type Error = RomeEvmError;
    fn advance(&mut self, payer: &Pubkey) -> ProgramResult<IxExecStepBatch<'static>> {
        if self.complete {
            return Ok(IxExecStepBatch::End);
        }

        if self.ixs.is_none() {
            self.ixs(payer)?;
        }

        self.complete = true;
        let ixs = self.ixs.take().unwrap();

        Ok(IxExecStepBatch::Parallel(ixs))
    }
}
