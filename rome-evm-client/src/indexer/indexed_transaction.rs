use crate::indexer::tx_parser::GasReport;
use ethers::prelude::{Transaction, TransactionReceipt};
use solana_program::clock::Slot;
use solana_sdk::signature::Signature as SolSignature;
use std::collections::HashSet;

pub trait IndexedTransaction: Send + Sync {
    fn sol_signatures(&self) -> HashSet<SolSignature>;
    fn final_slot(&self) -> Option<Slot>;
    fn gas_report(&self) -> Option<GasReport>;
    fn eth_transaction(&self) -> Option<Transaction>;
    fn eth_receipt(&self) -> Option<TransactionReceipt>;
}
