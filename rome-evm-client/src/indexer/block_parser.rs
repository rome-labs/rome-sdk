use {
    crate::{
        error::{Result, RomeEvmError::*},
        indexer::{
            ethereum_block_storage::BlockData,
            solana_block_storage::{BlockWithCommitment, SolanaBlockStorage},
            transaction_data::TransactionResult,
            transaction_storage::TransactionStorage,
            tx_parser::decode_transaction_from_rlp,
        },
    },
    emulator::instruction::Instruction::*,
    ethers::types::{TxHash, H256, U256, U64},
    rlp::Rlp,
    rome_evm::api::{
        do_tx_holder_iterative::args as holder_iterative_args,
        do_tx_iterative::args as iterative_args,
    },
    solana_program::instruction::CompiledInstruction,
    solana_sdk::pubkey::Pubkey,
    solana_sdk::{
        hash::Hash as SolanaHash, signature::Signature as SolSignature, slot_history::Slot,
    },
    solana_transaction_status::{UiConfirmedBlock, UiTransactionStatusMeta},
    std::{mem::size_of, str::FromStr, sync::Arc},
};

#[derive(Debug)]
struct InstructionData<'a> {
    sol_tx: &'a SolSignature,
    instr: &'a CompiledInstruction,
    #[allow(dead_code)]
    instr_idx: usize,
    meta: &'a UiTransactionStatusMeta,
}

fn parse_solana_block<F>(block: &UiConfirmedBlock, program_id: &Pubkey, mut instr_parser: F)
where
    F: FnMut(usize, InstructionData),
{
    if let Some(transactions) = &block.transactions {
        transactions
            .iter()
            .enumerate()
            .map(|(tx_idx, transaction)| {
                (
                    tx_idx,
                    transaction.transaction.decode(),
                    transaction.meta.as_ref(),
                )
            })
            .for_each(|(tx_idx, transaction, meta)| {
                if let (Some(transaction), Some(meta)) = (transaction, meta) {
                    let accounts = transaction.message.static_account_keys();
                    transaction
                        .message
                        .instructions()
                        .iter()
                        .enumerate()
                        .filter(|(_, instruction)| {
                            accounts[instruction.program_id_index as usize] == *program_id
                        })
                        .for_each(|(instr_idx, instr)| {
                            instr_parser(
                                tx_idx,
                                InstructionData {
                                    sol_tx: &transaction.signatures[0],
                                    instr,
                                    instr_idx,
                                    meta,
                                },
                            )
                        })
                }
            });
    }
}

pub fn cvt_solana_block_hash(block_hash: &String) -> Result<H256> {
    Ok(H256::from(
        SolanaHash::from_str(block_hash.as_str())
            .map_err(|_| InternalError)?
            .to_bytes(),
    ))
}

pub struct BlockParser<'a> {
    solana_block_storage: SolanaBlockStorage,
    transaction_storage: &'a mut TransactionStorage,
}

impl<'a> BlockParser<'a> {
    pub fn new(
        solana_block_storage: SolanaBlockStorage,
        transaction_storage: &'a mut TransactionStorage,
    ) -> Self {
        BlockParser {
            solana_block_storage,
            transaction_storage,
        }
    }

    fn process_do_tx(
        &mut self,
        slot_number: Slot,
        tx_idx: usize,
        sol_tx: &SolSignature,
        instr_data: &[u8],
        meta: &UiTransactionStatusMeta,
    ) -> Result<Option<TransactionResult>> {
        let (tx_request, eth_signature) = decode_transaction_from_rlp(&Rlp::new(instr_data))?;
        self.transaction_storage.register_small_tx(
            slot_number,
            tx_idx,
            sol_tx,
            tx_request,
            eth_signature,
            meta,
        )
    }

    fn process_do_tx_iterative(
        &mut self,
        slot_number: Slot,
        tx_idx: usize,
        sol_tx: &SolSignature,
        instr_data: &[u8],
        meta: &UiTransactionStatusMeta,
    ) -> Result<Option<TransactionResult>> {
        let (_, _, tx) = iterative_args(instr_data)?;
        let (tx_request, eth_signature) = decode_transaction_from_rlp(&Rlp::new(tx))?;
        self.transaction_storage.register_small_tx(
            slot_number,
            tx_idx,
            sol_tx,
            tx_request,
            eth_signature,
            meta,
        )
    }

    fn process_do_tx_holder(
        &mut self,
        slot_number: Slot,
        tx_idx: usize,
        sol_tx: &SolSignature,
        instr_data: &[u8],
        meta: &UiTransactionStatusMeta,
    ) -> Result<Option<TransactionResult>> {
        if instr_data.len() < size_of::<u64>() + size_of::<rome_evm::H256>() {
            tracing::warn!(
                "Sol Tx {:?}: EVM DoTxHolder instruction data has incorrect length",
                sol_tx
            );
            return Err(InternalError);
        }

        let (_, txn_hash) = instr_data.split_at(8);
        let txn_hash = TxHash::from_slice(txn_hash);
        self.transaction_storage
            .register_big_tx(txn_hash, slot_number, tx_idx, sol_tx, meta)
    }

    fn process_do_tx_holder_iterative(
        &mut self,
        slot_number: Slot,
        tx_idx: usize,
        sol_tx: &SolSignature,
        instr_data: &[u8],
        meta: &UiTransactionStatusMeta,
    ) -> Result<Option<TransactionResult>> {
        let (_, txn_hash, _) = holder_iterative_args(instr_data)?;
        self.transaction_storage.register_big_tx(
            TxHash::from_slice(txn_hash.as_bytes()),
            slot_number,
            tx_idx,
            sol_tx,
            meta,
        )
    }

    fn process_transmit_tx(
        &mut self,
        slot_number: Slot,
        tx_idx: usize,
        sol_tx: &SolSignature,
        instr_data: &[u8],
        meta: &UiTransactionStatusMeta,
    ) -> Result<Option<TransactionResult>> {
        if instr_data.len() < size_of::<u64>() + size_of::<u64>() + size_of::<rome_evm::H256>() {
            tracing::warn!(
                "Sol Tx {:?}: EVM TransferTx instruction data is too short",
                sol_tx
            );
            return Err(InternalError);
        }

        let (_, rest) = instr_data.split_at(8);
        let (holder_offset, rest) = rest.split_at(8);
        let holder_offset: usize = usize::from_le_bytes(holder_offset.try_into().unwrap()); // TODO remove unwrap
        let (txn_hash, data) = rest.split_at(32);
        let txn_hash = TxHash::from_slice(txn_hash);

        self.transaction_storage.register_transmit_tx(
            txn_hash,
            slot_number,
            holder_offset,
            data,
            tx_idx,
            sol_tx,
            meta,
        )
    }

    fn register_block(
        &mut self,
        slot_number: Slot,
        program_id: &Pubkey,
        solana_block: Arc<BlockWithCommitment>,
    ) -> Vec<TxHash> {
        let mut result = Vec::new();
        parse_solana_block(&solana_block.block, program_id, |tx_idx, entry| {
            if entry.instr.data.is_empty() {
                tracing::warn!("EVM Instruction data is empty")
            }

            let instr_data = &entry.instr.data[1..];
            if let Ok(Some(tx_result)) = match entry.instr.data[0] {
                n if n == DoTx as u8 => {
                    self.process_do_tx(slot_number, tx_idx, entry.sol_tx, instr_data, entry.meta)
                }
                n if n == TransmitTx as u8 => self.process_transmit_tx(
                    slot_number,
                    tx_idx,
                    entry.sol_tx,
                    instr_data,
                    entry.meta,
                ),
                n if n == DoTxHolder as u8 => self.process_do_tx_holder(
                    slot_number,
                    tx_idx,
                    entry.sol_tx,
                    instr_data,
                    entry.meta,
                ),
                n if n == DoTxIterative as u8 => self.process_do_tx_iterative(
                    slot_number,
                    tx_idx,
                    entry.sol_tx,
                    instr_data,
                    entry.meta,
                ),
                n if n == DoTxHolderIterative as u8 => self.process_do_tx_holder_iterative(
                    slot_number,
                    tx_idx,
                    entry.sol_tx,
                    instr_data,
                    entry.meta,
                ),
                _ => Ok(None),
            } {
                // Parsing can find transactions which are parts of iterative and holder
                // transaction sequences. Only those transactions are considered included in
                // current slot which finalization step executed during current slot
                if let Some(included_in_slot) = tx_result.included_in_slot {
                    if included_in_slot == slot_number {
                        result.push(tx_result.tx_hash);
                    }
                }
            }
        });

        result
    }

    // Helper closure checking completeness of transactions found in requested block
    fn should_continue(&self, transactions: &Vec<TxHash>) -> bool {
        if transactions.is_empty() {
            return false;
        }

        for tx_hash in transactions {
            if let Some(tx_data) = self.transaction_storage.get_transaction(tx_hash) {
                return !tx_data.is_complete();
            }
        }

        true
    }

    pub fn parse(
        &mut self,
        solana_slot_number: Slot,
        program_id: &Pubkey,
        max_blocks: usize,
    ) -> Result<Option<Arc<BlockData>>> {
        if let Some(mut current_block) = self.solana_block_storage.get_block(solana_slot_number)? {
            let mut current_slot = solana_slot_number;
            let block_hash = cvt_solana_block_hash(&current_block.block.blockhash)?;
            let parent_hash = cvt_solana_block_hash(&current_block.block.previous_blockhash)?;
            let block_number = U64::from(solana_slot_number);
            let timestamp = current_block
                .block
                .block_time
                .map(U256::from)
                .unwrap_or_default();

            // Parsing stage 1 - transactions preparation.
            // On that stage, parser finds transactions which finished in requested solana block
            // and prepares tx builder for each transaction. That means, these transactions should
            // be included into corresponding EVM block.
            let transactions = self.register_block(current_slot, program_id, current_block.clone());

            // Parsing stage 2 - reading previous blocks to find all iterations and holder
            // accounts data
            let mut blocks_parsed: usize = 0;
            while blocks_parsed <= max_blocks && self.should_continue(&transactions) {
                current_slot = current_block.block.parent_slot;
                current_block = self
                    .solana_block_storage
                    .get_block(current_block.block.parent_slot)?
                    .ok_or(InternalError)?;

                self.register_block(current_slot, program_id, current_block.clone());
                blocks_parsed += 1;
            }

            // Calculating block gas and preparing transaction receipts
            let mut log_index = U256::zero();
            let mut gas_used = U256::zero();
            let transactions = transactions
                .into_iter()
                .enumerate()
                .filter_map(|(tx_idx, tx_hash)| {
                    if let Ok(Some(receipt)) = self.transaction_storage.try_finalize_transaction(
                        &tx_hash,
                        log_index,
                        tx_idx,
                        block_hash,
                        block_number,
                        gas_used,
                    ) {
                        // Update block gas and log index from confirmed transaction
                        log_index = log_index
                            .checked_add(U256::from(receipt.logs.len()))
                            .unwrap();
                        gas_used = gas_used
                            .checked_add(receipt.gas_used.expect("Unable to get transaction gas"))
                            .expect("This must never happen - block gas overflow!");
                        Some(tx_hash)
                    } else {
                        None
                    }
                })
                .collect();

            Ok(Some(Arc::new(BlockData::new(
                block_hash,
                parent_hash,
                block_number,
                timestamp,
                gas_used,
                transactions,
            ))))
        } else {
            Ok(None)
        }
    }
}
