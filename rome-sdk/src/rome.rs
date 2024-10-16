use crate::tx::{RemusTx, RheaTx};
use crate::{RomeConfig, RomeTx};
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::Address;
use rome_evm_client::emulator;
use rome_evm_client::error::{ProgramResult, RomeEvmError};
use rome_evm_client::rome_evm::H160 as EvmH160;
use rome_evm_client::tx::TxBuilder;
use rome_solana::batch::AdvanceTx;
use rome_solana::indexers::clock::SolanaClockIndexer;
use rome_solana::payer::SolanaKeyPayer;
use rome_solana::tower::SolanaTower;
use rome_solana::types::{AsyncAtomicRpcClient, SyncAtomicRpcClient};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature};
use std::collections::HashMap;
use std::sync::Arc;

/// A centralized structure that manages functionalities of the Rome network
pub struct Rome {
    /// Payer keypair
    payer: Keypair,
    /// Solana tower
    solana: SolanaTower,
    /// Mapping Chain_id to corresponding Rome-EVM transaction builder
    rollup_builders: HashMap<u64, TxBuilder>,
}

impl Rome {
    /// Create a new instance of [Rome]
    pub fn new(
        payer: Keypair,
        solana: SolanaTower,
        rollup_builders: HashMap<u64, TxBuilder>,
    ) -> Self {
        Self {
            payer,
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

        let rollup_builders = config
            .rollups
            .into_iter()
            .map(|(chain_id, rollup_pubkey)| {
                Pubkey::try_from(rollup_pubkey.as_str())
                    .map_err(|e| anyhow::anyhow!("Failed to parse program id: {:?}", e))
                    .map(|program_id| {
                        (
                            chain_id,
                            TxBuilder::new(chain_id, program_id, sync_rpc_client.clone()),
                        )
                    })
            })
            .collect::<anyhow::Result<HashMap<_, _>>>()?;

        let payer = SolanaKeyPayer::read_from_file(&config.payer_path).await?;

        Ok(Self {
            payer: payer.into_keypair(),
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
            emulator::eth_get_tx_count(&program_id, &EvmH160::from(address.0), client, chain_id)?;

        // convert to U64
        Ok(value)
    }

    /// Compose a simple rollup transaction
    pub async fn compose_rollup_tx<'a>(&self, tx: RheaTx<'a>) -> ProgramResult<RomeTx> {
        // get the transaction builder
        let builder = self.get_transaction_builder_for_tx(tx.tx())?;

        // get relevant data
        let rlp = tx.signed_rlp_bytes();
        let hash = tx.tx().hash(tx.sig());
        let signer = &self.payer;

        // build the transaction
        builder.build_tx(rlp, hash, signer).await
    }

    /// Compose a cross rollup transaction
    pub async fn compose_cross_rollup_tx<'a>(&self, _tx: RemusTx<'a>) -> ProgramResult<RomeTx> {
        todo!("Not implemented yet")
    }

    /// Send and confirm
    pub async fn send_and_confirm(
        &self,
        tx: &mut dyn AdvanceTx<'_, Error = RomeEvmError>,
    ) -> anyhow::Result<Signature> {
        Ok(self
            .solana
            .send_and_confirm_tx_iterable(tx, &self.payer)
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
