pub mod error;
pub mod indexer;
pub mod transaction_sender;
pub use emulator;
pub use rome_evm;

pub use indexer::*;
pub use transaction_sender::*;

use {
    crate::{
        error::{Result, RomeEvmError::*},
        indexer::{
            ethereum_block_storage::{BlockType, EthereumBlockStorage},
            indexer::{BlockReceiver, Indexer},
            transaction_data::TransactionData,
        },
        transaction_sender::{
            transaction_builder::TransactionBuilder, transaction_signer::TransactionSigner,
        },
    },
    emulator::{Emulation, Instruction},
    ethers::{
        types::{
            Address, BlockId, BlockNumber, Bytes, FeeHistory, NameOrAddress, TransactionRequest,
            TxHash, H256, U256, U64,
        },
        utils::keccak256,
    },
    rome_evm::{tx::legacy::Legacy as LegacyTx, H160 as EvmH160, U256 as EvmU256},
    solana_client::rpc_client::RpcClient,
    solana_sdk::{
        clock::Slot,
        commitment_config::CommitmentLevel,
        pubkey::Pubkey,
        signer::{keypair::Keypair, Signer},
        transaction::Transaction,
    },
    std::{ops::Deref, sync::Arc},
    tokio::select,
    tokio_util::sync::CancellationToken,
};

/// Helper function transforming Rome-SDK transaction data into Rome-EVM transaction data
fn legacy_tx_from_tx_request(value: &TransactionRequest) -> LegacyTx {
    let mut buf = [0; 32];

    let mut cast = |value: Option<U256>| -> rome_evm::U256 {
        value
            .map(|a| {
                a.to_big_endian(&mut buf);
                rome_evm::U256::from_big_endian(&buf)
            })
            .unwrap_or_default()
    };

    LegacyTx {
        nonce: value.nonce.unwrap_or_default().as_u64(), // todo: load from chain?
        gas_price: cast(value.gas_price),
        gas_limit: cast(value.gas),
        to: value.to.clone().map(|v| match v {
            NameOrAddress::Address(addr) => EvmH160::from(addr.0),
            NameOrAddress::Name(_) => EvmH160::default(),
        }),
        value: cast(value.value),
        data: value.data.clone().unwrap_or_default().to_vec(),
        chain_id: value.chain_id.map(|a| a.as_u64().into()),
        from: value.from.map(|v| EvmH160::from(v.0)).unwrap_or_default(),
        ..Default::default()
    }
}

/// Client component interacting with the instance of Rome-EVM smart-contract on Solana blockchain
/// (Rome-EVM Rollup).
///
/// Interface of RomeEVMClient is designed to be closely compatible with standard Ethereum JSON RPC
/// and mostly repeats its functionality.
pub struct RomeEVMClient {
    /// Chain ID of a Rollup
    pub chain_id: u64,

    /// Program ID of a Rollup smart-contract
    pub program_id: Pubkey,

    /// Solana keypair used to sign transactions to Rollup
    payer: Arc<Keypair>,

    /// Solana RPC client
    pub client: Arc<RpcClient>,

    /// Rollup indexer
    pub indexer: Arc<Indexer>,

    ethereum_block_storage: EthereumBlockStorage,

    tx_signer: TransactionSigner,

    /// Transaction builder
    pub tx_builder: TransactionBuilder,

    /// token to call Graceful Shutdown
    token: CancellationToken,
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
    pub fn new(
        chain_id: u64,
        program_id: Pubkey,
        payer: Arc<Keypair>,
        client: Arc<RpcClient>,
        number_holders: u64,
        commitment_level: CommitmentLevel,
        token: CancellationToken,
    ) -> Self {
        Self {
            chain_id,
            program_id,
            payer: payer.clone(),
            client: client.clone(),
            indexer: Arc::new(Indexer::new(
                program_id,
                Arc::clone(&client),
                commitment_level,
            )),
            ethereum_block_storage: EthereumBlockStorage::new(),
            tx_signer: TransactionSigner::new(payer, number_holders),
            tx_builder: TransactionBuilder::new(program_id, client),
            token,
        }
    }

    async fn start_receiver_loop(&self, block_receiver: &mut BlockReceiver) {
        while let Some(block) = block_receiver.recv().await {
            if let Err(err) = self.ethereum_block_storage.set_block(block) {
                tracing::warn!("Failed to write new block {:?}", err);
            }
        }
    }

    pub async fn start<T: FnOnce()>(&self, start_slot: Slot, on_started: T) {
        let (block_sender, mut block_receiver) = tokio::sync::mpsc::unbounded_channel();
        select! {
            res = self.indexer.start(start_slot, INDEXING_INTERVAL_MS, &block_sender, on_started) => {
                tracing::info!("Indexer stopped: {:?}", res);
            },
            res = self.start_receiver_loop(&mut block_receiver) => {
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
    pub async fn send_transaction(&self, rlp: Bytes) -> Result<TxHash> {
        let hash: TxHash = keccak256(rlp.clone()).into();
        self.tx_builder
            .build_transaction(rlp, hash, &self.tx_signer)?
            .send(
                self.client.clone(),
                self.tx_signer.keypair().clone(),
                self.token.clone(),
            )
            .await?;

        Ok(hash)
    }

    /// Returns balance of requested account on current block
    ///
    /// * `address` - address on an account
    pub fn get_balance(&self, address: Address) -> Result<U256> {
        let value = emulator::eth_get_balance(
            &self.program_id,
            &EvmH160::from(address.0),
            Arc::clone(&self.client),
        )?;

        let mut buf = [0; 32];
        value.to_big_endian(&mut buf);

        Ok(U256::from_big_endian(&buf))
    }

    /// Returns Chain ID of the Rollup
    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    /// Returns latest block number
    pub fn block_number(&self) -> Result<U64> {
        if let Some(block_number) = self.ethereum_block_storage.latest_block()? {
            Ok(block_number)
        } else {
            Err(Custom("Indexer is not started".to_string()))
        }
    }

    /// Returns current gas price
    pub fn gas_price(&self) -> Result<U256> {
        let gas_price = 1;
        Ok(gas_price.into())
    }

    /// Executes given transaction request on a current block
    ///
    /// * `call` - Transaction request object
    ///
    /// Returns result of execution
    pub fn call(&self, call: TransactionRequest) -> Result<Bytes> {
        let value = emulator::eth_call(
            &self.program_id,
            legacy_tx_from_tx_request(&call),
            Arc::clone(&self.client),
        )?;
        let bytes = value.into();
        Ok(bytes)
    }

    /// Returns transaction count (nonce) of a requested account in the latest block
    ///
    /// * `address` - address of account
    pub fn transaction_count(&self, address: Address) -> Result<U64> {
        let value = emulator::eth_get_tx_count(
            &self.program_id,
            &EvmH160::from(address.0),
            Arc::clone(&self.client),
        )?;
        Ok(value.into())
    }

    /// Estimate gas amount for a given transaction
    ///
    /// * `call` - transaction request to estimate gas
    pub fn estimate_gas(&self, call: TransactionRequest) -> Result<U256> {
        let emulation = emulator::eth_estimate_gas(
            &self.program_id,
            legacy_tx_from_tx_request(&call),
            Arc::clone(&self.client),
        )?;
        TransactionBuilder::check_revert(&emulation)?;

        Ok(emulation.gas.into())
    }

    fn get_block_number(&self, block_number: BlockId) -> Result<Option<U64>> {
        Ok(match block_number {
            BlockId::Number(number) => match number {
                BlockNumber::Latest => self.ethereum_block_storage.latest_block()?,
                BlockNumber::Number(number) => Some(U64::from(number)),
                other => {
                    tracing::warn!("Block tag {:?} not supported", other);
                    return Err(InternalError.into());
                }
            },
            _ => {
                tracing::warn!("Block hash not supported");
                return Err(InternalError.into());
            }
        })
    }

    /// Returns a collection of historical gas information for a specified number of blocks
    ///
    /// * `count` - Number of blocks to retrieve fee history for.
    /// * `block_number` - Starting block number for fee history retrieval.
    /// * `reward_percentiles` - Percentiles (0 to 100) for fee distribution.
    pub fn fee_history(
        &self,
        count: u64,
        block_number: BlockId,
        reward_percentiles: Vec<f64>,
    ) -> Result<FeeHistory> {
        if let Some(block_number) = self.get_block_number(block_number)? {
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
            Err(Custom("Indexer is not started".to_string()))
        }
    }

    /// Returns bytecode of a requested smart-contract in the latest block
    ///
    /// * `address` - Address of a smart-contract
    pub fn get_code(&self, address: Address) -> Result<Bytes> {
        let value = emulator::eth_get_code(
            &self.program_id,
            &EvmH160::from(address.0),
            Arc::clone(&self.client),
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
    pub fn get_block(
        &self,
        block_id: BlockId,
        full_transactions: bool,
    ) -> Result<Option<BlockType>> {
        if let Some(block_number) = self.get_block_number(block_id)? {
            Ok(
                if let Some(block) = self
                    .ethereum_block_storage
                    .get_block_by_number(block_number)?
                {
                    // Block found in the cache - return to client
                    if let Ok(lock) = self.indexer.get_transaction_storage().read() {
                        Some(block.get_block(full_transactions, lock.deref()))
                    } else {
                        tracing::warn!("Unable to get transaction storage lock");
                        None
                    }
                } else {
                    None
                },
            )
        } else {
            Err(Custom("Indexer is not started".to_string()))
        }
    }

    /// Retrieves the value stored at a specific storage slot for a given address.
    ///
    /// * `program_id` - The Pubkey of the program that owns the Ethereum account.
    /// * `address` - The Ethereum address (EvmH160) to retrieve the storage value from.
    /// * `slot` - The storage slot index (U256) to retrieve the value from.
    ///
    /// The corresponding storage value.
    // pub fn eth_get_storage_at(
    //     &self,
    //     program_id: Pubkey,
    //     address: EvmH160,
    //     slot: U256,
    // ) -> Result<U256> {
    //     let value = emulator::eth_get_storage_at(
    //         &self.program_id,
    //         &EvmH160::from(address.0),
    //         &slot,
    //         Arc::clone(&self.client),
    //     )?;

    //     let mut buf = [0; 32];
    //     value.to_big_endian(&mut buf);

    //     Ok(U256::from_big_endian(&buf))
    // }

    /// Finds transaction with a given transaction hash and executes <b>processor</b> function
    /// against this transaction
    ///
    /// * `tx_hash` - hash of requested transaction
    /// * `processor` - processor closure. Returns optional generic value -
    ///                 result of transaction processing
    ///
    /// Returns result of processing determined by <b>processor</b> function
    pub fn process_transaction<Ret>(
        &self,
        tx_hash: H256,
        processor: impl FnOnce(&TransactionData) -> Option<Ret>,
    ) -> Result<Option<Ret>> {
        self.indexer.map_tx(tx_hash, processor)
    }

    /// Retrieves the value stored at a specific storage slot for a given address.
    ///
    /// * `address` - The Ethereum address to retrieve the storage value from.
    /// * `slot` - The storage slot index to retrieve the value from.
    ///
    /// The corresponding storage value.
    pub fn eth_get_storage_at(&self, address: Address, slot: U256) -> Result<U256> {
        let mut buf = [0u8; 32];
        slot.to_big_endian(&mut buf);
        
        let value = emulator::eth_get_storage_at(
            &self.program_id,
            &EvmH160::from(address.0),
            &EvmU256::from_big_endian(&buf),
            Arc::clone(&self.client),
        )?;

        value.to_big_endian(&mut buf);

        Ok(U256::from_big_endian(&buf))
    }

    /// Runs emulation of a given transaction request on a latest block with commitment level
    /// configured during client construction.
    ///
    /// * `instruction` - Instruction to emulate
    /// * `data` - Instruction data
    ///
    /// Returns result of emulation
    pub fn emulate(&self, instruction: Instruction, data: &[u8]) -> Result<Emulation> {
        let mut bin = vec![instruction as u8];
        bin.extend(data);

        emulator::emulate(
            &self.program_id,
            &bin,
            &self.payer.pubkey(),
            self.client.clone(),
        )
        .map_err(|err| err.into())
    }

    /// Payer's public key
    pub fn payer_key(&self) -> Pubkey {
        self.payer.pubkey()
    }

    /// Register operator's gas recipient address in chain
    ///
    /// * `address` - Address of the operator to receive the payment for gas
    pub fn reg_gas_recipient(&self, address: Address) -> Result<()> {
        let mut data = vec![Instruction::RegSigner as u8];
        data.extend(address.as_bytes());

        let emulation = self.emulate(Instruction::RegSigner, address.as_bytes())?;
        let ix = self.tx_builder.build_solana_instr(&emulation, &data);

        let blockhash = self.client.get_latest_blockhash()?;

        let tx = Transaction::new_signed_with_payer(&[ix], Some(&self.payer.pubkey()), &[&self.payer], blockhash);
        let _ = self.client.send_and_confirm_transaction(&tx)?;
        Ok(())
    }
}
