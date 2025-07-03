use rome_solana::tower::SolanaTower;
use rome_solana::types::{AsyncAtomicRpcClient, SyncAtomicRpcClient};

use crate::error::RomeEvmError::Custom;
use crate::error::{ProgramResult, RomeEvmError};
use crate::indexer::{
    BlockParams, BlockParser, BlockProducer, MultiplexedSolanaClient, ProducedBlocks,
    ProductionResult,
};
use crate::indexer::{BlockType, EthereumBlockStorage, ProducerParams};
use crate::indexer::{RollupIndexer, SolanaBlockLoader, SolanaBlockStorage, StandaloneIndexer};
use crate::tx::{Iterable, TxBuilder};
use crate::util::{check_accounts_len, check_exit_reason, RomeEvmUtil};
use crate::Payer;
use async_trait::async_trait;
use emulator::Emulation;
use ethers::types::{
    Address, BlockId, BlockNumber, Bytes, FeeHistory, Transaction as EthTransaction,
    TransactionReceipt, TransactionRequest, TxHash, H256, U256, U64,
};
use ethers::utils::keccak256;
use rome_evm::error::RomeProgramError::AccountNotFound;
use rome_evm::{state::pda::Pda, OwnerInfo};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    clock::Slot,
    commitment_config::{CommitmentConfig, CommitmentLevel},
    pubkey::Pubkey,
    signer::{keypair::Keypair, Signer},
    transaction::Transaction,
    instruction::Instruction
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use solana_program::instruction::AccountMeta;
use tokio::sync::RwLock;
use tokio::{sync::oneshot, task::JoinHandle};

const BLOCK_RETRIES: usize = 10;
const TX_RETRIES: usize = 10;
const RETRY_INTERVAL: Duration = Duration::from_secs(10);
const INDEXING_INTERVAL_MS: u64 = 400;
const MAX_FEE_HISTORY_BLOCKS: u64 = 1000;
const BASE_FEE_PER_GAS: u64 = 1000;

/// Client component interacting with the instance of Rome-EVM smart-contract on Solana blockchain
///
/// Interface of RomeEVMClient is designed to be closely compatible with standard Ethereum JSON RPC
/// and mostly repeats its functionality.
pub struct RomeEVMClient {
    ethereum_block_storage: Arc<dyn EthereumBlockStorage>,

    tx_builder: TxBuilder,

    solana: SolanaTower,

    commitment_level: CommitmentLevel,

    genesis_timestamp: U256,

    gas_price: U256,
}

#[derive(Clone)]
struct DummyBlockProducer {
    next_block: Arc<AtomicU64>,
}

impl DummyBlockProducer {
    fn new() -> Self {
        Self {
            next_block: Arc::new(AtomicU64::new(1)),
        }
    }
}

#[async_trait]
impl BlockProducer for DummyBlockProducer {
    async fn last_produced_block(&self) -> ProgramResult<U64> {
        Ok(U64::from(self.next_block.load(Ordering::Relaxed) - 1))
    }

    async fn get_block_params(&self, _block_number: U64) -> ProgramResult<BlockParams> {
        Err(Custom(
            "DummyBlockProducer does not support get_block_params".to_string(),
        ))
    }

    async fn produce_blocks(
        &self,
        producer_params: &ProducerParams,
        limit: Option<usize>,
    ) -> ProgramResult<ProductionResult> {
        let mut parent_hash = producer_params.parent_hash;
        let mut produced_blocks = ProducedBlocks::default();
        for (slot_number, pending_l1_block, block_idx, _) in producer_params.pending_blocks.iter() {
            let block_number = self.next_block.fetch_add(1, Ordering::Relaxed);
            let blockhash = H256::random();
            produced_blocks.insert(
                *slot_number,
                *block_idx,
                BlockParams {
                    hash: blockhash,
                    parent_hash,
                    number: U64::from(block_number),
                    timestamp: pending_l1_block.timestamp,
                },
            );
            parent_hash = Some(blockhash);

            if let Some(limit) = limit {
                if produced_blocks.len() >= limit {
                    break;
                }
            }
        }

        Ok(ProductionResult {
            produced_blocks,
            finalized_block: H256::zero(),
        })
    }
}

impl RomeEVMClient {
    /// Constructor
    ///
    /// * `chain_id` - Chain ID of a Rollup
    /// * `program_id` - Address of a Rollup Solana smart-contract
    /// * `solana` - Solana RPC Client
    /// * `commitment_level` - Solana commitment level used to execute and index transactions
    /// * `ethereum_block_storage` - Ethereum block storage
    /// * `payer` - Solana account keypair used to sign transactions to Rollup smart-contract
    ///
    /// Returns tuple (<Client_instance, Run_future>)
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        chain_id: u64,
        program_id: Pubkey,
        solana: SolanaTower,
        commitment_level: CommitmentLevel,
        ethereum_block_storage: Arc<dyn EthereumBlockStorage>,
        payers: Vec<Payer>,
        gas_price: U256,
    ) -> Self {
        let sync_client = Arc::new(RpcClient::new_with_commitment(
            solana.client().url(),
            solana.client().commitment(),
        ));

        Self {
            ethereum_block_storage,
            tx_builder: TxBuilder::new(chain_id, program_id, sync_client, payers),
            solana,
            commitment_level,
            genesis_timestamp: U256::from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis(),
            ),
            gas_price,
        }
    }

    /// Start the indexer and consume blocks
    pub fn start_indexing<S: SolanaBlockStorage + 'static>(
        &self,
        block_parser: BlockParser,
        solana_block_storage: Arc<S>,
        start_slot: Option<Slot>,
        idx_started_oneshot: Option<oneshot::Sender<()>>,
        max_slot_history: Option<Slot>,
        block_loader_batch_size: Slot,
    ) -> JoinHandle<ProgramResult<()>> {
        let solana_block_loader = SolanaBlockLoader {
            solana_block_storage: solana_block_storage.clone(),
            client: Arc::new(MultiplexedSolanaClient::from(self.solana.client_cloned())),
            commitment: self.commitment_level,
            program_id: *self.program_id(),
            batch_size: block_loader_batch_size,
            block_retries: BLOCK_RETRIES,
            tx_retries: TX_RETRIES,
            retry_int: RETRY_INTERVAL,
        };

        let rollup_indexer = RollupIndexer::new(
            Arc::new(RwLock::new(block_parser)),
            solana_block_storage.clone(),
            self.ethereum_block_storage.clone(),
            Some(Arc::new(DummyBlockProducer::new())),
            max_slot_history,
        );

        StandaloneIndexer {
            solana_block_loader: Some(solana_block_loader),
            rollup_indexer: Some(rollup_indexer),
        }
        .start_indexing(start_slot, idx_started_oneshot, INDEXING_INTERVAL_MS)
    }

    /// Emulates a raw transaction using full set of resources
    ///
    /// * `rlp` - signed rlp bytes for the transaction to be emulated
    ///
    /// Returns tuple (<transaction_hash>, <AdvanceTx>)
    pub async fn prepare_transaction(&self, rlp: Bytes) -> ProgramResult<(TxHash, Iterable)> {
        let hash: TxHash = keccak256(rlp.as_ref()).into();
        Ok((hash, self.tx_builder.build_tx(rlp, hash).await?))
    }

    /// Executes transaction in a Rollup smart-contract
    ///
    /// * `rlp` - rlp of transaction
    ///
    /// Returns transaction hash or error if transaction can not be executed
    pub async fn send_transaction(&self, rlp: Bytes) -> ProgramResult<TxHash> {
        let (hash, mut tx) = self.prepare_transaction(rlp).await?;

        self.solana
            .send_and_confirm_tx_iterable(&mut *tx)
            .await
            .map_err(|err| Custom(err.to_string()))?;

        Ok(hash)
    }
    /// Executes transaction consisting of rome-evm instruction and SVM-instructions
    ///
    /// * `rlp` - rlp of rome-evm transaction
    /// * `svm` - SVM-instructions
    /// * `alt_keys` - pubkeys of address lookup tables 
    ///
    /// Returns transaction hash or error if transaction can not be executed
    pub async fn send_composite_transaction(
        &self, 
        rlp: Bytes,
        svm: Vec<Instruction>,
        alt_keys: Option<Vec<Pubkey>>,
    ) -> ProgramResult<TxHash> {
        let hash: TxHash = keccak256(rlp.as_ref()).into();
        let mut tx = self.tx_builder.build_svm_tx(rlp, svm, alt_keys).await?;

        self.solana
            .send_and_confirm_tx_iterable(&mut *tx)
            .await
            .map_err(|err| Custom(err.to_string()))?;

        Ok(hash)
    }

    /// Returns balance of requested account on current block
    ///
    /// * `address` - address on an account
    pub fn get_balance(&self, address: Address) -> ProgramResult<U256> {
        let value = emulator::eth_get_balance(
            self.program_id(),
            &rome_evm::H160::from(address.0),
            self.sync_rpc_client(),
            self.tx_builder.chain_id,
        )?;

        let mut buf = [0; 32];
        value.to_big_endian(&mut buf);

        Ok(U256::from_big_endian(&buf))
    }

    /// Returns Chain ID of the Rollup
    pub fn chain_id(&self) -> u64 {
        self.tx_builder.chain_id
    }

    /// Returns latest block number
    pub async fn block_number(&self) -> ProgramResult<U64> {
        if let Some(block_number) = self.ethereum_block_storage.latest_block().await? {
            Ok(block_number)
        } else {
            Ok(U64::zero())
        }
    }

    /// Returns current gas price
    pub fn gas_price(&self) -> ProgramResult<U256> {
        Ok(self.gas_price)
    }

    /// Executes given transaction request on a current block
    ///
    /// * `call` - Transaction request object
    ///
    /// Returns result of execution
    pub fn call(&self, call: &TransactionRequest) -> ProgramResult<Bytes> {
        let emulation = emulator::eth_call(
            self.program_id(),
            RomeEvmUtil::cast_transaction_request(call, self.tx_builder.chain_id),
            self.sync_rpc_client(),
        )?;

        check_exit_reason(&emulation)?;
        let vm = emulation.vm.expect("vm expected");
        let value = vm.return_value.unwrap_or_default();
        Ok(value.into())
    }

    /// Returns transaction count (nonce) of a requested account in the latest block
    ///
    /// * `address` - address of account
    pub fn transaction_count(&self, address: Address) -> ProgramResult<U64> {
        let value = emulator::eth_get_tx_count(
            self.program_id(),
            &rome_evm::H160::from(address.0),
            self.sync_rpc_client(),
            self.tx_builder.chain_id,
        )?;
        Ok(value.into())
    }

    /// Estimate gas amount for a given transaction
    ///
    /// * `call` - transaction request to estimate gas
    pub fn estimate_gas(&self, call: &TransactionRequest) -> ProgramResult<U256> {
        let emulation = emulator::eth_estimate_gas(
            self.program_id(),
            RomeEvmUtil::cast_transaction_request(call, self.tx_builder.chain_id),
            self.sync_rpc_client(),
        )?;
        check_exit_reason(&emulation)?;
        check_accounts_len(&emulation)?;

        Ok(emulation.gas.into())
    }

    async fn get_block_number(&self, block_number: BlockId) -> ProgramResult<Option<U64>> {
        match block_number {
            BlockId::Number(number) => match number {
                BlockNumber::Latest => {
                    if let Some(latest) = self.ethereum_block_storage.latest_block().await? {
                        Ok(Some(latest))
                    } else {
                        Ok(Some(U64::zero()))
                    }
                }
                BlockNumber::Number(number) => Ok(Some(number)),
                other => Err(RomeEvmError::Custom(format!(
                    "Block tag {:?} not supported",
                    other
                ))),
            },
            BlockId::Hash(hash) => self.ethereum_block_storage.get_block_number(&hash).await,
        }
    }

    /// Returns a collection of historical gas information for a specified number of blocks
    ///
    /// * `count` - Number of blocks to retrieve fee history for.
    /// * `block_number` - Starting block number for fee history retrieval.
    /// * `reward_percentiles` - Percentiles (0 to 100) for fee distribution.
    pub async fn fee_history(
        &self,
        count: u64,
        block_number: BlockId,
        reward_percentiles: Vec<f64>,
    ) -> ProgramResult<FeeHistory> {
        let count = count.min(MAX_FEE_HISTORY_BLOCKS);
        if let Some(block_number) = self.get_block_number(block_number).await? {
            let block_number = block_number.as_u64();
            let base_fee_per_gas = vec![U256::from(BASE_FEE_PER_GAS); count as usize];
            let gas_used_ratio = vec![0.5; count as usize];
            let oldest_block = U256::from(block_number.saturating_sub(count));

            let rewards = vec![
                reward_percentiles
                    .iter()
                    .map(|&p| U256::from(p as u64))
                    .collect::<Vec<U256>>();
                count as usize
            ];

            Ok(FeeHistory {
                base_fee_per_gas,
                gas_used_ratio,
                oldest_block,
                reward: rewards,
            })
        } else {
            Err(RomeEvmError::Custom("Indexer is not started".to_string()))
        }
    }

    /// Returns bytecode of a requested smart-contract in the latest block
    ///
    /// * `address` - Address of a smart-contract
    pub fn get_code(&self, address: Address) -> ProgramResult<Bytes> {
        let value = emulator::eth_get_code(
            self.program_id(),
            &rome_evm::H160::from(address.0),
            self.sync_rpc_client(),
            self.tx_builder.chain_id,
        )?;
        let bytes = value.into();
        Ok(bytes)
    }

    /// Returns requested block of a Rollup smart-contract.
    ///
    /// * `block_id`- identifier of a block given by block number/eth-commitment-level/block-hash
    /// * `full_transactions` - whether to return transaction list in a block as a full form
    ///                         (list of transaction request structures) or only list of
    ///                         transaction hashes
    ///
    /// Returns Eth-compatible representation of Rollup block
    pub async fn get_block(
        &self,
        block_id: BlockId,
        full_transactions: bool,
    ) -> ProgramResult<Option<BlockType>> {
        if let Some(block_number) = self.get_block_number(block_id).await? {
            if block_number == U64::zero() {
                Ok(Some(BlockType::genesis(
                    self.genesis_timestamp,
                    full_transactions,
                )))
            } else {
                self.ethereum_block_storage
                    .get_block_by_number(block_number, full_transactions)
                    .await
            }
        } else {
            Ok(None)
        }
    }

    /// Emulates a raw transaction using rome-evm emulator
    ///
    /// * `rlp` - signed rlp bytes for the transaction to be emulated
    /// * `pkey` - solana public key of the payer account
    ///
    /// Returns the list of accounts from emulation report
    pub async fn emulate_tx(&self, rlp: Bytes, pkey: Pubkey) -> ProgramResult<Vec<AccountMeta>> {
        let mut data = vec![0];
        data.extend_from_slice(&rlp);

        let emulation = self.emulate(emulator::Instruction::DoTx, &data, &pkey)?;
        check_exit_reason(&emulation)?;
        check_accounts_len(&emulation)?;

        Ok(emulation.accounts.iter().map(|(pubkey, acc)| AccountMeta {
            pubkey: *pubkey,
            is_signer: acc.signer,
            is_writable: acc.account.writable,
        }).collect())
    }

    /// Runs emulation of a given transaction request on a latest block with commitment level
    /// configured during client construction.
    ///
    /// * `instruction` - Instruction to emulate
    /// * `data` - Instruction data
    ///
    /// Returns result of emulation
    pub fn emulate(
        &self,
        instruction: emulator::Instruction,
        data: &[u8],
        payer: &Pubkey,
    ) -> ProgramResult<Emulation> {
        let mut bin = vec![instruction as u8];
        bin.extend(data);

        emulator::emulate(
            self.program_id(),
            &bin,
            payer,
            self.tx_builder.client_cloned(),
        )
        .map_err(|err| err.into())
    }

    /// Get the solana tower
    pub fn solana(&self) -> &SolanaTower {
        &self.solana
    }

    /// Get [AsyncAtomicRpcClient]
    pub fn rpc_client(&self) -> AsyncAtomicRpcClient {
        self.solana.client_cloned()
    }

    /// Get [SyncAtomicRpcClient]
    pub fn sync_rpc_client(&self) -> SyncAtomicRpcClient {
        self.tx_builder.client_cloned()
    }

    /// Get program id
    pub fn program_id(&self) -> &Pubkey {
        self.tx_builder.program_id()
    }

    /// Get reference to TxBuilder
    pub fn tx_builder(&self) -> &TxBuilder {
        &self.tx_builder
    }

    /// Retrieves the value stored at a specific storage slot for a given address.
    ///
    /// * `address` - The Ethereum address to retrieve the storage value from.
    /// * `slot` - The storage slot index to retrieve the value from.
    ///
    /// The corresponding storage value.
    pub fn eth_get_storage_at(&self, address: Address, slot: U256) -> ProgramResult<U256> {
        let mut buf = [0u8; 32];
        slot.to_big_endian(&mut buf);

        let value = emulator::eth_get_storage_at(
            self.program_id(),
            &rome_evm::H160::from(address.0),
            &rome_evm::U256::from_big_endian(&buf),
            self.sync_rpc_client(),
            self.chain_id(),
        )?;

        value.to_big_endian(&mut buf);

        Ok(U256::from_big_endian(&buf))
    }

    /// The instruction is used to deposit funds to the rome-evm balance account.
    /// The instruction data consists of: chain_id | rlp
    ///
    /// Special type 0x7E of rlp is used.
    /// Rome-evm mints the funds on the user account.
    /// SOLs are transferred from the solana user's wallet to rome-evm wallet.
    ///
    /// Rate: 1 SOL = 1 rome-evm token
    ///
    /// The amount in Wei is used as rlp.mint.
    /// This amount must be multiple of 10^9, because the precision of rome-evm token is 10^18,
    /// precision of native SOL token is 10^9.
    ///
    /// This solana transaction must be signed by solana user's wallet private key.
    pub async fn deposit(&self, rlp: &[u8], keypair: &Keypair) -> ProgramResult<()> {
        let mut data = vec![emulator::Instruction::Deposit as u8];
        data.extend(self.chain_id().to_le_bytes());
        data.extend(rlp);

        let emulation = emulator::emulate(
            self.program_id(),
            &data,
            &keypair.pubkey(),
            self.sync_rpc_client(),
        )?;

        let ix = self.tx_builder.build_ix(&emulation, data);
        let blockhash = self.rpc_client().get_latest_blockhash().await?;
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&keypair.pubkey()),
            &[keypair],
            blockhash,
        );
        let _ = self.rpc_client().send_and_confirm_transaction(&tx).await?;

        Ok(())
    }

    /// Instruction is used to registry rollup owner.
    /// This private instruction must be signed with the registry-authority keypair
    pub async fn reg_owner(
        &self,
        chain_id: u64,
        registry_authority: &Keypair,
    ) -> ProgramResult<()> {
        let mut data = vec![emulator::Instruction::RegOwner as u8];
        data.extend(chain_id.to_le_bytes());

        let emulation = emulator::emulate(
            self.program_id(),
            &data,
            &registry_authority.pubkey(),
            self.sync_rpc_client(),
        )?;

        let ix = self.tx_builder.build_ix(&emulation, data);
        let blockhash = self.rpc_client().get_latest_blockhash().await?;
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&registry_authority.pubkey()),
            &[registry_authority],
            blockhash,
        );
        let _ = self.rpc_client().send_and_confirm_transaction(&tx).await?;

        Ok(())
    }

    pub fn get_rollups(&self) -> ProgramResult<Vec<OwnerInfo>> {
        let rollups = emulator::get_rollups(self.program_id(), self.sync_rpc_client())?;

        Ok(rollups)
    }

    pub async fn get_transaction_receipt(
        &self,
        tx_hash: &TxHash,
    ) -> ProgramResult<Option<TransactionReceipt>> {
        self.ethereum_block_storage
            .get_transaction_receipt(tx_hash)
            .await
    }

    pub async fn get_transaction(&self, tx_hash: &TxHash) -> ProgramResult<Option<EthTransaction>> {
        self.ethereum_block_storage.get_transaction(tx_hash).await
    }

    pub fn program_sol_wallet(&self) -> Pubkey {
        let pda = Pda::new_(self.program_id(), self.chain_id());
        let (key, _) = pda.sol_wallet();

        key
    }
    pub async fn solana_balance(&self, key: &Pubkey) -> ProgramResult<u64> {
        let cfg = CommitmentConfig {
            commitment: self.commitment_level,
        };

        let acc = self
            .solana
            .client()
            .get_account_with_commitment(key, cfg)
            .await?
            .value
            .ok_or(AccountNotFound(*key))?;

        Ok(acc.lamports)
    }
}
