use crate::indexer::parsers::block_parser::TxResult;
use ethers::prelude::TxHash;
use rlp::Decodable;
use std::collections::HashMap;
use {
    crate::error::{ProgramResult, RomeEvmError::Custom},
    ethers::types::Transaction,
    rlp::Rlp,
    solana_sdk::clock::Slot,
    std::collections::BTreeMap,
};

pub fn decode_transaction_from_rlp(rlp: &Rlp) -> ProgramResult<Transaction> {
    let mut tx = Transaction::decode(rlp)?;
    tx.from = tx.recover_from()?;
    Ok(tx)
}

// Helper enum accumulating data from Solana iterative and write holder
// instructions to build resulting Eth-like transaction
#[derive(Clone, Debug)]
pub enum EvmTx {
    SmallEvmTx {
        transaction: Transaction,
    },
    BigEvmTx {
        holder_data: Vec<u8>,
        pieces: BTreeMap<usize, usize>,
    },
}

impl EvmTx {
    pub fn new_small_tx(transaction: Transaction) -> Self {
        Self::SmallEvmTx { transaction }
    }

    pub fn new_big_tx() -> Self {
        Self::BigEvmTx {
            holder_data: Vec::new(),
            pieces: BTreeMap::new(),
        }
    }

    pub fn update_tx_holder(&mut self, offset: usize, data: &[u8]) -> ProgramResult<()> {
        if let Self::BigEvmTx {
            ref mut holder_data,
            ref mut pieces,
        } = self
        {
            if pieces.get(&offset).is_some() {
                // Duplicate data chunk - skipping. Possible on forks
                return Ok(());
            }

            if holder_data.len() < offset + data.len() {
                holder_data.resize(offset + data.len(), 0);
            };

            holder_data.as_mut_slice()[offset..][..data.len()].copy_from_slice(data);
            pieces.insert(offset, data.len());
            Ok(())
        } else {
            Err(Custom("Unable to set holder data because transaction is not supposed to have holder account".to_string()))
        }
    }

    fn is_data_complete(&self) -> bool {
        match self {
            EvmTx::SmallEvmTx { .. } => true,
            EvmTx::BigEvmTx { pieces, .. } => {
                // check that all found pieces are forming uninterrupted buffer
                let mut position: usize = 0;
                for (offset, length) in pieces.iter() {
                    if *offset != position {
                        return false;
                    }

                    position += length;
                }

                position != 0
            }
        }
    }

    pub fn build(&self) -> ProgramResult<Transaction> {
        match self {
            EvmTx::SmallEvmTx { transaction } => Ok(transaction.clone()),
            EvmTx::BigEvmTx { holder_data, .. } => {
                decode_transaction_from_rlp(&Rlp::new(holder_data.as_slice()))
            }
        }
    }
}

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

pub struct TxParser {
    // Transaction Hash -> EvmTx
    transactions_by_hash: HashMap<TxHash, EvmTx>,

    // Ordered index: Solana slot -> TxHash -> (Tx Index, TxResult)
    results_by_slot: BTreeMap<Slot, HashMap<TxHash, BTreeMap<usize, Option<TxResult>>>>,
}

impl Default for TxParser {
    fn default() -> Self {
        Self::new()
    }
}

impl TxParser {
    pub fn new() -> Self {
        Self {
            transactions_by_hash: HashMap::new(),
            results_by_slot: BTreeMap::new(),
        }
    }

    pub fn register_slot_transactions(
        &mut self,
        slot_number: Slot,
        slot_txs: ParsedTxs,
    ) -> ProgramResult<()> {
        // 1-st remove non-existing transactions and update changed results
        if let Some(existing_slot_txs) = self.results_by_slot.get_mut(&slot_number) {
            existing_slot_txs.retain(|ex_tx_hash, ex_tx_results| {
                if let Some(new_tx_steps) = slot_txs.get(ex_tx_hash) {
                    // Remove tx results which are absent in new data and update those which were changed
                    ex_tx_results.retain(|ex_tx_idx, ex_tx_result| {
                        if let Some(new_tx_step) = new_tx_steps.get(ex_tx_idx) {
                            let new_tx_result = new_tx_step.result();
                            if !new_tx_result.eq(ex_tx_result) {
                                tracing::warn!(
                                    "Transaction {:?} result on slot {slot_number} \
                                    is going to be replaced by new one: {:?} -> {:?}",
                                    ex_tx_hash,
                                    ex_tx_result,
                                    new_tx_result,
                                );

                                *ex_tx_result = new_tx_result;
                            }

                            true // retain tx result
                        } else {
                            tracing::warn!(
                                "Transaction {:?} result removed from slot {:?} on tx idx {:?}",
                                ex_tx_hash,
                                slot_number,
                                ex_tx_idx
                            );

                            false // remove tx result
                        }
                    });

                    true // retain tx results
                } else {
                    tracing::info!(
                        "Transaction result {:?} removed from slot {:?}",
                        ex_tx_hash,
                        slot_number
                    );
                    false // remove all tx results from this slot
                }
            })
        }

        for (tx_hash, tx_steps) in slot_txs {
            for (tx_idx, tx_step) in tx_steps {
                let tx_result = tx_step.result();

                match tx_step {
                    ParsedTxStep::SmallTx { tx, .. } => {
                        self.transactions_by_hash
                            .entry(tx_hash)
                            .or_insert_with(|| EvmTx::new_small_tx(tx));
                    }

                    ParsedTxStep::BigTx { .. } => {
                        self.transactions_by_hash
                            .entry(tx_hash)
                            .or_insert_with(EvmTx::new_big_tx);
                    }

                    ParsedTxStep::TransmitTx { offset, data, .. } => {
                        self.transactions_by_hash
                            .entry(tx_hash)
                            .or_insert_with(EvmTx::new_big_tx)
                            .update_tx_holder(offset, &data)?;
                    }
                };

                // Add new tx results which were not found previously
                self.results_by_slot
                    .entry(slot_number)
                    .or_default()
                    .entry(tx_hash)
                    .or_default()
                    .entry(tx_idx)
                    .or_insert(tx_result);
            }
        }

        Ok(())
    }

    pub fn is_slot_complete(&self, slot_number: Slot) -> bool {
        if let Some(slot_results) = self.results_by_slot.get(&slot_number) {
            for (tx_hash, tx_results) in slot_results {
                let mut slot_complete = true;
                for tx_result in tx_results.values() {
                    if tx_result.is_some() {
                        if let Some(tx) = self.transactions_by_hash.get(tx_hash) {
                            slot_complete &= tx.is_data_complete();
                        } else {
                            panic!("Transaction {:?} is referenced from slot {:?} but not found in TxParser", tx_hash, slot_number);
                        }
                    }
                }

                if !slot_complete {
                    return false;
                }
            }
        }

        true
    }

    pub fn finalize_slot(&self, slot_number: Slot) -> ProgramResult<Vec<(Transaction, TxResult)>> {
        let mut results = BTreeMap::new();
        if let Some(slot_txs) = self.results_by_slot.get(&slot_number) {
            for (tx_hash, tx_results) in slot_txs {
                for (tx_idx, tx_result) in tx_results {
                    if let Some(tx_result) = tx_result {
                        if let Some(tx) = self.transactions_by_hash.get(tx_hash) {
                            if tx.is_data_complete() {
                                results.insert(*tx_idx, (tx.build()?, tx_result.clone()));
                                break; // stop iterating over results of current tx_hash
                            } else {
                                return Err(Custom(format!(
                                    "Tx {:?} data is not complete ",
                                    tx_hash
                                )));
                            }
                        } else {
                            return Err(Custom(format!("Tx {:?} not found by hash", tx_hash)));
                        }
                    }
                }
            }
        }

        Ok(results.into_values().collect())
    }

    pub fn retain_from_slot(&mut self, from_slot: Slot) {
        for (_, txs) in self.results_by_slot.range(..from_slot) {
            for tx_hash in txs.keys() {
                self.transactions_by_hash.remove(tx_hash);
            }
        }

        self.results_by_slot.retain(|slot, _| *slot >= from_slot);
    }
}

#[cfg(test)]
mod test {
    use crate::indexer::parsers::block_parser::GasReport;
    use crate::indexer::parsers::log_parser::ExitReason;
    use crate::indexer::parsers::tx_parser::{ParsedTxStep, ParsedTxs, TxParser};
    use crate::indexer::test::{create_big_tx, create_simple_tx, create_wallet};
    use crate::indexer::TxResult;
    use ethers::types::{Address, U256, U64};
    use solana_program::clock::Slot;
    use tokio;

    const CHAIN_ID: u64 = 100;

    #[tokio::test]
    async fn test_register_atomic() {
        let wallet = create_wallet();
        let gas_recipient = Address::random();
        let mut tx_parser = TxParser::new();
        let slot_number: Slot = 13;

        let (tx, tx_result) =
            create_simple_tx(Some(U64::from(CHAIN_ID)), &wallet, Some(gas_recipient)).await;

        let mut slot_txs = ParsedTxs::new();
        slot_txs
            .entry(tx.hash)
            .or_default()
            .entry(0)
            .or_insert(ParsedTxStep::SmallTx {
                tx: tx.clone(),
                tx_result: Some(tx_result.clone()),
            });

        tx_parser
            .register_slot_transactions(slot_number, slot_txs)
            .unwrap();

        assert!(tx_parser.is_slot_complete(slot_number));
        let chain_txs = tx_parser.finalize_slot(slot_number).unwrap();
        assert_eq!(chain_txs.len(), 1);
        assert!(chain_txs[0].eq(&(tx, tx_result)));
    }

    #[tokio::test]
    async fn test_register_iterative() {
        let wallet = create_wallet();
        let gas_recipient = Address::random();
        let mut tx_parser = TxParser::new();
        let slot_number: Slot = 13;
        let (tx, tx_result) =
            create_simple_tx(Some(U64::from(CHAIN_ID)), &wallet, Some(gas_recipient)).await;

        let mut slot_txs = ParsedTxs::new();
        // 10 execute iterations
        for i in 0..10 {
            // Only 4-th iteration has a result
            let tx_result = if i == 4 {
                Some(tx_result.clone())
            } else {
                None
            };

            slot_txs
                .entry(tx.hash)
                .or_default()
                .entry(i)
                .or_insert(ParsedTxStep::SmallTx {
                    tx: tx.clone(),
                    tx_result,
                });
        }

        tx_parser
            .register_slot_transactions(slot_number, slot_txs)
            .unwrap();

        // Finalization should present in execute slot
        assert!(tx_parser.is_slot_complete(slot_number));
        let chain_txs = tx_parser.finalize_slot(slot_number).unwrap();
        assert_eq!(chain_txs.len(), 1);
        assert!(chain_txs[0].eq(&(tx, tx_result)));
    }

    #[tokio::test]
    async fn test_replace_transaction_result() {
        let wallet = create_wallet();
        let gas_recipient = Address::random();
        let mut tx_parser = TxParser::new();
        let slot_number: Slot = 13;

        let (tx, tx_result) =
            create_simple_tx(Some(U64::from(CHAIN_ID)), &wallet, Some(gas_recipient)).await;

        let mut slot_txs = ParsedTxs::new();
        slot_txs
            .entry(tx.hash)
            .or_default()
            .entry(0)
            .or_insert(ParsedTxStep::SmallTx {
                tx: tx.clone(),
                tx_result: Some(tx_result),
            });
        tx_parser
            .register_slot_transactions(slot_number, slot_txs.clone())
            .unwrap();

        let mut slot_txs = ParsedTxs::new();
        let tx_result = TxResult {
            exit_reason: ExitReason {
                code: 0,
                reason: "Failure".to_string(),
                return_value: vec![],
            },
            logs: vec![],
            gas_report: GasReport {
                gas_value: U256::from(22000),
                gas_recipient: None,
            },
        };

        slot_txs
            .entry(tx.hash)
            .or_default()
            .entry(0)
            .or_insert(ParsedTxStep::SmallTx {
                tx: tx.clone(),
                tx_result: Some(tx_result.clone()),
            });

        tx_parser
            .register_slot_transactions(slot_number, slot_txs.clone())
            .unwrap();
        let finalized_txs = tx_parser.finalize_slot(slot_number).unwrap();
        assert!(finalized_txs[0].1.eq(&tx_result));
    }

    #[tokio::test]
    async fn test_register_atomic_holder() {
        let wallet = create_wallet();
        let gas_recipient = Address::random();
        let mut tx_parser = TxParser::new();
        let transmit_slot_number: Slot = 13;
        let execute_slot_number: Slot = 14;

        let (tx, tx_result) =
            create_big_tx(Some(U64::from(CHAIN_ID)), &wallet, Some(gas_recipient)).await;
        println!("{:?}, {:?}", tx, tx_result);
        let tx_hash = tx.hash();
        let mut rlp = tx.rlp().to_vec();
        const CHUNK_SIZE: usize = 32;
        let mut offset: usize = 0;

        let mut transmit_slot_txs = ParsedTxs::new();

        // Fill transaction data
        while !rlp.is_empty() {
            let rest = rlp.split_off(std::cmp::min(CHUNK_SIZE, rlp.len()));
            transmit_slot_txs
                .entry(tx_hash)
                .or_default()
                .entry(offset)
                .or_insert(ParsedTxStep::TransmitTx {
                    offset,
                    data: rlp.clone(),
                });

            offset += rlp.len();
            rlp = rest;
        }

        // register transmit transactions
        tx_parser
            .register_slot_transactions(transmit_slot_number, transmit_slot_txs)
            .unwrap();

        let mut execute_slot_txs = ParsedTxs::new();
        execute_slot_txs
            .entry(tx_hash)
            .or_default()
            .entry(0)
            .or_insert(ParsedTxStep::BigTx {
                tx_result: Some(tx_result.clone()),
            });

        // Register execute from holder
        tx_parser
            .register_slot_transactions(execute_slot_number, execute_slot_txs)
            .unwrap();

        // Finalization should absent in transmit slot
        assert!(tx_parser.is_slot_complete(transmit_slot_number));
        let result = tx_parser.finalize_slot(transmit_slot_number).unwrap();
        assert!(result.is_empty());

        // Finalization should present in execute slot
        assert!(tx_parser.is_slot_complete(execute_slot_number));
        let chain_txs = tx_parser.finalize_slot(execute_slot_number).unwrap();

        assert_eq!(chain_txs.len(), 1);
        println!("{:?}", chain_txs);
        assert!(chain_txs[0].eq(&(tx, tx_result)));
    }

    #[tokio::test]
    async fn test_failed_to_finalize_not_completed_tx() {
        let wallet = create_wallet();
        let gas_recipient = Address::random();
        let mut tx_parser = TxParser::new();
        let transmit_slot_number: Slot = 13;
        let execute_slot_number: Slot = 14;

        let (tx, tx_result) =
            create_big_tx(Some(U64::from(CHAIN_ID)), &wallet, Some(gas_recipient)).await;
        let tx_hash = tx.hash();
        let mut rlp = tx.rlp().to_vec();
        const CHUNK_SIZE: usize = 32;
        let mut offset: usize = 0;

        let mut transmit_slot_txs = ParsedTxs::new();
        while !rlp.is_empty() {
            let rest = rlp.split_off(std::cmp::min(CHUNK_SIZE, rlp.len()));
            if offset > 0 {
                // Skip first piece of data
                transmit_slot_txs
                    .entry(tx_hash)
                    .or_default()
                    .entry(offset)
                    .or_insert(ParsedTxStep::TransmitTx {
                        offset,
                        data: rlp.clone(),
                    });
            }

            offset += rlp.len();
            rlp = rest;
        }

        tx_parser
            .register_slot_transactions(transmit_slot_number, transmit_slot_txs)
            .unwrap();
        let mut execute_slot_txs = ParsedTxs::new();
        execute_slot_txs
            .entry(tx_hash)
            .or_default()
            .entry(0)
            .or_insert(ParsedTxStep::BigTx {
                tx_result: Some(tx_result.clone()),
            });
        tx_parser
            .register_slot_transactions(execute_slot_number, execute_slot_txs)
            .unwrap();

        // Finalization should absent in transmit slot
        assert!(tx_parser.is_slot_complete(transmit_slot_number));
        let result = tx_parser.finalize_slot(transmit_slot_number).unwrap();
        assert!(result.is_empty());

        // Slot should be incomplete but try to finalize anyway
        assert!(!tx_parser.is_slot_complete(execute_slot_number));
        assert!(tx_parser.finalize_slot(execute_slot_number).is_err());
    }

    #[tokio::test]
    async fn test_register_iterative_holder() {
        let wallet = create_wallet();
        let gas_recipient = Address::random();
        let mut tx_parser = TxParser::new();
        let transmit_slot_number: Slot = 13;
        let execute_slot_number: Slot = 14;

        let (tx, tx_result) =
            create_big_tx(Some(U64::from(CHAIN_ID)), &wallet, Some(gas_recipient)).await;
        let tx_hash = tx.hash();
        let mut rlp = tx.rlp().to_vec();
        const CHUNK_SIZE: usize = 32;
        let mut offset: usize = 0;

        let mut transmit_slot_txs = ParsedTxs::new();
        // Fill transaction data
        while !rlp.is_empty() {
            let rest = rlp.split_off(std::cmp::min(CHUNK_SIZE, rlp.len()));
            transmit_slot_txs
                .entry(tx_hash)
                .or_default()
                .entry(offset)
                .or_insert(ParsedTxStep::TransmitTx {
                    offset,
                    data: rlp.clone(),
                });

            offset += rlp.len();
            rlp = rest;
        }

        tx_parser
            .register_slot_transactions(transmit_slot_number, transmit_slot_txs)
            .unwrap();

        let mut execute_slot_txs = ParsedTxs::new();
        // 10 execute iterations
        for tx_idx in 0..10 {
            // Only 4-th iteration has a result
            let tx_result = if tx_idx == 4 {
                Some(tx_result.clone())
            } else {
                None
            };

            execute_slot_txs
                .entry(tx_hash)
                .or_default()
                .entry(tx_idx)
                .or_insert(ParsedTxStep::BigTx { tx_result });
        }

        tx_parser
            .register_slot_transactions(execute_slot_number, execute_slot_txs)
            .unwrap();

        // Finalization should absent in transmit slot
        assert!(tx_parser.is_slot_complete(transmit_slot_number));
        let result = tx_parser.finalize_slot(transmit_slot_number).unwrap();
        assert!(result.is_empty());

        // Finalization should present in execute slot
        assert!(tx_parser.is_slot_complete(execute_slot_number));
        let chain_txs = tx_parser.finalize_slot(execute_slot_number).unwrap();
        assert_eq!(chain_txs.len(), 1);
        assert!(chain_txs[0].eq(&(tx, tx_result)));
    }

    #[tokio::test]
    async fn test_replace_result_and_tx_idx() {
        let wallet = create_wallet();
        let gas_recipient = Address::random();
        let mut tx_parser = TxParser::new();
        let slot_number: Slot = 13;
        let (tx, tx_result) =
            create_simple_tx(Some(U64::from(CHAIN_ID)), &wallet, Some(gas_recipient)).await;

        let mut slot_txs = ParsedTxs::new();
        slot_txs
            .entry(tx.hash)
            .or_default()
            .entry(0)
            .or_insert(ParsedTxStep::SmallTx {
                tx: tx.clone(),
                tx_result: Some(tx_result.clone()),
            });

        tx_parser
            .register_slot_transactions(slot_number, slot_txs)
            .unwrap();

        let mut slot_txs = ParsedTxs::new();
        let new_tx_result = TxResult {
            exit_reason: ExitReason {
                code: 0,
                reason: "Failure".to_string(),
                return_value: vec![],
            },
            logs: vec![],
            gas_report: GasReport {
                gas_value: U256::from(22000),
                gas_recipient: None,
            },
        };

        slot_txs
            .entry(tx.hash)
            .or_default()
            .entry(1)
            .or_insert(ParsedTxStep::SmallTx {
                tx: tx.clone(),
                tx_result: Some(new_tx_result.clone()),
            });

        tx_parser
            .register_slot_transactions(slot_number, slot_txs.clone())
            .unwrap();
        let finalized_txs = tx_parser.finalize_slot(slot_number).unwrap();
        assert!(finalized_txs[0].1.eq(&new_tx_result));
    }
}
