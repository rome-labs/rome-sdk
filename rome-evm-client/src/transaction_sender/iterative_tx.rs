use solana_program::pubkey::Pubkey;
use {
    super::{
        transaction_builder::TransactionBuilder,
        transaction_signer::Holder,
        utils::{ro_lock_overrides, MULTIPLE_ITERATIONS},
        RomeTx,
    },
    crate::error::Result,
    async_trait::async_trait,
    emulator::{Emulation, Instruction::DoTxIterative},
    ethers::types::Bytes,
    solana_program::instruction::Instruction,
};

pub struct IterativeTx {
    tx_builder: TransactionBuilder,
    rlp: Bytes,
    holder: Holder,
}

impl IterativeTx {
    pub fn new(tx_builder: TransactionBuilder, holder: Holder, rlp: Bytes) -> Result<Self> {
        Ok(Self {
            tx_builder,
            rlp,
            holder,
        })
    }

    fn emulation_data(&self) -> Vec<u8> {
        let mut data = vec![DoTxIterative as u8];
        data.extend(self.holder.index.to_le_bytes());
        data.extend_from_slice(self.rlp.as_ref());

        data
    }

    fn tx_data(&self, emulation: &Emulation, unique: u64) -> Result<Vec<u8>> {
        let mut overrides = ro_lock_overrides(emulation)?;
        let overrides_len = overrides.len() as u64;

        let mut data = vec![DoTxIterative as u8];
        data.extend(unique.to_le_bytes());
        data.extend(self.holder.index.to_le_bytes());
        data.extend(overrides_len.to_le_bytes());
        data.append(&mut overrides);
        data.extend_from_slice(self.rlp.as_ref());

        Ok(data)
    }
}

#[async_trait]
impl RomeTx for IterativeTx {
    fn emulate(&self, payer: &Pubkey) -> Result<Emulation> {
        let data = self.emulation_data();
        self.tx_builder.emulate(&data, payer)
    }

    fn build_transmit_tx_instructions(&self, _payer: &Pubkey) -> Result<Vec<Instruction>> {
        Ok(Vec::new())
    }

    fn build_execute_instructions(&self, payer: &Pubkey) -> Result<Vec<Instruction>> {
        let emulation = self.emulate(payer)?;
        let vm = emulation.vm.as_ref().expect("vm expected");
        let count = vm.iteration_count * MULTIPLE_ITERATIONS;
        let mut result = Vec::new();

        for unique in 0..count {
            let data = self.tx_data(&emulation, unique)?;
            result.push(self.tx_builder.build_solana_instr(&emulation, &data))
        }

        Ok(result)
    }

    fn build_commit_instruction(&self, _payer: &Pubkey) -> Result<Option<Instruction>> {
        Ok(None)
    }

    fn cross_rollup_available(&self) -> bool {
        return false;
    }
}
