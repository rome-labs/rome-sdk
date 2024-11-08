use {
    crate::{
        error::{ProgramResult, RomeEvmError::*},
        indexer::{
            ethereum_block_storage::BlockData,
            solana_block_storage::{BlockWithCommitment, SolanaBlockStorage},
            transaction_storage::{BlockBuilder, TransactionStorage},
            tx_parser::decode_transaction_from_rlp,
        },
    },
    emulator::instruction::Instruction::*,
    ethers::types::{TxHash, H256, U256, U64},
    rlp::Rlp,
    rome_evm::api::{do_tx_holder, do_tx_holder_iterative, do_tx_iterative, transmit_tx, split_fee,},
    solana_program::instruction::CompiledInstruction,
    solana_sdk::pubkey::Pubkey,
    solana_sdk::{
        hash::Hash as SolanaHash,
        signature::Signature as SolSignature,
        slot_history::Slot,
    },
    solana_transaction_status::{
        UiConfirmedBlock, UiTransactionStatusMeta,
    },
    std::{collections::LinkedList, str::FromStr, sync::Arc},
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

pub fn cvt_solana_block_hash(block_hash: &str) -> ProgramResult<H256> {
    Ok(H256::from(
        SolanaHash::from_str(block_hash)
            .map_err(|_| Custom(format!("Failed to parse Solana hash from {:?}", block_hash)))?
            .to_bytes(),
    ))
}

pub struct BlockParser<'a, S: SolanaBlockStorage, T: TransactionStorage> {
    solana_block_storage: &'a S,
    transaction_storage: &'a T,
    chain_id: u64,
}

impl<'a, S: SolanaBlockStorage, T: TransactionStorage> BlockParser<'a, S, T> {
    pub fn new(
        solana_block_storage: &'a S,
        transaction_storage: &'a T,
        chain_id: u64,
    ) -> Self {
        BlockParser {
            solana_block_storage,
            transaction_storage,
            chain_id,
        }
    }

    fn process_do_tx<'b>(
        &mut self,
        block: &mut T::BlockType<'b>,
        slot_number: Slot,
        tx_idx: usize,
        sol_tx: SolSignature,
        instr_data: &[u8],
        meta: UiTransactionStatusMeta,
    ) -> ProgramResult<()> {
        let (_, rlp) = split_fee(instr_data).map_err(|e| {
            tracing::warn!("Tx {:?} {}: ", sol_tx, e.to_string());
            e
        })?;

        let (tx, eth_sig) = decode_transaction_from_rlp(&Rlp::new(rlp))?;

        let chain_id = tx
            .chain_id()
            .ok_or(NoChainId)
            .map_err(|e| {
                tracing::warn!("Tx {:?} {}: ", sol_tx, e.to_string());
                e
            })?
            .as_u64();

        if chain_id != self.chain_id {
            return Ok(());
        }

        block.register_small_tx(slot_number, tx_idx, sol_tx, tx, eth_sig, meta);
        Ok(())
    }

    fn process_do_tx_iterative<'b>(
        &mut self,
        block: &mut T::BlockType<'b>,
        slot_number: Slot,
        tx_idx: usize,
        sol_tx: SolSignature,
        instr_data: &[u8],
        meta: UiTransactionStatusMeta,
    ) -> ProgramResult<()> {
        let (_, _, _, _, rlp) = do_tx_iterative::args(instr_data).map_err(|e| {
            tracing::warn!("Tx {:?} {}: ", sol_tx, e.to_string());
            e
        })?;

        let (tx, eth_sig) = decode_transaction_from_rlp(&Rlp::new(rlp))?;
        let chain_id = tx
            .chain_id()
            .ok_or(NoChainId)
            .map_err(|e| {
                tracing::warn!("Tx {:?} {}: ", sol_tx, e.to_string());
                e
            })?
            .as_u64();

        if chain_id != self.chain_id {
            return Ok(());
        }

        block.register_small_tx(slot_number, tx_idx, sol_tx, tx, eth_sig, meta);
        Ok(())
    }

    fn process_do_tx_holder<'b>(
        &mut self,
        block: &mut T::BlockType<'b>,
        slot_number: Slot,
        tx_idx: usize,
        sol_tx: SolSignature,
        instr_data: &[u8],
        meta: UiTransactionStatusMeta,
    ) -> ProgramResult<()> {
        let (_, hash, chain, _) = do_tx_holder::args(instr_data).map_err(|e| {
            tracing::warn!("Tx {:?} {}: ", sol_tx, e.to_string());
            e
        })?;

        if chain != self.chain_id {
            return Ok(());
        }

        block.register_big_tx(
            TxHash::from_slice(hash.as_bytes()),
            slot_number,
            tx_idx,
            sol_tx,
            meta,
        );
        Ok(())
    }

    fn process_do_tx_holder_iterative<'b>(
        &mut self,
        block: &mut T::BlockType<'b>,
        slot_number: Slot,
        tx_idx: usize,
        sol_tx: SolSignature,
        instr_data: &[u8],
        meta: UiTransactionStatusMeta,
    ) -> ProgramResult<()> {
        let (_, _, hash, chain, _, _) = do_tx_holder_iterative::args(instr_data).map_err(|e| {
            tracing::warn!("Tx {:?} {}: ", sol_tx, e.to_string());
            e
        })?;

        if chain != self.chain_id {
            return Ok(());
        }

        block.register_big_tx(
            TxHash::from_slice(hash.as_bytes()),
            slot_number,
            tx_idx,
            sol_tx,
            meta,
        );
        Ok(())
    }

    fn process_transmit_tx<'b>(
        &mut self,
        block: &mut T::BlockType<'b>,
        slot_number: Slot,
        tx_idx: usize,
        sol_tx: SolSignature,
        instr_data: &[u8],
        meta: UiTransactionStatusMeta,
    ) -> ProgramResult<()> {
        let (_, offset, hash, chain, chunk) = transmit_tx::args(instr_data).map_err(|e| {
            tracing::warn!("Tx {:?} {}: ", sol_tx, e.to_string());
            e
        })?;

        if chain != self.chain_id {
            return Ok(());
        }

        block.register_transmit_tx(
            TxHash::from_slice(hash.as_bytes()),
            slot_number,
            offset,
            chunk.to_vec(),
            tx_idx,
            sol_tx,
            meta,
        );
        Ok(())
    }

    async fn register_block(
        &mut self,
        slot_number: Slot,
        program_id: &Pubkey,
        solana_block: Arc<BlockWithCommitment>,
    ) -> ProgramResult<LinkedList<TxHash>> {
        let mut block = self.transaction_storage.start_block().await?;
        parse_solana_block(&solana_block.block, program_id, |tx_idx, entry| {
            if entry.instr.data.is_empty() {
                tracing::warn!("EVM Instruction data is empty")
            }

            let instr_data = &entry.instr.data[1..];
            if let Err(err) = match entry.instr.data[0] {
                n if n == DoTx as u8 => self.process_do_tx(
                    &mut block,
                    slot_number,
                    tx_idx,
                    entry.sol_tx.clone(),
                    instr_data,
                    entry.meta.clone(),
                ),
                n if n == TransmitTx as u8 => self.process_transmit_tx(
                    &mut block,
                    slot_number,
                    tx_idx,
                    entry.sol_tx.clone(),
                    instr_data,
                    entry.meta.clone(),
                ),
                n if n == DoTxHolder as u8 => self.process_do_tx_holder(
                    &mut block,
                    slot_number,
                    tx_idx,
                    entry.sol_tx.clone(),
                    instr_data,
                    entry.meta.clone(),
                ),
                n if n == DoTxIterative as u8 => self.process_do_tx_iterative(
                    &mut block,
                    slot_number,
                    tx_idx,
                    entry.sol_tx.clone(),
                    instr_data,
                    entry.meta.clone(),
                ),
                n if n == DoTxHolderIterative as u8 => self.process_do_tx_holder_iterative(
                    &mut block,
                    slot_number,
                    tx_idx,
                    entry.sol_tx.clone(),
                    instr_data,
                    entry.meta.clone(),
                ),
                _ => Ok(()),
            } {
                tracing::warn!(
                    "Failed to parse Solana transaction {:?}: {:?}",
                    entry.sol_tx,
                    err
                )
            }
        });

        block.end(slot_number).await
    }

    pub async fn parse(
        mut self,
        solana_slot_number: Slot,
        program_id: &Pubkey,
        mut max_blocks: usize,
    ) -> ProgramResult<Option<Arc<BlockData>>> {
        max_blocks = std::cmp::min(max_blocks, solana_slot_number as usize);
        if let Some(mut current_block) = self.solana_block_storage.get_block(solana_slot_number).await? {
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
            let tx_hashes = self
                .register_block(current_slot, program_id, current_block.clone())
                .await?;

            // Parsing stage 2 - reading previous blocks to find all iterations and holder
            // accounts data
            let mut blocks_parsed: usize = 0;
            while blocks_parsed <= max_blocks
                && self
                    .transaction_storage
                    .should_continue_parsing(&tx_hashes)
                    .await?
            {
                current_slot = current_block.block.parent_slot;
                current_block = self
                    .solana_block_storage
                    .get_block(current_block.block.parent_slot)
                    .await?
                    .ok_or(Custom(format!(
                        "Failed to get block for parent slot {:?}",
                        current_block.block.parent_slot
                    )))?;

                self.register_block(current_slot, program_id, current_block.clone())
                    .await?;
                blocks_parsed += 1;
            }

            // Calculating block gas and preparing transaction receipts
            let mut log_index = U256::zero();
            let mut gas_used = U256::zero();
            let mut transactions = vec![];
            transactions.reserve(tx_hashes.len());

            for (tx_idx, tx_hash) in tx_hashes.iter().enumerate() {
                if let Ok(Some(receipt)) = self
                    .transaction_storage
                    .try_finalize_transaction(
                        &tx_hash,
                        log_index,
                        tx_idx,
                        block_hash,
                        block_number,
                        gas_used,
                    )
                    .await
                {
                    // Update block gas and log index from confirmed transaction
                    log_index = log_index
                        .checked_add(U256::from(receipt.logs.len()))
                        .unwrap();
                    gas_used = gas_used
                        .checked_add(receipt.gas_used.expect("Unable to get transaction gas"))
                        .expect("This must never happen - block gas overflow!");
                    transactions.push(*tx_hash);
                }
            }

            Ok(Some(Arc::new(BlockData::new(
                block_hash,
                parent_hash,
                block_number,
                timestamp,
                gas_used,
                Arc::new(transactions),
            ))))
        } else {
            Ok(None)
        }
    }
}
