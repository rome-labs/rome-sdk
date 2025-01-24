use crate::tx::{RemusTx, RheaTx, RomulusTx};
use crate::{RomeConfig, RomeTx};
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::{Address, TransactionRequest, U256};
use rome_evm_client::error::{ProgramResult, RomeEvmError};
use rome_evm_client::rome_evm::H160 as EvmH160;
use rome_evm_client::tx::CrossChainTx;
use rome_evm_client::tx::CrossRollupTx;
use rome_evm_client::tx::TxBuilder;
use rome_evm_client::util::{check_exit_reason, RomeEvmUtil};
use rome_evm_client::Resource;
use rome_evm_client::{emulator, resources::Payer};
use rome_solana::batch::AdvanceTx;
use rome_solana::batch::AtomicIxBatch;
use rome_solana::indexers::clock::SolanaClockIndexer;
use rome_solana::tower::SolanaTower;
use rome_solana::types::{AsyncAtomicRpcClient, SyncAtomicRpcClient};
use solana_sdk::compute_budget::ComputeBudgetInstruction;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature};
use std::collections::HashMap;
use std::sync::Arc;

/// A centralized structure that manages functionalities of the Rome network
pub struct Rome {
    // Payer keypair
    // payer: Keypair,
    // Solana tower
    solana: SolanaTower,
    /// Mapping Chai1n_id to corresponding Rome-EVM transaction builder
    rollup_builders: HashMap<u64, TxBuilder>,
}

impl Rome {
    /// Create a new instance of [Rome]
    pub fn new(
        // payer: Keypair,
        solana: SolanaTower,
        rollup_builders: HashMap<u64, TxBuilder>,
    ) -> Self {
        Self {
            // payer,
            solana,
            rollup_builders,
        }
    }

    /// Create a new instance of [Rome] from [RomeConfig]
    /// and start the services
    pub async fn new_with_config(config: RomeConfig) -> anyhow::Result<Self> {
        let sync_rpc_client: SyncAtomicRpcClient = Arc::new(config.solana_config.clone().into());
        let async_rpc_client: AsyncAtomicRpcClient = Arc::new(config.solana_config.into());

        let clock_indexer = SolanaClockIndexer::new(async_rpc_client.clone())
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create clock indexer: {:?}", e))?;

        let clock = clock_indexer.get_current_clock();

        // WARN: this needs to be spawned outside of the function
        // if the clock exists, it will fail all the transactions
        //
        // start the clock
        tokio::spawn(clock_indexer.start());

        let solana = SolanaTower::new(async_rpc_client, clock);

        let payers = Payer::from_config_list(&config.payers).await?;
        let rollup_builders = config
            .rollups
            .into_iter()
            .map(|(chain_id, rollup_pubkey)| {
                Pubkey::try_from(rollup_pubkey.as_str())
                    .map_err(|e| anyhow::anyhow!("Failed to parse program id: {:?}", e))
                    .map(|program_id| {
                        (
                            chain_id,
                            // TODO: use its own payer list for each rollup
                            TxBuilder::new(
                                chain_id,
                                program_id,
                                sync_rpc_client.clone(),
                                payers.clone(),
                            ),
                        )
                    })
            })
            .collect::<anyhow::Result<HashMap<_, _>>>()?;

        // let payer = SolanaKeyPayer::read_from_file(&config.payer_path).await?;

        Ok(Self {
            // payer: payer.into_keypair(),
            solana,
            rollup_builders,
        })
    }

    /// Get the transaction builder for the given chain_id
    pub fn get_transaction_builder(&self, chain_id: u64) -> ProgramResult<&TxBuilder> {
        self.rollup_builders
            .get(&chain_id)
            .ok_or(RomeEvmError::UnsupportedChainId(chain_id))
    }

    /// Get the transaction builder for the given transaction
    pub fn get_transaction_builder_for_tx(
        &self,
        tx: &TypedTransaction,
    ) -> ProgramResult<&TxBuilder> {
        let Some(chain_id) = tx.chain_id() else {
            return Err(RomeEvmError::NoChainId);
        };

        self.get_transaction_builder(chain_id.as_u64())
    }

    /// Returns transaction count (nonce) of a requested account in the latest block
    ///
    /// * `address` - address of account
    /// * `chain_id` - chain id
    pub fn transaction_count(&self, address: Address, chain_id: u64) -> ProgramResult<u64> {
        let tx_builder = self.get_transaction_builder(chain_id)?;

        // get the program id
        let program_id = tx_builder.program_id();

        // get the client
        let client = tx_builder.client_cloned();

        // get the transaction count
        let value =
            emulator::eth_get_tx_count(program_id, &EvmH160::from(address.0), client, chain_id)?;

        // convert to U64
        Ok(value)
    }

    /// Estimate gas amount for a given transaction
    ///
    /// * `tx` - transaction request to estimate gas
    pub fn estimate_gas(&self, tx: &TransactionRequest) -> ProgramResult<U256> {
        // get the chain id
        let Some(chain_id) = tx.chain_id else {
            return Err(RomeEvmError::NoChainId);
        };

        // get the transaction builder
        let tx_builder = self.get_transaction_builder(chain_id.as_u64())?;

        // get the program id
        let program_id = tx_builder.program_id();

        // get the client
        let client = tx_builder.client_cloned();

        let emulation = emulator::eth_estimate_gas(
            program_id,
            RomeEvmUtil::cast_transaction_request(tx, tx_builder.chain_id),
            client,
        )?;

        check_exit_reason(&emulation)?;

        Ok(emulation.gas.into())
    }

    /// Compose a simple rollup transaction
    pub async fn compose_rollup_tx<'a>(&self, tx: RheaTx<'a>) -> ProgramResult<RomeTx> {
        println!("\nCompose rollup tx\n");
        println!("Transaction {:?}", tx.tx());

        // get the transaction builder
        let builder = self.get_transaction_builder_for_tx(tx.tx())?;

        // get relevant data
        let rlp = tx.signed_rlp_bytes();
        let hash = tx.tx().hash(tx.sig());

        // build the transaction
        builder.build_tx(rlp, hash).await
    }

    /// Compose a cross rollup transaction
    pub async fn compose_cross_rollup_tx<'a>(&self, _tx: RemusTx<'a>) -> ProgramResult<RomeTx> {
        println!("\nCompose cross rollup tx\n");

        let mut instructions = vec![
            ComputeBudgetInstruction::set_compute_unit_limit(1_400_000),
            ComputeBudgetInstruction::request_heap_frame(256 * 1024),
        ];
        let mut resource: Option<Resource> = None;
        let mut vm_steps_executed = 0;
        let mut emulation_allocated = 0;
        let mut emulation_syscalls = 0;

        for tx in _tx.iter() {
            println!("Transaction {:?}", tx);

            let builder = self.get_transaction_builder_for_tx(tx.tx())?;
            let current_resource = builder.lock_resource().await?;
            if resource.is_none() {
                resource = Some(current_resource);
            }

            let rlp = tx.signed_rlp_bytes();

            let mut data = vec![emulator::Instruction::DoTx as u8];
            data.append(&mut resource.as_ref().unwrap().fee_recipient());
            data.extend_from_slice(rlp.as_ref());
            let emulation = builder.emulate(&data, &resource.as_ref().unwrap().payer_key())?;
            let vm = emulation.vm.as_ref().expect("Vm expected");

            vm_steps_executed += vm.steps_executed;
            emulation_allocated += emulation.allocated;
            emulation_syscalls += emulation.syscalls;

            let ix = builder.build_ix(&emulation, data);
            println!("Instruction {:?}", ix);
            instructions.push(ix);
        }

        println!(
            "VM steps executed: {}, allocated: {}, syscalls: {}",
            vm_steps_executed, emulation_allocated, emulation_syscalls
        );

        let is_atomic_tx = vm_steps_executed <= 500 // NUMBER_OPCODES_PER_TX
            && emulation_allocated <= 1_024 * 10 // MAX_PERMITTED_DATA_INCREASE
            && emulation_syscalls < 64;

        if !is_atomic_tx {
            return Err(RomeEvmError::Custom(
                "Transaction is too large or expensive".to_string(),
            ));
        }

        let resource = resource.ok_or_else(|| {
            RomeEvmError::Custom("Failed to acquire resource for Solana transaction".to_string())
        })?;

        Ok(Box::new(CrossRollupTx::new(
            AtomicIxBatch::new_owned(instructions),
            resource.payer(),
        )))
    }

    /// Compose a cross chain transaction
    pub async fn compose_cross_chain_tx<'a>(
        &self,
        romulus_tx: RomulusTx<'a>,
        signers: Vec<Arc<Keypair>>,
    ) -> ProgramResult<RomeTx> {
        println!("\nCompose cross chain tx\n");

        let mut instructions = vec![
            ComputeBudgetInstruction::set_compute_unit_limit(1_400_000),
            ComputeBudgetInstruction::request_heap_frame(256 * 1024),
        ];
        let mut resource: Option<Resource> = None;
        let mut vm_steps_executed = 0;
        let mut emulation_allocated = 0;
        let mut emulation_syscalls = 0;

        for tx in romulus_tx.eth_txs().iter() {
            println!("Eth Transaction {:?}", tx);

            let builder = self.get_transaction_builder_for_tx(tx.tx())?;
            let current_resource = builder.lock_resource().await?;
            if resource.is_none() {
                resource = Some(current_resource);
            }

            let rlp = tx.signed_rlp_bytes();

            let mut data = vec![emulator::Instruction::DoTx as u8];
            data.append(&mut resource.as_ref().unwrap().fee_recipient());
            data.extend_from_slice(rlp.as_ref());
            let emulation = builder.emulate(&data, &resource.as_ref().unwrap().payer_key())?;
            let vm = emulation.vm.as_ref().expect("Vm expected");

            vm_steps_executed += vm.steps_executed;
            emulation_allocated += emulation.allocated;
            emulation_syscalls += emulation.syscalls;

            let ix = builder.build_ix(&emulation, data);
            println!("Instruction {:?}", ix);
            instructions.push(ix);
        }
        for ix in romulus_tx.sol_ixs().iter() {
            println!("Sol Instruction {:?}", ix);

            instructions.push(ix.clone());
        }

        println!(
            "VM steps executed: {}, allocated: {}, syscalls: {}",
            vm_steps_executed, emulation_allocated, emulation_syscalls
        );

        let is_atomic_tx = vm_steps_executed <= 500 // NUMBER_OPCODES_PER_TX
                && emulation_allocated <= 1_024 * 10 // MAX_PERMITTED_DATA_INCREASE
                && emulation_syscalls < 64;

        if !is_atomic_tx {
            return Err(RomeEvmError::Custom(
                "Transaction is too large or expensive".to_string(),
            ));
        }

        let resource = resource.ok_or_else(|| {
            RomeEvmError::Custom("Failed to acquire resource for Solana transaction".to_string())
        })?;

        Ok(Box::new(CrossChainTx::new(
            AtomicIxBatch::new_owned(instructions),
            resource.payer(),
            signers,
        )))
    }

    /// Send and confirm
    pub async fn send_and_confirm(
        &self,
        tx: &mut dyn AdvanceTx<'_, Error = RomeEvmError>,
    ) -> anyhow::Result<Signature> {
        println!("\nsend_and_confirm\n");

        Ok(self
            .solana
            .send_and_confirm_tx_iterable(tx)
            .await?
            .into_iter()
            .last()
            .unwrap())
    }

    /// Get solana tower
    pub fn solana(&self) -> &SolanaTower {
        &self.solana
    }
}
