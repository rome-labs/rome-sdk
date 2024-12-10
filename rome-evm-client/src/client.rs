use rome_solana::tower::SolanaTower;
use rome_solana::types::{AsyncAtomicRpcClient, SyncAtomicRpcClient};
use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};

use crate::error::{ProgramResult, RomeEvmError};
use crate::indexer::{
    BlockParams, BlockProducer, PendingBlocks, ProducedBlocks, SolanaBlockStorage,
    StandaloneIndexer,
};
use crate::indexer::{BlockType, EthereumBlockStorage};
use crate::tx::TxBuilder;
use crate::util::RomeEvmUtil;
use crate::Payer;
use async_trait::async_trait;
use emulator::{Emulation, Instruction};
use ethers::types::{
    Address, BlockId, BlockNumber, Bytes, FeeHistory, Transaction as EthTransaction,
    TransactionReceipt, TransactionRequest, TxHash, H256, U256, U64,
};
use ethers::utils::keccak256;
use rome_evm::OwnerInfo;
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    clock::Slot,
    commitment_config::CommitmentLevel,
    pubkey::Pubkey,
    signer::{keypair::Keypair, Signer},
    transaction::Transaction,
};
use std::sync::Arc;
use tokio::{
    sync::{oneshot, RwLock},
    task::JoinHandle,
};

/// Client component interacting with the instance of Rome-EVM smart-contract on Solana blockchain
/// (Rome-EVM Rollup).
///
/// Interface of RomeEVMClient is designed to be closely compatible with standard Ethereum JSON RPC
/// and mostly repeats its functionality.
pub struct RomeEVMClient<S: SolanaBlockStorage + 'static, E: EthereumBlockStorage + 'static> {
    solana_block_storage: Arc<S>,

    ethereum_block_storage: Arc<E>,

    tx_builder: TxBuilder,

    solana: SolanaTower,

    commitment_level: CommitmentLevel,

    genesis_timestamp: U256,
}

const INDEXING_INTERVAL_MS: u64 = 400;

#[derive(Clone)]
struct ClientBlockRegistrator {
    last_number: Arc<RwLock<u64>>,
}

impl ClientBlockRegistrator {
    fn new() -> Self {
        Self {
            last_number: Arc::new(RwLock::new(1)),
        }
    }
}

#[async_trait]
impl BlockProducer for ClientBlockRegistrator {
    async fn produce_blocks(
        &self,
        mut parent_hash: Option<TxHash>,
        pending_blocks: &PendingBlocks,
    ) -> ProgramResult<ProducedBlocks> {
        let mut lock = self.last_number.write().await;
        let mut results = BTreeMap::new();
        for (block_id, pending_block) in pending_blocks {
            let blockhash = H256::random();
            results.insert(
                *block_id,
                BlockParams {
                    hash: blockhash,
                    parent_hash,
                    number: U64::from(*lock.deref()),
                    timestamp: U256::from(pending_block.slot_timestamp.unwrap_or_default()),
                },
            );
            *lock.deref_mut() += 1;
            parent_hash = Some(blockhash);
        }

        Ok(results)
    }
}

impl<S: SolanaBlockStorage + 'static, E: EthereumBlockStorage + 'static> RomeEVMClient<S, E> {
    /// Constructor
    ///
    /// * `chain_id` - Chain ID of a Rollup
    /// * `program_id` - Address of a Rollup Solana smart-contract
    /// * `payer` - Solana account keypair used to sign transactions to Rollup smart-contract
    /// * `client` - Solana RPC Client
    /// * `number_holders` - number of holder accounts used by this instance of client.
    ///                      Determines maximum number of parallel transactions which
    ///                      this particular client can execute simultaneously.
    /// * `commitment_level` - Solana commitment level used to execute and index transactions
    /// * `start_slot` - number of solana slot to start indexing from
    /// * `token` - token to call Graceful Shutdown of the async tasks
    ///
    /// Returns tuple (<Client_instance, Run_future>)
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        program_id: Pubkey,
        solana: SolanaTower,
        commitment_level: CommitmentLevel,
        solana_block_storage: Arc<S>,
        ethereum_block_storage: Arc<E>,
        payers: Vec<Payer>,
    ) -> Self {
        let chain_id = ethereum_block_storage.chain();
        let sync_client = Arc::new(RpcClient::new_with_commitment(
            solana.client().url(),
            solana.client().commitment(),
        ));

        Self {
            solana_block_storage,
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
        }
    }

    /// Start the indexer and consume blocks
    pub async fn start_indexing(
        &self,
        start_slot: Option<Slot>,
        idx_started_oneshot: Option<oneshot::Sender<()>>,
        max_slot_history: Option<Slot>,
    ) -> JoinHandle<()> {
        StandaloneIndexer {
            solana_client: self.solana.client_cloned(),
            commitment_level: self.commitment_level,
            rome_evm_pubkey: *self.program_id(),
            solana_block_storage: self.solana_block_storage.clone(),
            ethereum_block_storage: self.ethereum_block_storage.clone(),
            block_producer: ClientBlockRegistrator::new(),
        }
        .start_indexing(
            start_slot,
            idx_started_oneshot,
            INDEXING_INTERVAL_MS,
            max_slot_history,
        )
    }

    /// Executes transaction in a Rollup smart-contract
    ///
    /// * `tx_request` - Transaction request object to execute
    /// * `eth_signature` - Signature of a transaction
    ///
    /// Returns transaction hash or error if transaction can not be executed
    pub async fn send_transaction(&self, rlp: Bytes) -> ProgramResult<TxHash> {
        let hash: TxHash = keccak256(rlp.as_ref()).into();

        let mut tx = self.tx_builder.build_tx(rlp, hash).await?;

        self.solana
            .send_and_confirm_tx_iterable(&mut *tx)
            .await
            .map_err(|err| RomeEvmError::Custom(err.to_string()))?;

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
        let gas_price = 1;
        Ok(gas_price.into())
    }

    /// Executes given transaction request on a current block
    ///
    /// * `call` - Transaction request object
    ///
    /// Returns result of execution
    pub fn call(&self, call: &TransactionRequest) -> ProgramResult<Bytes> {
        let value = emulator::eth_call(
            self.program_id(),
            RomeEvmUtil::cast_transaction_request(call, self.tx_builder.chain_id),
            self.sync_rpc_client(),
        )?;
        let bytes = value.into();
        Ok(bytes)
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
        TxBuilder::check_emulation(&emulation)?;

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
        if let Some(block_number) = self.get_block_number(block_number).await? {
            let base_fee_per_gas = vec![U256::from(1000); count as usize];
            let gas_used_ratio = vec![0.5; count as usize];
            let oldest_block = U256::from(block_number.as_u64() - count);
            let rewards = reward_percentiles
                .iter()
                .map(|&percentile| U256::from(percentile as u64))
                .collect::<Vec<U256>>();

            Ok(FeeHistory {
                base_fee_per_gas,
                gas_used_ratio,
                oldest_block,
                reward: vec![rewards],
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

    /// Runs emulation of a given transaction request on a latest block with commitment level
    /// configured during client construction.
    ///
    /// * `instruction` - Instruction to emulate
    /// * `data` - Instruction data
    ///
    /// Returns result of emulation
    pub fn emulate(
        &self,
        instruction: Instruction,
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

    /// Instruction is used to synchronize the initial state of contract with the state of op-geth.
    /// This private instruction is only available to the rollup owner, which was previously
    /// registered using the reg_owner instruction.
    /// It is not possible to overwrite the account state
    pub async fn create_balance(
        &self,
        address: Address,
        balance: U256,
        rollup_owner: &Keypair,
    ) -> ProgramResult<()> {
        let mut buf = [0; 32];
        balance.to_big_endian(&mut buf);

        let mut data = vec![Instruction::CreateBalance as u8];
        data.extend(address.as_bytes());
        data.extend(buf);
        data.extend(self.chain_id().to_le_bytes());

        let emulation = emulator::emulate(
            self.program_id(),
            &data,
            &rollup_owner.pubkey(),
            self.sync_rpc_client(),
        )?;

        let ix = self.tx_builder.build_ix(&emulation, data);
        let blockhash = self.rpc_client().get_latest_blockhash().await?;
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&rollup_owner.pubkey()),
            &[rollup_owner],
            blockhash,
        );
        let _ = self.rpc_client().send_and_confirm_transaction(&tx).await?;

        Ok(())
    }

    /// Instruction is used to registry rollup owner.
    /// After the registration the rollup owner is able to init state of the rollup by using
    /// the create_balance instruction.
    /// This private instruction must be signed with the upgrade-authority keypair that was used
    /// to deploy the rome-evm contract.
    pub async fn reg_owner(
        &self,
        rollup_owner_key: &Pubkey,
        chain_id: u64,
        upgrade_authority: &Keypair,
    ) -> ProgramResult<()> {
        let mut data = vec![Instruction::RegOwner as u8];
        data.extend(rollup_owner_key.to_bytes());
        data.extend(chain_id.to_le_bytes());

        let emulation = emulator::emulate(
            self.program_id(),
            &data,
            &upgrade_authority.pubkey(),
            self.sync_rpc_client(),
        )?;

        let ix = self.tx_builder.build_ix(&emulation, data);
        let blockhash = self.rpc_client().get_latest_blockhash().await?;
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&upgrade_authority.pubkey()),
            &[upgrade_authority],
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
}
