use crate::error::ProgramResult;
use crate::indexer::TxResult;
use ethers::prelude::{Transaction, H256 as TxHash};
use solana_program::clock::Slot;
use std::collections::{BTreeMap, HashMap};

#[derive(Clone)]
pub enum ParsedTxStep {
    SmallTx {
        tx: Transaction,
        tx_result: Option<TxResult>,
    },

    BigTx {
        tx_result: Option<TxResult>,
    },

    TransmitTx {
        offset: usize,
        data: Vec<u8>,
    },
}

impl ParsedTxStep {
    pub fn result(&self) -> Option<TxResult> {
        match self {
            ParsedTxStep::SmallTx { tx_result, .. } => tx_result.clone(),
            ParsedTxStep::BigTx { tx_result, .. } => tx_result.clone(),
            ParsedTxStep::TransmitTx { .. } => None,
        }
    }
}

pub type TxSteps = BTreeMap<usize, ParsedTxStep>;

pub type ParsedTxs = HashMap<TxHash, TxSteps>;

pub trait TxParser {
    fn register_slot_transactions(
        &mut self,
        slot_number: Slot,
        slot_txs: ParsedTxs,
    ) -> ProgramResult<()>;

    fn is_slot_complete(&self, slot_number: Slot) -> bool;

    fn finalize_slot(&self, slot_number: Slot) -> ProgramResult<Vec<(Transaction, TxResult)>>;

    fn retain_from_slot(&mut self, from_slot: Slot);
}
