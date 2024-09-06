use {
    crate::{
        error::RomeEvmError::{CrossRollupExecutionNotAvailable, EmulationError},
        transaction_sender::utils::build_solana_tx,
        transaction_sender::RomeTx,
    },
    emulator::Emulation,
    solana_program::{hash::Hash, instruction::Instruction, pubkey::Pubkey},
    solana_sdk::{
        signature::{Keypair, Signer},
        transaction::Transaction,
    },
};

pub struct CrossRollupTx {
    rollup_txs: Vec<Box<dyn RomeTx>>,
}

impl CrossRollupTx {
    pub fn new(rollup_txs: Vec<Box<dyn RomeTx>>) -> Self {
        Self { rollup_txs }
    }
}

impl RomeTx for CrossRollupTx {
    fn emulate(&self, _payer: &Pubkey) -> crate::error::Result<Emulation> {
        Err(EmulationError(
            "Emulation is not available for CrossRollup transaction".to_string(),
        ))
    }

    fn build_transmit_tx_instructions(
        &self,
        payer: &Pubkey,
    ) -> crate::error::Result<Vec<Instruction>> {
        let mut result = Vec::new();
        for tx in &self.rollup_txs {
            result.extend_from_slice(tx.build_transmit_tx_instructions(payer)?.as_slice());
        }

        Ok(result)
    }

    fn build_execute_instructions(&self, payer: &Pubkey) -> crate::error::Result<Vec<Instruction>> {
        let mut result = Vec::new();
        for tx in &self.rollup_txs {
            result.extend_from_slice(tx.build_execute_instructions(payer)?.as_slice());
        }

        Ok(result)
    }

    fn build_commit_instruction(
        &self,
        _payer: &Pubkey,
    ) -> crate::error::Result<Option<Instruction>> {
        Ok(None)
    }

    fn cross_rollup_available(&self) -> bool {
        true
    }

    fn build_commit_transaction(
        &self,
        recent_blockhash: Hash,
        payer: &Keypair,
    ) -> crate::error::Result<Option<Transaction>> {
        let mut instructions = vec![];
        for tx in &self.rollup_txs {
            if let Some(ix) = tx.build_commit_instruction(&payer.pubkey())? {
                instructions.push(ix);
            } else {
                return Err(CrossRollupExecutionNotAvailable);
            }
        }

        Ok(Some(build_solana_tx(
            recent_blockhash,
            payer,
            instructions.as_slice(),
        )?))
    }
}
