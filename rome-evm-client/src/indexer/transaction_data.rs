use {
    crate::{
        error::Result,
        indexer::tx_parser::{EVMStatusCode, TxParser},
    },
    ethers::types::{
        transaction::eip2718::TypedTransaction, Signature as EthSignature, Transaction,
        TransactionReceipt, TxHash, H256, U256, U64,
    },
    solana_program::clock::Slot,
    solana_sdk::signature::Signature as SolSignature,
    solana_transaction_status::UiTransactionStatusMeta,
    std::{
        cmp::Ordering,
        collections::{BTreeSet, HashSet},
    },
};

#[derive(PartialOrd, PartialEq, Eq, Clone)]
pub struct TxSignature {
    pub sol_slot_number: Slot,
    pub sol_tx_idx: usize,
    pub sol_signature: SolSignature,
}

impl Ord for TxSignature {
    fn cmp(&self, other: &Self) -> Ordering {
        if self.sol_slot_number == other.sol_slot_number {
            if self.sol_signature == other.sol_signature {
                Ordering::Equal
            } else {
                if self.sol_tx_idx > other.sol_tx_idx {
                    Ordering::Greater
                } else {
                    Ordering::Less
                }
            }
        } else if self.sol_slot_number > other.sol_slot_number {
            Ordering::Greater
        } else {
            Ordering::Less
        }
    }
}

#[derive(Clone)]
pub enum TransactionDataState {
    Parsing(TxParser),
    Completed(TransactionReceipt),
}

#[derive(Clone)]
pub struct TransactionData {
    state: TransactionDataState,
    transaction: Option<Transaction>,

    // Ordered set of signatures (ordering by sol_slot_number -> sol_tx_idx)
    sol_signatures: BTreeSet<TxSignature>,

    // Solana slot number (EVM block number) which contains this transaction
    included_in_slot: Option<Slot>,
}

impl TransactionData {
    pub fn new_small_tx(
        tx_request: &TypedTransaction,
        eth_signature: EthSignature,
    ) -> Result<Self> {
        Ok(Self {
            state: TransactionDataState::Parsing(TxParser::new_small_tx_parser(
                tx_request.clone(),
                eth_signature,
            )?),
            transaction: None,
            sol_signatures: BTreeSet::new(),
            included_in_slot: None,
        })
    }

    pub fn new_big_tx() -> Self {
        Self {
            state: TransactionDataState::Parsing(TxParser::new_big_tx_parser()),
            transaction: None,
            sol_signatures: BTreeSet::new(),
            included_in_slot: None,
        }
    }

    pub fn is_complete(&self) -> bool {
        match &self.state {
            TransactionDataState::Parsing(tx_parser) => tx_parser.is_complete(),
            TransactionDataState::Completed(_) => true,
        }
    }

    pub fn try_finalize(
        &mut self,
        log_index: U256,
        transaction_index: U64,
        block_hash: H256,
        block_number: U64,
    ) -> Result<()> {
        let new_state = if let TransactionDataState::Parsing(tx_parser) = &self.state {
            let (transaction, receipt) =
                tx_parser.build(log_index, transaction_index, block_hash, block_number)?;
            self.transaction = Some(transaction);
            TransactionDataState::Completed(receipt)
        } else {
            return Ok(());
        };

        self.state = new_state;
        Ok(())
    }

    pub fn get_transaction(&self) -> Option<&Transaction> {
        self.transaction.as_ref()
    }

    pub fn get_receipt(&self) -> Option<&TransactionReceipt> {
        match &self.state {
            TransactionDataState::Parsing(_) => None,
            TransactionDataState::Completed(receipt) => Some(receipt),
        }
    }

    pub fn update_execute_tx(
        &mut self,
        evm_tx_hash: TxHash,
        sol_slot_number: Slot,
        sol_tx_idx: usize,
        sol_signature: SolSignature,
        sol_meta: &UiTransactionStatusMeta,
    ) -> Result<Option<TransactionResult>> {
        match &mut self.state {
            TransactionDataState::Parsing(tx_parser) => {
                if self.sol_signatures.insert(TxSignature {
                    sol_tx_idx,
                    sol_slot_number,
                    sol_signature,
                }) {
                    // New signature found - parse logs from corresponding Solana transaction
                    if let Some(status_code) =
                        tx_parser.parse_transaction_logs(sol_meta, evm_tx_hash)?
                    {
                        // Finalization step parsed - transaction considered included in current slot
                        self.included_in_slot = Some(sol_slot_number);
                        return Ok(Some(TransactionResult {
                            tx_hash: evm_tx_hash,
                            status_code,
                            included_in_slot: self.included_in_slot,
                        }));
                    }
                }

                return Ok(None);
            }

            TransactionDataState::Completed(_) => Ok(None),
        }
    }

    pub fn update_transmit_tx(
        &mut self,
        sol_slot_number: Slot,
        sol_tx_idx: usize,
        sol_signature: &SolSignature,
        offset: usize,
        data: &[u8],
    ) -> Result<Option<TransactionResult>> {
        match &mut self.state {
            TransactionDataState::Parsing(tx_parser) => {
                if self.sol_signatures.insert(TxSignature {
                    sol_tx_idx,
                    sol_slot_number,
                    sol_signature: *sol_signature,
                }) {
                    tx_parser.write_trx_data_to_holder(offset, data);
                }

                return Ok(None);
            }

            TransactionDataState::Completed(_) => return Ok(None),
        }
    }

    pub fn indexed_sol_transactions(&self) -> HashSet<SolSignature> {
        self.sol_signatures
            .iter()
            .map(|sig| sig.sol_signature.clone())
            .collect()
    }
}

pub struct TransactionResult {
    pub tx_hash: TxHash,
    #[allow(dead_code)]
    pub status_code: EVMStatusCode,
    pub included_in_slot: Option<Slot>,
}
