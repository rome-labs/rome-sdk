use rome_solana::tower::SolanaTower;
use rome_solana::types::{AsyncAtomicRpcClient, SyncAtomicRpcClient};
use tokio::sync::oneshot;

use crate::error::{ProgramResult, RomeEvmError};
use crate::indexer::{
    ethereum_block_storage::{BlockType, EthereumBlockStorage},
    indexer::Indexer,
    transaction_data::TransactionData,
};
use crate::tx::TxBuilder;
use crate::util::RomeEvmUtil;
use emulator::{Emulation, Instruction};
use ethers::types::{
    Address, BlockId, BlockNumber, Bytes, FeeHistory, TransactionRequest, TxHash, H256, U256, U64,
};
use ethers::utils::keccak256;
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    clock::Slot,
    commitment_config::CommitmentLevel,
    pubkey::Pubkey,
    signer::{keypair::Keypair, Signer},
    transaction::Transaction,
};
use std::sync::Arc;

/// Client component interacting with the instance of Rome-EVM smart-contract on Solana blockchain
/// (Rome-EVM Rollup).
///
/// Interface of RomeEVMClient is designed to be closely compatible with standard Ethereum JSON RPC
/// and mostly repeats its functionality.
pub struct RomeEVMClient {
    /// Rollup indexer
    indexer: Arc<Indexer>,

    /// Ethereum block storage
    ethereum_block_storage: EthereumBlockStorage,

    /// Transaction builder
    tx_builder: TxBuilder,

    /// Solana Tower
    solana: SolanaTower,
}

const INDEXING_INTERVAL_MS: u64 = 400;

impl RomeEVMClient {
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
    pub fn
    new(
        chain_id: u64,
        program_id: Pubkey,
        solana: SolanaTower,
        commitment_level: CommitmentLevel,
    ) -> Self {
        let sync_client = Arc::new(RpcClient::new_with_commitment(
            solana.client().url(),
            solana.client().commitment(),
        ));

        Self {
            indexer: Arc::new(Indexer::new(
                program_id,
                solana.client_cloned(),
                commitment_level,
                chain_id,
            )),
            ethereum_block_storage: EthereumBlockStorage::new(),
            tx_builder: TxBuilder::new(chain_id, program_id, sync_client),
            solana,
        }
    }

    /// Start the indexer and consume blocks
    pub async fn start_indexing(
        &self,
        start_slot: Slot,
        idx_started_oneshot: Option<oneshot::Sender<()>>,
    ) {
        let (block_sender, mut block_receiver) = tokio::sync::mpsc::unbounded_channel();

        let recv_loop_jh = {
            let ethereum_block_storage = self.ethereum_block_storage.clone();

            tokio::spawn(async move {
                tracing::info!("Block consumer started");

                while let Some(block) = block_receiver.recv().await {
                    ethereum_block_storage.set_block(block).await;
                }
            })
        };

        let indexer_jh = {
            let indexer = self.indexer.clone();

            tokio::spawn(async move {
                indexer
                    .start(
                        start_slot,
                        INDEXING_INTERVAL_MS,
                        block_sender,
                        idx_started_oneshot,
                    )
                    .await
            })
        };

        tokio::select! {
            res = indexer_jh => {
                tracing::info!("Indexer stopped: {:?}", res);
            },
            res = recv_loop_jh => {
                tracing::info!("Block consumer stopped: {:?}", res);
            }
        }
    }

    /// Executes transaction in a Rollup smart-contract
    ///
    /// * `tx_request` - Transaction request object to execute
    /// * `eth_signature` - Signature of a transaction
    ///
    /// Returns transaction hash or error if transaction can not be executed
    pub async fn send_transaction(&self, rlp: Bytes, payer: &Keypair) -> ProgramResult<TxHash> {
        let hash: TxHash = keccak256(rlp.as_ref()).into();

        let mut tx = self.tx_builder.build_tx(rlp, hash, payer).await?;
        // let iter = EvmTxStepIterator::new(&*tx);

        self.solana
            .send_and_confirm_tx_iterable(&mut *tx, payer)
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
        if let Some(block_number) = self.ethereum_block_storage.latest_block().await {
            Ok(block_number)
        } else {
            Err(RomeEvmError::Custom("Indexer is not started".to_string()))
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
            RomeEvmUtil::cast_transaction_request(&call, self.tx_builder.chain_id),
            self.sync_rpc_client(),
        )?;
        TxBuilder::check_revert(&emulation)?;

        Ok(emulation.gas.into())
    }

    async fn get_block_number(&self, block_number: BlockId) -> ProgramResult<Option<U64>> {
        Ok(match block_number {
            BlockId::Number(number) => match number {
                BlockNumber::Latest => self.ethereum_block_storage.latest_block().await,
                BlockNumber::Number(number) => Some(number),
                other => {
                    tracing::warn!("Block tag {:?} not supported", other);
                    return Err(RomeEvmError::InternalError);
                }
            },
            BlockId::Hash(hash) => {
                self.ethereum_block_storage.get_block_by_hash(hash)
                    .await
                    .map(|block| block.number)
            },
        })
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
            Ok(
                if let Some(block) = self
                    .ethereum_block_storage
                    .get_block_by_number(block_number)
                    .await
                {
                    // Block found in the cache - return to client
                    let lock = self.indexer.get_transaction_storage();
                    let lock = lock.read().await;

                    Some(block.get_block(full_transactions, &lock))
                } else {
                    None
                },
            )
        } else {
            Err(RomeEvmError::Custom("Indexer is not started".to_string()))
        }
    }

    /// Finds transaction with a given transaction hash and executes <b>processor</b> function
    /// against this transaction
    ///
    /// * `tx_hash` - hash of requested transaction
    /// * `processor` - processor closure. Returns optional generic value -
    ///                 result of transaction processing
    ///
    /// Returns result of processing determined by <b>processor</b> function
    pub async fn process_transaction<Ret>(
        &self,
        tx_hash: H256,
        processor: impl FnOnce(&TransactionData) -> Option<Ret>,
    ) -> ProgramResult<Option<Ret>> {
        self.indexer.map_tx(tx_hash, processor).await
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

    /// Get indexer
    pub fn indexer(&self) -> Arc<Indexer> {
        self.indexer.clone()
    }

    /// Get reference to TxBuilder
    pub fn tx_builder(&self) -> &TxBuilder {
        &self.tx_builder
    }

    /// Register operator's gas recipient address in chain
    ///
    /// * `address` - Address of the operator to receive the payment for gas
    pub async fn reg_gas_recipient(&self, address: Address, payer: &Keypair) -> ProgramResult<()> {
        let mut data = vec![Instruction::RegSigner as u8];
        data.extend(address.as_bytes());
        data.extend(self.chain_id().to_le_bytes());

        let emulation = emulator::emulate(
            self.program_id(),
            &data,
            &payer.pubkey(),
            self.sync_rpc_client(),
        )?;

        let ix = self.tx_builder.build_ix(&emulation, data);
        let blockhash = self.rpc_client().get_latest_blockhash().await?;
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[payer], blockhash);
        let _ = self.rpc_client().send_and_confirm_transaction(&tx).await?;

        Ok(())
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
}
