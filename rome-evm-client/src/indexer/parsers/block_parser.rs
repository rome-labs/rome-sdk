use {
    crate::{
        error::{ProgramResult, RomeEvmError::*},
        indexer::{
            parsers::{
                default_tx_parser::decode_transaction_from_rlp,
                l1_attributes::{create_l1_attributes_tx, update_if_user_deposited_tx},
                log_parser::{self, LogParser},
                tx_parser::{ParsedTxStep, ParsedTxs, TxParser},
            },
            produced_blocks::BlockParams,
            solana_block_storage::SolanaBlockStorage,
            BlockType,
        },
    },
    emulator::instruction::Instruction::*,
    ethers::types::{Address, BigEndianHash, Block, Log, Transaction, TxHash, H256, U256},
    jsonrpsee_core::Serialize,
    rlp::Rlp,
    rome_evm::api::{
        deposit, do_tx_holder, do_tx_holder_iterative, do_tx_iterative, split_fee, transmit_tx,
    },
    serde::Deserialize,
    solana_program::{clock::UnixTimestamp, instruction::CompiledInstruction},
    solana_sdk::pubkey::Pubkey,
    solana_sdk::{bs58, signature::Signature as SolSignature, slot_history::Slot},
    solana_transaction_status::{
        option_serializer::OptionSerializer, UiConfirmedBlock, UiTransactionStatusMeta,
    },
    std::{ops::AddAssign, sync::Arc},
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
    pub gas_price: U256,
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
    pub fn new(
        timestamp: Option<UnixTimestamp>,
        transactions: Vec<TxHash>,
        block_gas_used: U256,
        gas_recipient: Option<Address>,
    ) -> Self {
        Self {
            block_gas_used,
            transactions,
            gas_recipient,
            slot_timestamp: timestamp,
            block_params: None,
        }
    }

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
    pub solana_blockhash: H256,
    pub parent_solana_blockhash: H256,
    pub timestamp: Option<UnixTimestamp>,
    pub transactions: Vec<(Transaction, TxResult)>,
    pub eth_blocks: Vec<EthBlock>,
}

impl BlockParseResult {
    pub fn new<T: TxParser>(
        slot_number: Slot,
        solana_block: Arc<UiConfirmedBlock>,
        tx_parser: &mut T,
        parse_mode: BlockParseMode,
        enable_deposits_slot: Option<Slot>,
    ) -> ProgramResult<Self> {
        let solana_blockhash = H256::from_slice(
            &bs58::decode(&solana_block.blockhash)
                .into_vec()
                .map_err(|_| {
                    Custom(format!(
                        "Solana block {:?} invalid blockhash: {}",
                        slot_number, solana_block.blockhash
                    ))
                })?,
        );

        let parent_solana_blockhash = H256::from_slice(
            &bs58::decode(&solana_block.previous_blockhash)
                .into_vec()
                .map_err(|_| {
                    Custom(format!(
                        "Solana block {:?} invalid parent blockhash: {}",
                        slot_number, solana_block.previous_blockhash
                    ))
                })?,
        );

        let solana_blocktime = solana_block
            .block_time
            .map(U256::from)
            .ok_or(Custom(format!(
                "Solana blocktime is not set for {slot_number}"
            )))?;
        let mut transactions = Self::parse_transactions(
            slot_number,
            &solana_blockhash,
            &solana_blocktime,
            enable_deposits_slot,
            tx_parser,
        )?;

        let eth_blocks = Self::create_eth_blocks(
            &solana_blockhash,
            solana_block.block_time,
            &mut transactions,
            parse_mode,
        );

        Ok(Self {
            slot_number,
            solana_blockhash,
            parent_solana_blockhash,
            timestamp: solana_block.block_time,
            transactions,
            eth_blocks,
        })
    }

    fn parse_transactions<T: TxParser>(
        slot_number: Slot,
        solana_blockhash: &H256,
        solana_blocktime: &U256,
        enable_deposits_slot: Option<Slot>,
        tx_parser: &mut T,
    ) -> ProgramResult<Vec<(Transaction, TxResult)>> {
        let parsed_txs = tx_parser.finalize_slot(slot_number)?;
        if !parsed_txs.is_empty() {
            tracing::info!("Parsed transactions: {:?}", parsed_txs);
        }

        Ok(if let Some(enable_deposits_slot) = enable_deposits_slot {
            if slot_number >= enable_deposits_slot {
                Some(create_l1_attributes_tx(
                    slot_number,
                    solana_blockhash,
                    solana_blocktime,
                ))
                .into_iter()
                .chain(parsed_txs)
                .collect()
            } else {
                parsed_txs
            }
        } else {
            parsed_txs
        })
    }

    pub fn create_eth_blocks(
        solana_blockhash: &H256,
        timestamp: Option<UnixTimestamp>,
        transactions: &mut Vec<(Transaction, TxResult)>,
        parse_mode: BlockParseMode,
    ) -> Vec<EthBlock> {
        let mut block_gas_used = U256::zero();
        let mut eth_blocks = Vec::new();
        let mut block_txs = Vec::new();
        let mut gas_recipient = None;
        let mut block_log_index = U256::zero();

        for (tx, tx_result) in transactions {
            update_if_user_deposited_tx(tx, solana_blockhash, block_log_index);
            block_log_index.add_assign(U256::from(tx_result.logs.len()));
            match parse_mode {
                BlockParseMode::EngineApi => {
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

                        eth_blocks.push(EthBlock::new(
                            timestamp,
                            block_txs,
                            block_gas_used,
                            gas_recipient,
                        ));
                        block_txs = vec![tx.hash];
                        block_gas_used = tx_result.gas_report.gas_value;
                        gas_recipient = tx_result.gas_report.gas_recipient;
                        block_log_index = U256::zero();
                    }
                }

                BlockParseMode::SingleState => {
                    block_txs.push(tx.hash);
                    block_gas_used += tx_result.gas_report.gas_value;
                }
            }
        }

        if !block_txs.is_empty() {
            eth_blocks.push(EthBlock::new(
                timestamp,
                block_txs,
                block_gas_used,
                gas_recipient,
            ));
        }

        eth_blocks
    }
}

struct SolanaBlockParser<'a, T: TxParser> {
    chain_id: u64,
    parsed_txs: ParsedTxs,
    tx_parser: &'a mut T,
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
                        gas_price: parser.gas_price.unwrap_or(U256::zero()),
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

impl<'a, T: TxParser> SolanaBlockParser<'a, T> {
    pub fn new(chain_id: u64, tx_parser: &'a mut T, slot_number: Slot) -> Self {
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

    pub fn process_deposit_tx(
        &mut self,
        tx_idx: usize,
        sol_tx: SolSignature,
        instr_data: &[u8],
    ) -> ProgramResult<()> {
        let (chain_id, rlp) = deposit::args(instr_data).inspect_err(|e| {
            tracing::warn!("Tx {:?} {}: ", sol_tx, e.to_string());
        })?;

        if chain_id == self.chain_id {
            let rlp = Rlp::new(rlp);
            let tx = decode_transaction_from_rlp(&rlp)?;
            self.parsed_txs
                .entry(tx.hash)
                .or_default()
                .entry(tx_idx)
                .or_insert(ParsedTxStep::SmallTx {
                    tx,
                    tx_result: Some(TxResult::default()),
                });
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
    pub solana_block_storage: Arc<dyn SolanaBlockStorage>,
    pub program_id: Pubkey,
    pub chain_id: u64,
    pub parse_mode: BlockParseMode,
    pub enable_deposit_slot: Option<Slot>,
}

impl BlockParser {
    fn register_block<T: TxParser>(
        &self,
        slot_number: Slot,
        solana_block: Arc<UiConfirmedBlock>,
        tx_parser: &mut T,
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
                n if n == Deposit as u8 => {
                    sol_bp.process_deposit_tx(tx_idx, *entry.sol_tx, instr_data)
                }
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

    pub async fn parse<T: TxParser>(
        &mut self,
        final_slot: Slot,
        mut max_blocks: usize,
        tx_parser: &mut T,
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
                        self.enable_deposit_slot,
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

#[cfg(test)]
mod test {
    use super::*;
    use crate::indexer::parsers::block_parser::{GasReport, TxResult};
    use crate::indexer::parsers::log_parser::ExitReason;
    use ethers::core::k256::ecdsa::SigningKey;
    use ethers::prelude::transaction::eip2718::TypedTransaction;
    use ethers::prelude::OtherFields;
    use ethers::signers::{Signer, Wallet};
    use ethers::types::{
        Address, Bytes, NameOrAddress, Transaction, TransactionRequest, U256, U64,
    };
    use rand::{random, thread_rng};
    use std::ops::Mul;

    struct MockTxParser {
        pub transactions: Vec<(Transaction, TxResult)>,
    }

    impl TxParser for MockTxParser {
        fn register_slot_transactions(
            &mut self,
            _slot_number: Slot,
            _slot_txs: ParsedTxs,
        ) -> ProgramResult<()> {
            todo!()
        }

        fn is_slot_complete(&self, _slot_number: Slot) -> bool {
            todo!()
        }

        fn finalize_slot(&self, _slot_number: Slot) -> ProgramResult<Vec<(Transaction, TxResult)>> {
            Ok(self.transactions.clone())
        }

        fn retain_from_slot(&mut self, _from_slot: Slot) {
            todo!()
        }
    }

    pub fn create_wallet() -> Wallet<SigningKey> {
        Wallet::new(&mut thread_rng())
    }

    pub fn create_result(gas_report: GasReport) -> TxResult {
        TxResult {
            exit_reason: ExitReason {
                code: 0,
                reason: "".to_string(),
                return_value: vec![],
            },
            logs: vec![],
            gas_report,
        }
    }

    const SIMPLE_TX_GAS: U256 = U256::one();

    pub async fn create_simple_tx(
        chain_id: Option<U64>,
        wallet: &Wallet<SigningKey>,
        gas_recipient: Option<Address>,
    ) -> (Transaction, TxResult) {
        let to = Address::random();
        let value = U256::from(100);
        let gas_price = U256::from(1);
        let gas = SIMPLE_TX_GAS;
        let nonce = U256::from(random::<u32>());

        let tx_request = TypedTransaction::Legacy(TransactionRequest {
            from: Some(wallet.address()),
            to: Some(NameOrAddress::Address(to)),
            gas: Some(gas),
            gas_price: Some(gas_price),
            value: Some(value),
            data: None,
            nonce: Some(nonce),
            chain_id,
        });

        let signature = wallet.sign_transaction(&tx_request).await.unwrap();

        (
            Transaction {
                hash: tx_request.hash(&signature),
                nonce: *tx_request.nonce().unwrap(),
                block_hash: None,
                block_number: None,
                transaction_index: None,
                from: wallet.address(),
                to: Some(to),
                value,
                gas_price: Some(gas_price),
                gas,
                input: Bytes::new(),
                v: U64::from(signature.v),
                r: signature.r,
                s: signature.s,
                source_hash: Default::default(),
                mint: None,
                is_system_tx: false,
                transaction_type: None,
                access_list: None,
                max_priority_fee_per_gas: None,
                max_fee_per_gas: None,
                chain_id: Some(U256::from(chain_id.unwrap().as_u64())),
                other: OtherFields::default(),
            },
            create_result(GasReport {
                gas_value: gas,
                gas_price,
                gas_recipient,
            }),
        )
    }

    const CHAIN_ID1: Option<U64> = Some(U64::one());
    const SLOT_NUMBER: Slot = 123;

    fn create_block_parse_result(
        transactions: Vec<(Transaction, TxResult)>,
        enable_deposits_slot: Option<Slot>,
    ) -> ProgramResult<BlockParseResult> {
        let mut tx_parser = MockTxParser { transactions };

        BlockParseResult::new(
            SLOT_NUMBER,
            Arc::new(UiConfirmedBlock {
                previous_blockhash: "F9j8t4tCumYJn8fcyw5nFg28YebU1caRXnoogSbtrX7F".to_string(),
                blockhash: "CvjH76pDbx5jJRThU6n2VrhMqwtjNCdXDSjTfpxnb4dR".to_string(),
                parent_slot: 0,
                transactions: None,
                signatures: None,
                rewards: None,
                num_reward_partitions: None,
                block_time: Some(0),
                block_height: None,
            }),
            &mut tx_parser,
            BlockParseMode::EngineApi,
            enable_deposits_slot,
        )
    }

    #[tokio::test]
    async fn test_grouping_gas_recipients() {
        let gas_recipient1 = Address::random();
        let gas_recipient2 = Address::random();
        let gas_recipient3 = Address::random();

        let wallet = create_wallet();

        let parse_result = create_block_parse_result(
            vec![
                create_simple_tx(CHAIN_ID1, &wallet, Some(gas_recipient2)).await,
                create_simple_tx(CHAIN_ID1, &wallet, Some(gas_recipient1)).await,
                create_simple_tx(CHAIN_ID1, &wallet, Some(gas_recipient1)).await,
                create_simple_tx(CHAIN_ID1, &wallet, Some(gas_recipient1)).await,
                create_simple_tx(CHAIN_ID1, &wallet, None).await,
                create_simple_tx(CHAIN_ID1, &wallet, Some(gas_recipient2)).await,
                create_simple_tx(CHAIN_ID1, &wallet, Some(gas_recipient1)).await,
                create_simple_tx(CHAIN_ID1, &wallet, Some(gas_recipient3)).await,
                create_simple_tx(CHAIN_ID1, &wallet, Some(gas_recipient3)).await,
                create_simple_tx(CHAIN_ID1, &wallet, None).await,
                create_simple_tx(CHAIN_ID1, &wallet, None).await,
            ],
            None,
        )
        .unwrap();

        assert_eq!(parse_result.eth_blocks.len(), 7);

        assert_eq!(
            parse_result.eth_blocks[0].gas_recipient,
            Some(gas_recipient2)
        );
        assert_eq!(
            parse_result.eth_blocks[0].block_gas_used,
            SIMPLE_TX_GAS.mul(1)
        );

        assert_eq!(
            parse_result.eth_blocks[1].gas_recipient,
            Some(gas_recipient1)
        );
        assert_eq!(
            parse_result.eth_blocks[1].block_gas_used,
            SIMPLE_TX_GAS.mul(3)
        );

        assert_eq!(parse_result.eth_blocks[2].gas_recipient, None);
        assert_eq!(
            parse_result.eth_blocks[2].block_gas_used,
            SIMPLE_TX_GAS.mul(1)
        );

        assert_eq!(
            parse_result.eth_blocks[3].gas_recipient,
            Some(gas_recipient2)
        );
        assert_eq!(
            parse_result.eth_blocks[3].block_gas_used,
            SIMPLE_TX_GAS.mul(1)
        );

        assert_eq!(
            parse_result.eth_blocks[4].gas_recipient,
            Some(gas_recipient1)
        );
        assert_eq!(
            parse_result.eth_blocks[4].block_gas_used,
            SIMPLE_TX_GAS.mul(1)
        );

        assert_eq!(
            parse_result.eth_blocks[5].gas_recipient,
            Some(gas_recipient3)
        );
        assert_eq!(
            parse_result.eth_blocks[5].block_gas_used,
            SIMPLE_TX_GAS.mul(2)
        );

        assert_eq!(parse_result.eth_blocks[6].gas_recipient, None);
        assert_eq!(
            parse_result.eth_blocks[6].block_gas_used,
            SIMPLE_TX_GAS.mul(2)
        );
    }

    #[tokio::test]
    async fn test_adding_l1_attributes_to_empty_block() {
        let parse_result = create_block_parse_result(vec![], Some(SLOT_NUMBER - 1)).unwrap();

        assert_eq!(parse_result.transactions.len(), 1);
        assert_eq!(
            parse_result.transactions[0].0.transaction_type,
            Some(U64::from(0x7E))
        );

        assert_eq!(parse_result.eth_blocks.len(), 1);
        assert_eq!(parse_result.eth_blocks[0].transactions.len(), 1);
    }

    #[tokio::test]
    async fn test_adding_l1_attributes_to_not_empty_block() {
        let gas_recipient2 = Address::random();
        let wallet = create_wallet();

        let parse_result = create_block_parse_result(
            vec![create_simple_tx(CHAIN_ID1, &wallet, Some(gas_recipient2)).await],
            Some(SLOT_NUMBER - 1),
        )
        .unwrap();

        assert_eq!(parse_result.transactions.len(), 2);
        assert_eq!(
            parse_result.transactions[0].0.transaction_type,
            Some(U64::from(0x7E))
        );

        assert_eq!(parse_result.eth_blocks.len(), 2);
        assert_eq!(parse_result.eth_blocks[0].transactions.len(), 1);
        assert_eq!(parse_result.eth_blocks[1].transactions.len(), 1);
    }
}
