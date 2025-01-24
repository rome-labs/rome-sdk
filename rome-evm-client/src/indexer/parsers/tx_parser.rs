use crate::indexer::parsers::block_parser::TxResult;
use ethers::prelude::TxHash;
use rlp::Decodable;
use std::collections::{btree_map, HashMap};
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

pub struct TxParser {
    // Transaction Hash -> EvmTx
    transactions_by_hash: HashMap<TxHash, EvmTx>,

    // Ordered index: Solana slot -> TxIndex -> Internal Index
    transactions_by_slot: BTreeMap<Slot, BTreeMap<usize, (TxHash, Option<TxResult>)>>,
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
            transactions_by_slot: BTreeMap::new(),
        }
    }

    fn update_or_create_transaction(
        &mut self,
        tx_hash: TxHash,
        slot: Slot,
        tx_idx: usize,
        tx_result: Option<TxResult>,
        create_tx: impl FnOnce() -> EvmTx,
        update_tx: impl FnOnce(&mut EvmTx) -> ProgramResult<()>,
    ) -> ProgramResult<()> {
        update_tx(
            self.transactions_by_hash
                .entry(tx_hash)
                .or_insert_with(create_tx),
        )?;

        if tx_result.is_some()
            && self.transactions_by_slot.iter().any(|(_, slot_results)| {
                slot_results
                    .iter()
                    .any(|(_, (hash, tx_result))| tx_hash.eq(hash) && tx_result.is_some())
            })
        {
            return Err(Custom(format!(
                "Transaction {:?} already has registered result in slot {:?}",
                tx_hash, slot,
            )));
        }

        match self
            .transactions_by_slot
            .entry(slot)
            .or_default()
            .entry(tx_idx)
        {
            btree_map::Entry::Vacant(entry) => {
                entry.insert((tx_hash, tx_result));
            }
            btree_map::Entry::Occupied(mut entry) => {
                let (ex_tx_hash, ex_tx_result) = entry.get_mut();
                if !tx_hash.eq(ex_tx_hash) {
                    tracing::warn!(
                        "Transaction {tx_idx} on slot {slot} \
                        is going to be replaced by new one: {:?} -> {:?}",
                        ex_tx_hash,
                        tx_hash
                    );
                    *ex_tx_hash = tx_hash;
                }

                if !tx_result.eq(&ex_tx_result) {
                    tracing::warn!(
                        "Transaction result {tx_idx} on slot {slot} \
                        is going to be replaced by new one: {:?} -> {:?}",
                        ex_tx_result,
                        tx_result,
                    );
                    *ex_tx_result = tx_result;
                }
            }
        }

        Ok(())
    }

    pub fn register_small_tx(
        &mut self,
        slot_number: Slot,
        tx_idx: usize,
        tx: Transaction,
        tx_result: Option<TxResult>,
    ) -> ProgramResult<()> {
        self.update_or_create_transaction(
            tx.hash,
            slot_number,
            tx_idx,
            tx_result,
            || EvmTx::new_small_tx(tx),
            |_| Ok(()),
        )
    }

    pub fn register_big_tx(
        &mut self,
        slot_number: Slot,
        tx_idx: usize,
        tx_hash: TxHash,
        tx_result: Option<TxResult>,
    ) -> ProgramResult<()> {
        self.update_or_create_transaction(
            tx_hash,
            slot_number,
            tx_idx,
            tx_result,
            EvmTx::new_big_tx,
            |_| Ok(()),
        )
    }

    pub fn register_transmit_tx(
        &mut self,
        slot_number: Slot,
        tx_idx: usize,
        tx_hash: TxHash,
        offset: usize,
        data: Vec<u8>,
    ) -> ProgramResult<()> {
        self.update_or_create_transaction(
            tx_hash,
            slot_number,
            tx_idx,
            None,
            EvmTx::new_big_tx,
            |tx| tx.update_tx_holder(offset, &data),
        )
    }

    pub fn is_slot_complete(&self, slot_number: Slot) -> bool {
        if let Some(slot_txs) = self.transactions_by_slot.get(&slot_number) {
            for (tx_hash, tx_result) in slot_txs.values() {
                if tx_result.is_some() {
                    if let Some(tx) = self.transactions_by_hash.get(tx_hash) {
                        if !tx.is_data_complete() {
                            return false;
                        }
                    } else {
                        panic!("Transaction {:?} is referenced from slot {:?} but not found in TxParser", tx_hash, slot_number);
                    }
                }
            }
        }

        true
    }

    pub fn finalize_slot(&self, slot_number: Slot) -> ProgramResult<Vec<(Transaction, TxResult)>> {
        let mut results = Vec::new();
        if let Some(slot_txs) = self.transactions_by_slot.get(&slot_number) {
            for (tx_hash, tx_result) in slot_txs.values() {
                if let Some(tx_result) = tx_result {
                    if let Some(tx) = self.transactions_by_hash.get(tx_hash) {
                        if tx.is_data_complete() {
                            results.push((tx.build()?, tx_result.clone()));
                        } else {
                            return Err(Custom(format!("Tx {:?} data is not complete ", tx_hash)));
                        }
                    } else {
                        return Err(Custom(format!("Tx {:?} not found by hash", tx_hash)));
                    }
                }
            }
        }

        Ok(results)
    }

    pub fn retain_from_slot(&mut self, from_slot: Slot) {
        for (_, txs) in self.transactions_by_slot.range(..from_slot) {
            for (tx_hash, _) in txs.values() {
                self.transactions_by_hash.remove(tx_hash);
            }
        }

        self.transactions_by_slot
            .retain(|slot, _| *slot >= from_slot);
    }
}

#[cfg(test)]
mod test {
    use crate::indexer::parsers::tx_parser::TxParser;
    use crate::indexer::test::{create_big_tx, create_simple_tx, create_wallet};
    use ethers::types::{Address, U64};
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
        tx_parser
            .register_small_tx(slot_number, 0, tx.clone(), Some(tx_result.clone()))
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

        // 10 execute iterations
        for i in 0..10 {
            // Only 4-th iteration has a result
            let tx_result = if i == 4 {
                Some(tx_result.clone())
            } else {
                None
            };

            tx_parser
                .register_small_tx(slot_number, i, tx.clone(), tx_result)
                .unwrap();
        }

        // Finalization should present in execute slot
        assert!(tx_parser.is_slot_complete(slot_number));
        let chain_txs = tx_parser.finalize_slot(slot_number).unwrap();
        assert_eq!(chain_txs.len(), 1);
        assert!(chain_txs[0].eq(&(tx, tx_result)));
    }

    #[tokio::test]
    async fn test_failed_to_register_block_in_different_slot() {
        let wallet = create_wallet();
        let gas_recipient = Address::random();
        let mut tx_parser = TxParser::new();
        let slot_number: Slot = 13;

        let (tx, tx_result) =
            create_simple_tx(Some(U64::from(CHAIN_ID)), &wallet, Some(gas_recipient)).await;
        tx_parser
            .register_small_tx(slot_number, 0, tx.clone(), Some(tx_result.clone()))
            .unwrap();

        assert!(tx_parser.is_slot_complete(slot_number));
        let chain_txs = tx_parser.finalize_slot(slot_number).unwrap();
        assert_eq!(chain_txs.len(), 1);
        assert!(chain_txs[0].eq(&(tx, tx_result)));
    }

    #[tokio::test]
    async fn test_failed_register_same_txid_in_slot() {
        let wallet = create_wallet();
        let gas_recipient = Address::random();
        let mut tx_parser = TxParser::new();
        let slot_number: Slot = 13;

        let (tx, tx_result) =
            create_simple_tx(Some(U64::from(CHAIN_ID)), &wallet, Some(gas_recipient)).await;
        tx_parser
            .register_small_tx(slot_number, 0, tx.clone(), Some(tx_result.clone()))
            .unwrap();

        assert!(tx_parser
            .register_small_tx(slot_number, 0, tx.clone(), Some(tx_result))
            .is_err());
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

        // Fill transaction data
        while !rlp.is_empty() {
            let rest = rlp.split_off(std::cmp::min(CHUNK_SIZE, rlp.len()));
            tx_parser
                .register_transmit_tx(transmit_slot_number, offset, tx_hash, offset, rlp.clone())
                .unwrap();
            offset += rlp.len();
            rlp = rest;
        }

        // Register execute from holder
        tx_parser
            .register_big_tx(execute_slot_number, 0, tx_hash, Some(tx_result.clone()))
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

        while !rlp.is_empty() {
            let rest = rlp.split_off(std::cmp::min(CHUNK_SIZE, rlp.len()));
            if offset > 0 {
                // Skip first piece of data
                tx_parser
                    .register_transmit_tx(
                        transmit_slot_number,
                        offset,
                        tx_hash,
                        offset,
                        rlp.clone(),
                    )
                    .unwrap();
            }

            offset += rlp.len();
            rlp = rest;
        }

        tx_parser
            .register_big_tx(execute_slot_number, 0, tx_hash, Some(tx_result.clone()))
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

        // Fill transaction data
        while !rlp.is_empty() {
            let rest = rlp.split_off(std::cmp::min(CHUNK_SIZE, rlp.len()));
            tx_parser
                .register_transmit_tx(transmit_slot_number, offset, tx_hash, offset, rlp.clone())
                .unwrap();
            offset += rlp.len();
            rlp = rest;
        }

        // 10 execute iterations
        for i in 0..10 {
            // Only 4-th iteration has a result
            let tx_result = if i == 4 {
                Some(tx_result.clone())
            } else {
                None
            };

            tx_parser
                .register_big_tx(execute_slot_number, i, tx_hash, tx_result)
                .unwrap();
        }

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
    async fn test_failed_to_register_two_results_in_one_block() {
        let wallet = create_wallet();
        let gas_recipient = Address::random();
        let mut tx_parser = TxParser::new();
        let slot_number: Slot = 13;
        let (tx, tx_result) =
            create_simple_tx(Some(U64::from(CHAIN_ID)), &wallet, Some(gas_recipient)).await;

        tx_parser
            .register_small_tx(slot_number, 0, tx.clone(), Some(tx_result.clone()))
            .unwrap();

        assert!(tx_parser
            .register_small_tx(slot_number, 1, tx.clone(), Some(tx_result),)
            .is_err());
    }
}
