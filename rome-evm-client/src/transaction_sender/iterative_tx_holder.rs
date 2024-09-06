use solana_program::pubkey::Pubkey;
use {
    super::{
        transaction_builder::TransactionBuilder,
        transaction_signer::Holder,
        utils::{ro_lock_overrides, transmit_tx_data, transmit_tx_list, MULTIPLE_ITERATIONS},
        RomeTx,
    },
    crate::error::Result,
    async_trait::async_trait,
    emulator::{Emulation, Instruction::*},
    ethers::types::{Bytes, TxHash},
    solana_program::instruction::Instruction,
};

pub struct IterativeTxHolder {
    tx_builder: TransactionBuilder,
    rlp: Bytes,
    holder: Holder,
    hash: TxHash,
}

impl IterativeTxHolder {
    pub fn new(
        tx_builder: TransactionBuilder,
        holder: Holder,
        rlp: Bytes,
        hash: TxHash,
    ) -> Result<Self> {
        Ok(Self {
            tx_builder,
            rlp,
            holder,
            hash,
        })
    }

    fn emulation_data(&self) -> Vec<u8> {
        let mut data = vec![DoTxHolderIterative as u8];
        data.extend(self.holder.index.to_le_bytes());
        data.extend(self.hash.as_bytes());

        data
    }

    // in case of iterative instruction the emulation data and tx data are different
    fn tx_data(&self, emulation: &Emulation, unique: u64) -> Result<Vec<u8>> {
        let mut data = vec![DoTxHolderIterative as u8];
        data.extend(unique.to_le_bytes());
        data.extend(self.holder.index.to_le_bytes());
        data.extend(self.hash.as_bytes());
        let mut overrides = ro_lock_overrides(emulation)?;
        data.append(&mut overrides);

        Ok(data)
    }
}

#[async_trait]
impl RomeTx for IterativeTxHolder {
    fn emulate(&self, payer: &Pubkey) -> Result<Emulation> {
        let data = self.emulation_data();
        self.tx_builder.emulate(&data, payer)
    }

    fn build_transmit_tx_instructions(&self, payer: &Pubkey) -> Result<Vec<Instruction>> {
        let transmit_data = transmit_tx_data(0, self.rlp.as_ref(), &self.holder, self.hash);
        let emulation_transmit = self.tx_builder.emulate(&transmit_data, payer)?;
        Ok(transmit_tx_list(
            self.rlp.as_ref(),
            &self.holder,
            self.hash,
            &self.tx_builder,
            &emulation_transmit,
        ))
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
