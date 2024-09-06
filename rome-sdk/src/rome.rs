use crate::tx::{RemusTx, RheaTx};
use crate::RomeConfig;
use rome_evm_client::error::Result as RomeEvmResult;
use rome_evm_client::error::RomeEvmError::{
    CrossRollupExecutionNotAvailable, Custom, NoChainId, UnsupportedChainId,
};
use rome_evm_client::transaction_sender::cross_rollup_tx::CrossRollupTx;
use rome_evm_client::transaction_sender::{
    transaction_builder::TransactionBuilder, transaction_signer::TransactionSigner, RomeTx,
};
use solana_client::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{read_keypair_file, Signature};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// A centralized structure that manages functionalities of the Rome network
pub struct Rome {
    /// signer
    signer: TransactionSigner,
    /// Mapping Chain_id to corresponding Rome-EVM transaction builder
    rollup_builders: BTreeMap<u64, TransactionBuilder>,
    /// Solana RPC client
    rpc_client: Arc<RpcClient>,
}

impl Rome {
    /// Create a new instance of [Rome]
    pub fn new(
        signer: TransactionSigner,
        rollup_builders: BTreeMap<u64, TransactionBuilder>,
        rpc_client: Arc<RpcClient>,
    ) -> Self {
        Self {
            signer,
            rollup_builders,
            rpc_client,
        }
    }

    /// Create a new instance of [Rome] from [RomeConfig]
    /// and start the services
    pub async fn new_with_config(config: RomeConfig) -> anyhow::Result<Self> {
        let blocking_rpc_client: Arc<RpcClient> = Arc::new(config.solana_config.into());

        let mut rollup_builders = BTreeMap::new();
        for (chain_id, rollup_pubkey) in config.rollups {
            rollup_builders.insert(
                chain_id,
                TransactionBuilder::new(
                    Pubkey::try_from(rollup_pubkey.as_str())
                        .map_err(|e| anyhow::anyhow!("Failed to parse program id: {:?}", e))?,
                    blocking_rpc_client.clone(),
                ),
            );
        }

        let signer = TransactionSigner::new(
            Arc::new(
                read_keypair_file(config.payer_path)
                    .map_err(|e| anyhow::anyhow!("Failed to read payer keypair file: {:?}", e))?,
            ),
            16, // num_holders
        );

        Ok(Self::new(signer, rollup_builders, blocking_rpc_client))
    }

    /// Compose a simple rollup transaction
    pub async fn compose_rollup_tx<'a>(&self, tx: RheaTx<'a>) -> RomeEvmResult<Box<dyn RomeTx>> {
        let chain_id = tx.tx().chain_id().ok_or(NoChainId)?.as_u64();
        if let Some(builder) = self.rollup_builders.get(&chain_id) {
            Ok(builder.build_transaction(
                tx.signed_rlp_bytes(),
                tx.tx().hash(tx.sig()),
                &self.signer,
            )?)
        } else {
            Err(UnsupportedChainId(chain_id))
        }
    }

    /// Compose a cross rollup transaction
    pub async fn compose_cross_rollup_tx<'a>(
        &self,
        tx: RemusTx<'a>,
    ) -> RomeEvmResult<Box<dyn RomeTx>> {
        let mut evm_transactions = Vec::new();
        for tx in tx.iter() {
            let chain_id = tx.tx().chain_id().ok_or(NoChainId)?.as_u64();
            if let Some(builder) = self.rollup_builders.get(&chain_id) {
                let evm_tx = builder.build_transaction(
                    tx.signed_rlp_bytes(),
                    tx.tx().hash(tx.sig()),
                    &self.signer,
                )?;
                if !evm_tx.cross_rollup_available() {
                    return Err(CrossRollupExecutionNotAvailable);
                }
                evm_transactions.push(evm_tx);
            } else {
                return Err(UnsupportedChainId(chain_id));
            }
        }

        Ok(Box::new(CrossRollupTx::new(evm_transactions)))
    }

    /// Send a [RomeTx] to the solana network
    pub async fn send_and_confirm_tx<'a>(&self, tx: Box<dyn RomeTx>) -> anyhow::Result<Signature> {
        let signatures = tx
            .send(
                self.rpc_client.clone(),
                self.signer.keypair().clone(),
                CancellationToken::new(),
            )
            .await?;
        Ok(signatures
            .last()
            .ok_or(Custom("No signatures".to_string()))?
            .clone())
    }
}
