use solana_program::pubkey::Pubkey;
use {
    super::{
        transaction_builder::TransactionBuilder,
        transaction_signer::Holder,
        utils::{transmit_tx_data, transmit_tx_list},
        RomeTx,
    },
    crate::error::Result,
    async_trait::async_trait,
    emulator::{Emulation, Instruction::*},
    ethers::types::{Bytes, TxHash},
    solana_sdk::instruction::Instruction,
};

pub struct AtomicTxHolder {
    tx_builder: TransactionBuilder,
    rlp: Bytes,
    hash: TxHash,
    holder: Holder,
}

impl<'a> AtomicTxHolder {
    pub fn new(
        tx_builder: TransactionBuilder,
        holder: Holder,
        rlp: Bytes,
        hash: TxHash,
    ) -> Result<Self> {
        Ok(Self {
            tx_builder,
            rlp,
            hash,
            holder,
        })
    }
    fn tx_data(&self) -> Vec<u8> {
        let mut data = vec![DoTxHolder as u8];
        data.extend(self.holder.index.to_le_bytes());
        data.extend(self.hash.as_bytes());

        data
    }
}

#[async_trait]
impl RomeTx for AtomicTxHolder {
    fn emulate(&self, payer: &Pubkey) -> Result<Emulation> {
        let data = self.tx_data();
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

    fn build_execute_instructions(&self, _payer: &Pubkey) -> Result<Vec<Instruction>> {
        Ok(vec![])
    }

    fn build_commit_instruction(&self, payer: &Pubkey) -> Result<Option<Instruction>> {
        let emulation = self.emulate(payer)?;
        let data = self.tx_data();
        Ok(Some(self.tx_builder.build_solana_instr(&emulation, &data)))
    }

    fn cross_rollup_available(&self) -> bool {
        return true;
    }
}
