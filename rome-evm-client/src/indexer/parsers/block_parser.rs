use crate::error::RomeEvmError;
use crate::indexer::parsers::l1_attributes::create_l1_attributes_tx;
use crate::indexer::parsers::log_parser;
use crate::indexer::parsers::log_parser::LogParser;
use crate::indexer::parsers::tx_parser::{
    decode_transaction_from_rlp, ParsedTxStep, ParsedTxs, TxParser,
};
use crate::indexer::produced_blocks::BlockParams;
use crate::indexer::BlockType;
use ethers::addressbook::Address;
use ethers::prelude::{Block, Log, H256};
use ethers::types::{BigEndianHash, Transaction};
use jsonrpsee_core::Serialize;
use serde::Deserialize;
use solana_sdk::bs58;
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
                    if meta.err.is_some() {
                        return;
                    }

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

#[derive(Default, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxResult {
    pub exit_reason: log_parser::ExitReason,
    pub logs: Vec<Log>,
    pub gas_report: GasReport,
}

#[derive(Clone, Debug)]
pub struct EthBlock {
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

#[derive(Copy, Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum BlockParseMode {
    EngineApi,
    SingleState,
}

#[derive(Clone)]
pub struct BlockParseResult {
    pub slot_number: Slot,
    pub parent_slot_number: Slot,
    pub solana_blockhash: String,
    pub parent_solana_blockhash: String,
    pub timestamp: Option<UnixTimestamp>,
    pub transactions: Vec<(Transaction, TxResult)>,
    pub parse_mode: BlockParseMode,
}

impl BlockParseResult {
    pub fn new(
        slot_number: Slot,
        solana_block: Arc<UiConfirmedBlock>,
        tx_parser: &TxParser,
        parse_mode: BlockParseMode,
    ) -> ProgramResult<Self> {
        Ok(Self {
            slot_number,
            parent_slot_number: solana_block.parent_slot,
            solana_blockhash: solana_block.blockhash.clone(),
            parent_solana_blockhash: solana_block.previous_blockhash.clone(),
            timestamp: solana_block.block_time,
            parse_mode,
            transactions: Some(create_l1_attributes_tx(
                slot_number,
                H256::from_slice(&bs58::decode(&solana_block.blockhash).into_vec().map_err(
                    |_| {
                        RomeEvmError::Custom(format!(
                            "Solana block {:?} invalid blockhash: {}",
                            slot_number, solana_block.blockhash
                        ))
                    },
                )?),
                U256::from(solana_block.block_time.ok_or(RomeEvmError::Custom(format!(
                    "Solana block {:?} timestamp is not set",
                    slot_number
                )))?),
            ))
            .into_iter()
            .chain(tx_parser.finalize_slot(slot_number)?)
            .collect(),
        })
    }

    fn new_eth_block(
        &self,
        transactions: Vec<TxHash>,
        block_gas_used: U256,
        gas_recipient: Option<Address>,
    ) -> EthBlock {
        EthBlock {
            block_gas_used,
            transactions,
            gas_recipient,
            slot_timestamp: self.timestamp,
            block_params: None,
        }
    }

    pub fn create_eth_blocks(&self) -> Vec<EthBlock> {
        match self.parse_mode {
            BlockParseMode::EngineApi => {
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
                    eth_blocks.push(self.new_eth_block(block_txs, block_gas_used, gas_recipient));
                }

                eth_blocks
            }

            BlockParseMode::SingleState => {
                let mut block_gas_used = U256::zero();
                let mut block_txs = Vec::new();

                for (tx, tx_result) in self.transactions.iter() {
                    block_txs.push(tx.hash);
                    block_gas_used += tx_result.gas_report.gas_value;
                }

                vec![self.new_eth_block(block_txs, block_gas_used, None)]
            }
        }
    }
}

struct SolanaBlockParser<'a> {
    chain_id: u64,
    parsed_txs: ParsedTxs,
    tx_parser: &'a mut TxParser,
    slot_number: Slot,
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

impl<'a> SolanaBlockParser<'a> {
    pub fn new(chain_id: u64, tx_parser: &'a mut TxParser, slot_number: Slot) -> Self {
        Self {
            chain_id,
            parsed_txs: ParsedTxs::new(),
            tx_parser,
            slot_number,
        }
    }

    pub fn process_do_tx(
        &mut self,
        tx_idx: usize,
        sol_tx: SolSignature,
        instr_data: &[u8],
        meta: &UiTransactionStatusMeta,
    ) -> ProgramResult<()> {
        let (_, rlp) = split_fee(instr_data).inspect_err(|e| {
            tracing::warn!("Tx {:?} {}: ", sol_tx, e.to_string());
        })?;

        let tx = decode_transaction_from_rlp(&Rlp::new(rlp))?;

        let chain_id = tx
            .chain_id
            .ok_or(NoChainId)
            .inspect_err(|e| tracing::warn!("Tx {:?} {}: ", sol_tx, e.to_string()))?
            .as_u64();

        if chain_id == self.chain_id {
            self.add_small_tx(tx_idx, tx, meta)?;
        }

        Ok(())
    }

    pub fn process_do_tx_iterative(
        &mut self,
        tx_idx: usize,
        sol_tx: SolSignature,
        instr_data: &[u8],
        meta: &UiTransactionStatusMeta,
    ) -> ProgramResult<()> {
        let (_, _, _, _, rlp) = do_tx_iterative::args(instr_data).inspect_err(|e| {
            tracing::warn!("Tx {:?} {}: ", sol_tx, e.to_string());
        })?;

        let tx = decode_transaction_from_rlp(&Rlp::new(rlp))?;
        let chain_id = tx
            .chain_id
            .ok_or(NoChainId)
            .inspect_err(|e| {
                tracing::warn!("Tx {:?} {}: ", sol_tx, e.to_string());
            })?
            .as_u64();

        if chain_id == self.chain_id {
            self.add_small_tx(tx_idx, tx, meta)?;
        }

        Ok(())
    }

    pub fn process_do_tx_holder(
        &mut self,
        tx_idx: usize,
        sol_tx: SolSignature,
        instr_data: &[u8],
        meta: &UiTransactionStatusMeta,
    ) -> ProgramResult<()> {
        let (_, hash, chain_id, _) = do_tx_holder::args(instr_data).inspect_err(|e| {
            tracing::warn!("Tx {:?} {}: ", sol_tx, e.to_string());
        })?;

        if chain_id == self.chain_id {
            self.add_big_tx(tx_idx, TxHash::from_slice(hash.as_bytes()), meta)?;
        }

        Ok(())
    }

    pub fn process_do_tx_holder_iterative(
        &mut self,
        tx_idx: usize,
        sol_tx: SolSignature,
        instr_data: &[u8],
        meta: &UiTransactionStatusMeta,
    ) -> ProgramResult<()> {
        let (_, _, hash, chain_id, _, _) =
            do_tx_holder_iterative::args(instr_data).inspect_err(|e| {
                tracing::warn!("Tx {:?} {}: ", sol_tx, e.to_string());
            })?;

        if chain_id == self.chain_id {
            self.add_big_tx(tx_idx, TxHash::from_slice(hash.as_bytes()), meta)?;
        }

        Ok(())
    }

    pub fn process_transmit_tx(
        &mut self,
        tx_idx: usize,
        sol_tx: SolSignature,
        instr_data: &[u8],
    ) -> ProgramResult<()> {
        let (_, offset, hash, chain_id, chunk) =
            transmit_tx::args(instr_data).inspect_err(|e| {
                tracing::warn!("Tx {:?} {}: ", sol_tx, e.to_string());
            })?;

        if chain_id == self.chain_id {
            self.parsed_txs
                .entry(TxHash::from_slice(hash.as_bytes()))
                .or_default()
                .entry(tx_idx)
                .or_insert(ParsedTxStep::TransmitTx {
                    offset,
                    data: chunk.to_vec(),
                });
        }

        Ok(())
    }

    pub fn commit(self) {
        if let Err(err) = self
            .tx_parser
            .register_slot_transactions(self.slot_number, self.parsed_txs)
        {
            tracing::warn!("Failed to register Solana transactions: {:?}", err);
        }
    }

    fn add_small_tx(
        &mut self,
        tx_idx: usize,
        tx: Transaction,
        meta: &UiTransactionStatusMeta,
    ) -> ProgramResult<()> {
        self.parsed_txs
            .entry(tx.hash)
            .or_default()
            .entry(tx_idx)
            .or_insert(ParsedTxStep::SmallTx {
                tx,
                tx_result: parse_transaction_meta(meta)?,
            });

        Ok(())
    }

    fn add_big_tx(
        &mut self,
        tx_idx: usize,
        tx_hash: TxHash,
        meta: &UiTransactionStatusMeta,
    ) -> ProgramResult<()> {
        self.parsed_txs
            .entry(tx_hash)
            .or_default()
            .entry(tx_idx)
            .or_insert(ParsedTxStep::BigTx {
                tx_result: parse_transaction_meta(meta)?,
            });

        Ok(())
    }
}

pub struct BlockParser {
    solana_block_storage: Arc<dyn SolanaBlockStorage>,
    pub program_id: Pubkey,
    pub chain_id: u64,
    pub parse_mode: BlockParseMode,
}

impl BlockParser {
    pub fn new(
        solana_block_storage: Arc<dyn SolanaBlockStorage>,
        program_id: Pubkey,
        chain_id: u64,
        parse_mode: BlockParseMode,
    ) -> Self {
        BlockParser {
            solana_block_storage,
            program_id,
            chain_id,
            parse_mode,
        }
    }

    fn register_block(
        &self,
        slot_number: Slot,
        solana_block: Arc<UiConfirmedBlock>,
        tx_parser: &mut TxParser,
    ) {
        let mut sol_bp = SolanaBlockParser::new(self.chain_id, tx_parser, slot_number);

        parse_solana_block(&solana_block, self.program_id, |tx_idx, entry| {
            if entry.instr.data.is_empty() {
                tracing::warn!("EVM Instruction data is empty")
            }

            let instr_data = &entry.instr.data[1..];
            if let Err(err) = match entry.instr.data[0] {
                n if n == DoTx as u8 => {
                    sol_bp.process_do_tx(tx_idx, *entry.sol_tx, instr_data, entry.meta)
                }
                n if n == TransmitTx as u8 => {
                    sol_bp.process_transmit_tx(tx_idx, *entry.sol_tx, instr_data)
                }
                n if n == DoTxHolder as u8 => {
                    sol_bp.process_do_tx_holder(tx_idx, *entry.sol_tx, instr_data, entry.meta)
                }
                n if n == DoTxIterative as u8 => {
                    sol_bp.process_do_tx_iterative(tx_idx, *entry.sol_tx, instr_data, entry.meta)
                }
                n if n == DoTxHolderIterative as u8 => sol_bp.process_do_tx_holder_iterative(
                    tx_idx,
                    *entry.sol_tx,
                    instr_data,
                    entry.meta,
                ),
                _ => Ok(()),
            } {
                tracing::warn!(
                    "Transaction {slot_number}:{tx_idx} parsing failed: {:?}",
                    err
                )
            }
        });

        sol_bp.commit();
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
        final_slot: Slot,
        mut max_blocks: usize,
        tx_parser: &mut TxParser,
    ) -> ProgramResult<Option<BlockParseResult>> {
        max_blocks = std::cmp::min(max_blocks, final_slot as usize);
        if let Some(final_block) = self.solana_block_storage.get_block(final_slot).await? {
            let mut current_slot = final_slot;
            let mut current_block = final_block.clone();
            let mut blocks_parsed: usize = 0;

            loop {
                self.register_block(current_slot, current_block.clone(), tx_parser);
                blocks_parsed += 1;

                if tx_parser.is_slot_complete(final_slot) {
                    return Ok(Some(BlockParseResult::new(
                        final_slot,
                        final_block,
                        tx_parser,
                        self.parse_mode,
                    )?));
                } else if blocks_parsed >= max_blocks {
                    return Err(Custom(format!(
                        "{:?} blocks parsed. Slot {:?} is not complete",
                        max_blocks, final_slot
                    )));
                }

                current_slot = current_block.parent_slot;
                current_block = self.get_block_with_retries(current_slot).await?;
            }
        } else {
            Ok(None)
        }
    }
}
