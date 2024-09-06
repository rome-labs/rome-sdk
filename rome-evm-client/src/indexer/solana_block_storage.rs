use {
    crate::error::{Result, RomeEvmError::InternalError},
    solana_client::rpc_client::RpcClient,
    solana_rpc_client_api::{
        client_error::{Error as ClientError, ErrorKind},
        config::RpcBlockConfig,
    },
    solana_sdk::{
        commitment_config::{CommitmentConfig, CommitmentLevel},
        slot_hashes::Slot,
    },
    solana_transaction_status::UiConfirmedBlock,
    solana_transaction_status::{TransactionDetails, UiTransactionEncoding},
    std::collections::BTreeMap,
    std::ops::Index,
    std::sync::{Arc, RwLock},
};

pub struct BlockWithCommitment {
    #[allow(dead_code)]
    pub commitment_level: CommitmentLevel,
    pub block: UiConfirmedBlock,
}

#[derive(Clone)]
pub struct SolanaBlockStorage {
    client: Arc<RpcClient>,
    commitment_level: CommitmentLevel,
    block_cache: Arc<RwLock<BTreeMap<Slot, Arc<BlockWithCommitment>>>>,
}

impl SolanaBlockStorage {
    pub fn new(client: Arc<RpcClient>, commitment_level: CommitmentLevel) -> Self {
        Self {
            client,
            commitment_level,
            block_cache: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }

    fn reload_block(&self, slot_number: Slot) -> Result<Option<Arc<BlockWithCommitment>>> {
        match self.client.get_block_with_config(
            slot_number,
            RpcBlockConfig {
                encoding: Some(UiTransactionEncoding::Base58),
                transaction_details: Some(TransactionDetails::Full),
                rewards: None,
                commitment: Some(CommitmentConfig {
                    commitment: self.commitment_level,
                }),
                max_supported_transaction_version: None,
            },
        ) {
            Ok(block) => {
                let mut lock = self.block_cache.write()?;
                lock.insert(
                    slot_number,
                    Arc::new(BlockWithCommitment {
                        block,
                        commitment_level: self.commitment_level,
                    }),
                );
                Ok(Some(lock.index(&slot_number).clone()))
            }
            Err(ClientError { request: _, kind }) => {
                return match kind {
                    ErrorKind::SerdeJson(_) => Ok(None),
                    ErrorKind::RpcError(_) => Ok(None), // Skipping missed block
                    err => {
                        tracing::warn!("Unable to reload block {:?}", err);
                        Err(InternalError)
                    }
                }
            }
        }
    }

    pub fn get_block(&self, slot_number: Slot) -> Result<Option<Arc<BlockWithCommitment>>> {
        let cached_block = {
            let lock = self.block_cache.read()?;
            lock.get(&slot_number).map(|e| e.clone())
        };

        if let Some(block) = cached_block {
            Ok(Some(block.clone()))
        } else {
            // Block not found in the cache. Try to load from blockchain
            self.reload_block(slot_number)
        }
    }
}
