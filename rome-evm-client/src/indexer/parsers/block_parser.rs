use crate::indexer::ethereum_block_storage::EthBlockId;
use crate::indexer::parsers::log_parser;
use crate::indexer::parsers::log_parser::LogParser;
use crate::indexer::parsers::tx_parser::{decode_transaction_from_rlp, TxParser};
use crate::indexer::{BlockParams, BlockType};
use ethers::addressbook::Address;
use ethers::prelude::{Block, Log, H256};
use ethers::types::{BigEndianHash, Transaction};
use jsonrpsee_core::Serialize;
use serde::Deserialize;
use {
    crate::{
        error::{ProgramResult, RomeEvmError::*},
        indexer::solana_block_storage::SolanaBlockStorage,
    },
    emulator::instruction::Instruction::*,
    ethers::types::{TxHash, U256},
    rlp::Rlp,
    rome_evm::api::{
        do_tx_holder, do_tx_holder_iterative, do_tx_iterative, split_fee, transmit_tx,
    },
    solana_program::{clock::UnixTimestamp, instruction::CompiledInstruction},
    solana_sdk::pubkey::Pubkey,
    solana_sdk::{signature::Signature as SolSignature, slot_history::Slot},
    solana_transaction_status::{
        option_serializer::OptionSerializer, UiConfirmedBlock, UiTransactionStatusMeta,
    },
    std::sync::Arc,
};

#[derive(Debug)]
struct InstructionData<'a> {
    sol_tx: &'a SolSignature,
    instr: &'a CompiledInstruction,
    #[allow(dead_code)]
    instr_idx: usize,
    meta: &'a UiTransactionStatusMeta,
}

fn parse_solana_block<F>(block: &UiConfirmedBlock, program_id: Pubkey, mut instr_parser: F)
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
                            accounts[instruction.program_id_index as usize] == program_id
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

#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GasReport {
    pub gas_value: U256,
    pub gas_recipient: Option<Address>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxResult {
    pub exit_reason: log_parser::ExitReason,
    pub logs: Vec<Log>,
    pub gas_report: GasReport,
}

#[derive(Clone, Debug)]
pub struct EthBlock {
    pub id: EthBlockId,

    // Values obtained from parsing stage
    pub block_gas_used: U256,
    pub transactions: Vec<TxHash>,
    pub gas_recipient: Option<Address>,
    pub slot_timestamp: Option<UnixTimestamp>,

    // Values obtained from producing stage
    pub block_params: Option<BlockParams>,
}

const EMPTY_ROOT: [u8; 32] = [
    0x56, 0xe8, 0x1f, 0x17, 0x1b, 0xcc, 0x55, 0xa6, 0xff, 0x83, 0x45, 0xe6, 0x92, 0xc0, 0xf8, 0x6e,
    0x5b, 0x48, 0xe0, 0x1b, 0x99, 0x6c, 0xad, 0xc0, 0x01, 0x62, 0x2f, 0xb5, 0xe3, 0x63, 0xb4, 0x21,
];

impl EthBlock {
    pub fn get_block_base<B>(&self) -> Block<B> {
        let tx_root = if self.transactions.is_empty() {
            H256::from(EMPTY_ROOT)
        } else {
            H256::from_uint(&U256::one())
        };

        let block_params = self.block_params.as_ref();
        BlockType::base::<B>(
            block_params.map(|p| p.hash),
            block_params
                .as_ref()
                .and_then(|p| p.parent_hash)
                .unwrap_or_default(),
            tx_root,
            block_params.as_ref().map(|p| p.number),
            self.block_gas_used,
            block_params
                .as_ref()
                .map(|p| p.timestamp)
                .unwrap_or_default(),
        )
    }
}

#[derive(Clone, Default)]
pub struct BlockParseResult {
    pub slot_number: Slot,
    pub parent_slot_number: Slot,
    pub timestamp: Option<UnixTimestamp>,
    pub transactions: Vec<(Transaction, TxResult)>,
}

impl BlockParseResult {
    fn new_eth_block(
        &self,
        block_idx: usize,
        transactions: Vec<TxHash>,
        block_gas_used: U256,
        gas_recipient: Option<Address>,
    ) -> EthBlock {
        EthBlock {
            id: (self.slot_number, block_idx),
            block_gas_used,
            transactions,
            gas_recipient,
            slot_timestamp: self.timestamp,
            block_params: None,
        }
    }

    pub fn create_eth_blocks(&self) -> Vec<EthBlock> {
        let mut block_gas_used = U256::zero();
        let mut eth_blocks = Vec::new();

        let mut block_txs = Vec::new();
        let mut gas_recipient = None;

        for (tx, tx_result) in self.transactions.iter() {
            if tx_result.gas_report.gas_recipient == gas_recipient {
                // Several transactions with the same gas recipient are included in a single block
                block_txs.push(tx.hash);
                block_gas_used += tx_result.gas_report.gas_value;
            } else {
                if block_txs.is_empty() {
                    // First transaction with gas recipient other than previous (or default which is None)
                    block_txs.push(tx.hash);
                    block_gas_used = tx_result.gas_report.gas_value;
                    gas_recipient = tx_result.gas_report.gas_recipient;
                    continue;
                }

                eth_blocks.push(self.new_eth_block(
                    eth_blocks.len(),
                    block_txs,
                    block_gas_used,
                    gas_recipient,
                ));
                block_txs = vec![tx.hash];
                block_gas_used = tx_result.gas_report.gas_value;
                gas_recipient = tx_result.gas_report.gas_recipient;
            }
        }

        if !block_txs.is_empty() {
            eth_blocks.push(self.new_eth_block(
                eth_blocks.len(),
                block_txs,
                block_gas_used,
                gas_recipient,
            ));
        }

        eth_blocks
    }
}

pub struct BlockParser<'a, S: SolanaBlockStorage> {
    solana_block_storage: &'a S,
    tx_parser: &'a mut TxParser,
    program_id: Pubkey,
    chain_id: u64,
}

impl<'a, S: SolanaBlockStorage> BlockParser<'a, S> {
    pub fn new(
        solana_block_storage: &'a S,
        tx_parser: &'a mut TxParser,
        program_id: Pubkey,
        chain_id: u64,
    ) -> Self {
        BlockParser {
            solana_block_storage,
            tx_parser,
            program_id,
            chain_id,
        }
    }

    fn parse_transaction_meta(meta: &UiTransactionStatusMeta) -> ProgramResult<Option<TxResult>> {
        Ok(match &meta.log_messages {
            OptionSerializer::Some(logs) => {
                let mut parser = LogParser::new();
                parser.parse(logs)?;
                if let Some(exit_reason) = parser.exit_reason {
                    Some(TxResult {
                        exit_reason,
                        logs: parser.events,
                        gas_report: GasReport {
                            gas_value: parser.gas_value.unwrap_or(U256::zero()),
                            gas_recipient: parser.gas_recipient,
                        },
                    })
                } else {
                    None
                }
            }
            _ => None,
        })
    }

    fn process_do_tx(
        &mut self,
        slot_number: Slot,
        tx_idx: usize,
        sol_tx: SolSignature,
        instr_data: &[u8],
        meta: &UiTransactionStatusMeta,
    ) -> ProgramResult<()> {
        let (_, rlp) = split_fee(instr_data).map_err(|e| {
            tracing::warn!("Tx {:?} {}: ", sol_tx, e.to_string());
            e
        })?;

        let tx = decode_transaction_from_rlp(&Rlp::new(rlp))?;

        let chain_id = tx
            .chain_id
            .ok_or(NoChainId)
            .map_err(|e| {
                tracing::warn!("Tx {:?} {}: ", sol_tx, e.to_string());
                e
            })?
            .as_u64();

        if chain_id != self.chain_id {
            return Ok(());
        }

        self.tx_parser.register_small_tx(
            slot_number,
            tx_idx,
            tx,
            Self::parse_transaction_meta(meta)?,
        )
    }

    fn process_do_tx_iterative(
        &mut self,
        slot_number: Slot,
        tx_idx: usize,
        sol_tx: SolSignature,
        instr_data: &[u8],
        meta: &UiTransactionStatusMeta,
    ) -> ProgramResult<()> {
        let (_, _, _, _, rlp) = do_tx_iterative::args(instr_data).map_err(|e| {
            tracing::warn!("Tx {:?} {}: ", sol_tx, e.to_string());
            e
        })?;

        let tx = decode_transaction_from_rlp(&Rlp::new(rlp))?;
        let chain_id = tx
            .chain_id
            .ok_or(NoChainId)
            .map_err(|e| {
                tracing::warn!("Tx {:?} {}: ", sol_tx, e.to_string());
                e
            })?
            .as_u64();

        if chain_id != self.chain_id {
            return Ok(());
        }

        self.tx_parser.register_small_tx(
            slot_number,
            tx_idx,
            tx,
            Self::parse_transaction_meta(meta)?,
        )
    }

    fn process_do_tx_holder(
        &mut self,
        slot_number: Slot,
        tx_idx: usize,
        sol_tx: SolSignature,
        instr_data: &[u8],
        meta: &UiTransactionStatusMeta,
    ) -> ProgramResult<()> {
        let (_, hash, chain_id, _) = do_tx_holder::args(instr_data).map_err(|e| {
            tracing::warn!("Tx {:?} {}: ", sol_tx, e.to_string());
            e
        })?;

        if chain_id != self.chain_id {
            return Ok(());
        }

        self.tx_parser.register_big_tx(
            slot_number,
            tx_idx,
            TxHash::from_slice(hash.as_bytes()),
            Self::parse_transaction_meta(meta)?,
        )
    }

    fn process_do_tx_holder_iterative(
        &mut self,
        slot_number: Slot,
        tx_idx: usize,
        sol_tx: SolSignature,
        instr_data: &[u8],
        meta: &UiTransactionStatusMeta,
    ) -> ProgramResult<()> {
        let (_, _, hash, chain_id, _, _) =
            do_tx_holder_iterative::args(instr_data).map_err(|e| {
                tracing::warn!("Tx {:?} {}: ", sol_tx, e.to_string());
                e
            })?;

        if chain_id != self.chain_id {
            return Ok(());
        }

        self.tx_parser.register_big_tx(
            slot_number,
            tx_idx,
            TxHash::from_slice(hash.as_bytes()),
            Self::parse_transaction_meta(meta)?,
        )
    }

    fn process_transmit_tx(
        &mut self,
        slot_number: Slot,
        tx_idx: usize,
        sol_tx: SolSignature,
        instr_data: &[u8],
    ) -> ProgramResult<()> {
        let (_, offset, hash, chain_id, chunk) = transmit_tx::args(instr_data).map_err(|e| {
            tracing::warn!("Tx {:?} {}: ", sol_tx, e.to_string());
            e
        })?;

        if chain_id != self.chain_id {
            return Ok(());
        }

        self.tx_parser.register_transmit_tx(
            slot_number,
            tx_idx,
            TxHash::from_slice(hash.as_bytes()),
            offset,
            chunk.to_vec(),
        )
    }

    fn register_block(&mut self, slot_number: Slot, solana_block: Arc<UiConfirmedBlock>) {
        parse_solana_block(&solana_block, self.program_id, |tx_idx, entry| {
            if entry.instr.data.is_empty() {
                tracing::warn!("EVM Instruction data is empty")
            }

            let instr_data = &entry.instr.data[1..];
            if let Err(err) = match entry.instr.data[0] {
                n if n == DoTx as u8 => {
                    self.process_do_tx(slot_number, tx_idx, *entry.sol_tx, instr_data, entry.meta)
                }
                n if n == TransmitTx as u8 => {
                    self.process_transmit_tx(slot_number, tx_idx, *entry.sol_tx, instr_data)
                }
                n if n == DoTxHolder as u8 => self.process_do_tx_holder(
                    slot_number,
                    tx_idx,
                    *entry.sol_tx,
                    instr_data,
                    entry.meta,
                ),
                n if n == DoTxIterative as u8 => self.process_do_tx_iterative(
                    slot_number,
                    tx_idx,
                    *entry.sol_tx,
                    instr_data,
                    entry.meta,
                ),
                n if n == DoTxHolderIterative as u8 => self.process_do_tx_holder_iterative(
                    slot_number,
                    tx_idx,
                    *entry.sol_tx,
                    instr_data,
                    entry.meta,
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
    }

    pub async fn get_block_with_retries(
        &self,
        slot_number: Slot,
    ) -> ProgramResult<Arc<UiConfirmedBlock>> {
        let mut num_retries = 10;

        loop {
            let block = self.solana_block_storage.get_block(slot_number).await;

            if let Ok(Some(block)) = block {
                return Ok(block);
            }

            num_retries -= 1;

            if num_retries > 0 {
                tracing::warn!("Failed to load block for slot {slot_number}, retrying...");
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            } else {
                break Err(Custom(format!(
                    "Unable to load block for slot {slot_number}"
                )));
            }
        }
    }

    pub async fn parse(
        &mut self,
        solana_slot_number: Slot,
        mut max_blocks: usize,
    ) -> ProgramResult<Option<BlockParseResult>> {
        max_blocks = std::cmp::min(max_blocks, solana_slot_number as usize);
        if let Some(mut current_block) = self
            .solana_block_storage
            .get_block(solana_slot_number)
            .await?
        {
            let parent_slot_number = current_block.parent_slot;
            let timestamp = current_block.block_time;

            let mut current_slot = solana_slot_number;
            let mut blocks_parsed: usize = 0;

            loop {
                self.register_block(current_slot, current_block.clone());
                blocks_parsed += 1;

                if blocks_parsed > max_blocks || self.tx_parser.is_slot_complete(solana_slot_number)
                {
                    break;
                }

                current_slot = current_block.parent_slot;
                current_block = self.get_block_with_retries(current_slot).await?;
            }

            Ok(Some(BlockParseResult {
                slot_number: solana_slot_number,
                parent_slot_number,
                timestamp,
                transactions: self.tx_parser.finalize_slot(solana_slot_number)?,
            }))
        } else {
            Ok(None)
        }
    }
}
