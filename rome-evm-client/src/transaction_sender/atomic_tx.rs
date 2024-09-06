use {
    super::{transaction_builder::TransactionBuilder, RomeTx},
    crate::error::Result,
    async_trait::async_trait,
    emulator::{Emulation, Instruction::DoTx},
    ethers::types::Bytes,
    solana_program::instruction::Instruction,
    solana_sdk::pubkey::Pubkey,
};

pub struct AtomicTx {
    tx_builder: TransactionBuilder,
    rlp: Bytes,
}

impl AtomicTx {
    pub fn new(tx_builder: TransactionBuilder, rlp: Bytes) -> Self {
        Self { tx_builder, rlp }
    }
}

#[async_trait]
impl RomeTx for AtomicTx {
    fn emulate(&self, payer: &Pubkey) -> Result<Emulation> {
        let mut data = vec![DoTx as u8];
        data.extend_from_slice(self.rlp.as_ref());
        self.tx_builder.emulate(&data, payer)
    }

    fn build_transmit_tx_instructions(&self, _payer: &Pubkey) -> Result<Vec<Instruction>> {
        Ok(Vec::new())
    }

    fn build_execute_instructions(&self, _payer: &Pubkey) -> Result<Vec<Instruction>> {
        Ok(vec![])
    }

    fn build_commit_instruction(&self, payer: &Pubkey) -> Result<Option<Instruction>> {
        let emulation = self.emulate(payer)?;
        let mut data = vec![DoTx as u8];
        data.extend_from_slice(self.rlp.as_ref());
        Ok(Some(self.tx_builder.build_solana_instr(&emulation, &data)))
    }

    fn cross_rollup_available(&self) -> bool {
        return true;
    }
}
