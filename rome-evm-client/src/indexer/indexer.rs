use std::iter;
use std::ops::Deref;
use crate::error::{ProgramResult, RomeEvmError};
use crate::indexer::{
    block_parser::BlockParser, ethereum_block_storage::BlockData,
    solana_block_storage::SolanaBlockStorage, transaction_storage::TransactionStorage,
};
use ethers::types::TxHash;
use rome_solana::types::AsyncAtomicRpcClient;
use solana_sdk::{
    clock::Slot,
    commitment_config::{CommitmentConfig, CommitmentLevel},
    pubkey::Pubkey,
};
use std::sync::Arc;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::time::Duration;
use futures_util;
use solana_sdk::signature::Signature;
use std::str::FromStr;
use solana_client::client_error::ClientError;
use solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config;
use solana_rpc_client_api::client_error::ErrorKind;
use solana_rpc_client_api::config::{RpcBlockConfig, RpcTransactionConfig};
use solana_rpc_client_api::request::RpcError;
use solana_transaction_status::{TransactionDetails, UiConfirmedBlock, UiTransactionEncoding};
use crate::error::RomeEvmError::Custom;
use crate::indexer::solana_block_storage::BlockWithCommitment;

pub type BlockSender = UnboundedSender<Arc<BlockData>>;
pub type BlockReceiver = UnboundedReceiver<Arc<BlockData>>;

const MAX_PARSE_BEHIND: usize = 32;
const NUM_PRELOAD_BLOCKS: Slot = 128;
const MAX_RETRIES: Option<usize> = Some(10);

async fn load_block_transactions(
    client: AsyncAtomicRpcClient,
    program_id: &Pubkey,
    mut block: UiConfirmedBlock,
    commitment: CommitmentLevel,
) -> ProgramResult<BlockWithCommitment> {
    let before = block.signatures.as_ref()
        .map(|sigs| sigs.last())
        .and_then(|sig| sig.and_then(|sig| Signature::from_str(&sig).ok()));

    let until = block.signatures.as_ref()
        .map(|sigs| sigs.first())
        .and_then(|sig| sig.and_then(|sig| Signature::from_str(&sig).ok()));

    let mut transactions = vec![];
    if let (Some(before), Some(until)) = (before, until) {
        let evm_signatures = client.get_signatures_for_address_with_config(
            program_id,
            GetConfirmedSignaturesForAddress2Config {
                commitment: Some(CommitmentConfig { commitment }),
                before: Some(before),
                until: Some(until),
                limit: None,
            }
        )
            .await?
            .into_iter()
            .filter_map(|status_with_signature| Signature::from_str(&status_with_signature.signature).ok());

        let futures = iter::once(before)
            .chain(evm_signatures)
            .chain(iter::once(until))
            .rev()
            .map(|signature| {
                let client = client.clone();
                async move {
                    client.get_transaction_with_config(&signature, RpcTransactionConfig {
                        encoding: Some(UiTransactionEncoding::Base58),
                        commitment: Some(CommitmentConfig { commitment }),
                        max_supported_transaction_version: Some(0),
                    }).await
                }
            });

        let result = futures_util::future::join_all(futures).await;
        transactions.reserve(result.len());
        for res in result {
            match res {
                Ok(tx) => transactions.push(tx.transaction),
                Err(solana_rpc_client_api::client_error::Error { kind, .. }) => {
                    match kind {
                        ErrorKind::SerdeJson(_) => continue,
                        ErrorKind::RpcError(err) => {
                            match err {
                                RpcError::RpcResponseError {
                                    code,
                                    message,
                                    ..
                                } => {
                                    match code {
                                        -32005 => {
                                            tracing::error!("Node is unhealthy: {:?}", message);
                                            return Err(Custom(message))
                                        },
                                        -32011 => panic!("{:?}", message),
                                        -32007 | -32009 => continue,    // Slot skipped
                                        -32004 => continue,             // Block for slot not available
                                        _ => panic!("Unexpected RPC error: {:?}", message),
                                    }
                                }
                                _ => continue
                            }
                        },
                        err => return Err(Custom(
                            format!("Failed to load block transactions: {:?}", err)
                        ))
                    }
                },
            }
        }
    }

    block.transactions = Some(transactions);
    Ok(BlockWithCommitment {
        block,
        commitment_level: commitment,
    })
}

async fn load_block(
    client: AsyncAtomicRpcClient,
    commitment: CommitmentLevel,
    program_id: &Pubkey,
    slot_number: Slot,
) -> ProgramResult<Option<Arc<BlockWithCommitment>>> {
    match client
        .get_block_with_config(
            slot_number,
            RpcBlockConfig {
                encoding: Some(UiTransactionEncoding::Base58),
                transaction_details: Some(TransactionDetails::Signatures),
                rewards: None,
                commitment: Some(CommitmentConfig {
                    commitment: commitment,
                }),
                max_supported_transaction_version: Some(0),
            },
        )
        .await
    {
        Ok(block) => {
            Ok(Some(Arc::new(load_block_transactions(client, program_id, block, commitment).await?)))
        },
        Err(ClientError { request: _, kind }) => {
            match kind {
                ErrorKind::SerdeJson(_) => Ok(None),
                ErrorKind::RpcError(err) => {
                    match err {
                        RpcError::RpcResponseError {
                            code,
                            message,
                            ..
                        } => {
                            match code {
                                -32005 => {
                                    tracing::error!("Node is unhealthy: {:?}", message);
                                    Err(Custom(message))
                                },
                                -32011 => panic!("{:?}", message),
                                -32007 | -32009 => Ok(None),    // Slot skipped
                                -32004 => Ok(None),             // Block for slot not available
                                _ => panic!("Unexpected RPC error: {:?}", message),
                            }
                        }
                        err => Err(Custom(format!("Failed to load block: {:?}", err)))
                    }
                },
                err => Err(Custom(format!("Failed to load block: {:?}", err)))
            }
        }
    }
}

async fn preload_block<B: SolanaBlockStorage + 'static>(
    client: AsyncAtomicRpcClient,
    commitment: CommitmentLevel,
    program_id: Pubkey,
    slot_number: Slot,
    solana_block_storage: Arc<B>
) -> ProgramResult<()> {
    if solana_block_storage
        .get_block(slot_number)
        .await?
        .is_none()
    {
        if let Some(block) = load_block(client, commitment, &program_id, slot_number).await? {
            solana_block_storage
                .set_block(slot_number, block.clone())
                .await?;
        }
    }

    Ok(())
}

async fn parse_block<B: SolanaBlockStorage + 'static, T: TransactionStorage + 'static>(
    solana_block_storage: Arc<B>,
    transaction_storage: Arc<T>,
    chain_id: u64,
    slot: Slot,
    program_id: Pubkey,
) -> ProgramResult<Option<Arc<BlockData>>> {
    BlockParser::new(
        solana_block_storage.deref(),
        transaction_storage.deref(),
        chain_id,
    )
        .parse(slot, &program_id, MAX_PARSE_BEHIND)
        .await
}


#[derive(Clone)]
pub struct Indexer<
    S: SolanaBlockStorage + 'static,
    T: TransactionStorage + 'static
> {
    program_id: Pubkey,
    solana_block_storage: Arc<S>,
    transaction_storage: Arc<T>,
    commitment: CommitmentLevel,
    client: AsyncAtomicRpcClient,
    chain_id: u64,
}

impl<
    S: SolanaBlockStorage + 'static,
    T: TransactionStorage + 'static,
> Indexer<S, T> {
    pub fn new(
        program_id: Pubkey,
        solana_block_storage: S,
        transaction_storage: T,
        client: AsyncAtomicRpcClient,
        commitment: CommitmentLevel,
        chain_id: u64,
    ) -> Self {
        Self {
            program_id,
            solana_block_storage: Arc::new(solana_block_storage),
            transaction_storage: Arc::new(transaction_storage),
            commitment,
            client,
            chain_id,
        }
    }

    pub async fn map_tx<Ret>(
        &self,
        tx_hash: TxHash,
        processor: impl FnOnce(&T::TransactionType) -> Option<Ret>,
    ) -> ProgramResult<Option<Ret>> {
        let result =
            if let Some(tx_data) = self.transaction_storage.get_transaction(&tx_hash).await? {
                processor(&tx_data)
            } else {
                None
            };

        Ok(result)
    }

    pub fn get_transaction_storage(&self) -> &T {
        &self.transaction_storage
    }

    // Takes vector of slot numbers to preload returns vector of failed slots
    async fn preload_blocks_array(&self, slots: &Vec<Slot>) -> Vec<Slot> {
        let futures = slots
            .iter()
            .map(|slot_number|
                tokio::spawn(
                    preload_block(
                        self.client.clone(), self.commitment, self.program_id, *slot_number, self.solana_block_storage.clone()
                    )
                )
            );

        slots
            .iter()
            .zip(futures_util::future::join_all(futures).await.into_iter())
            .filter_map(|(slot, result)| {
                match result {
                    Err(_) => Some(*slot),
                    Ok(Err(_)) => Some(*slot),
                    _ => None,
                }
            })
            .collect()
    }

    async fn preload_blocks(&self, from_slot: Slot, to_slot: Slot) -> ProgramResult<()> {
        let mut retries_remaining = MAX_RETRIES.unwrap_or(usize::MAX);
        let mut slots: Vec<_> = (from_slot..to_slot).collect();
        loop {
            slots = self.preload_blocks_array(&slots).await;
            if slots.len() != 0 {
                tracing::info!("{:?} blocks failed to load", slots.len());
                retries_remaining -= 1;
                if retries_remaining <= 0 {
                    return Err(Custom(format!("Failed to preload blocks {:?} {:?}", from_slot, to_slot)))
                }
                tracing::info!("Will retry loading...")
            } else {
                break;
            }
        }

        Ok(())
    }

    async fn parse_blocks(&self, from_slot: Slot, to_slot: Slot, block_sender: &BlockSender) -> ProgramResult<()> {
        let futures = (from_slot..to_slot).map(|slot|
            tokio::spawn(
                parse_block(
                    self.solana_block_storage.clone(),
                    self.transaction_storage.clone(),
                    self.chain_id,
                    slot,
                    self.program_id
                )
            )
        );

        let results = futures_util::future::join_all(futures)
            .await
            .into_iter();

        for res in results {
            match res {
                Ok(res) => {
                    match res {
                        Ok(Some(block)) => {
                            if let Err(err) = block_sender.send(block.clone()).map_err(|_| RomeEvmError::TokioSendError) {
                                return Err(Custom(format!("Unable to send block {:?} : {:?}", block.number, err)))
                            }
                        }

                        Err(err) => {
                            tracing::warn!("Failed to parse blocks {:?} - {:?}", from_slot, to_slot);
                            return Err(err)
                        },
                        Ok(_) => {},
                    }
                },
                Err(err) => {
                    tracing::warn!("Failed to parse blocks {:?} - {:?}", from_slot, to_slot);
                    return Err(err.into())
                }
            }
        }

        Ok(())
    }

    async fn preload_and_parse_blocks(
        &self,
        from_slot: Slot,
        max_slot: Slot,
        block_sender: &BlockSender,
        num_preload_blocks: Slot,
    ) -> ProgramResult<Slot> {
        let to_slot = from_slot + std::cmp::min(max_slot - from_slot, num_preload_blocks);
        self.preload_blocks(from_slot, to_slot).await?;
        self.parse_blocks(from_slot, to_slot, block_sender).await?;

        if from_slot > MAX_PARSE_BEHIND as Slot {
            if let Err(err) = self
                .solana_block_storage
                .remove_blocks_before(from_slot - MAX_PARSE_BEHIND as Slot)
                .await
                {
                    tracing::warn!("Failed to remove outdated blocks: {:?}", err);
                }
        }

        Ok(to_slot)
    }

    async fn process_blocks_until_in_sync(
        &self,
        mut from_slot: Slot,
        interval: Duration,
        block_sender: BlockSender,
        num_preload_blocks: Slot,
    ) -> ProgramResult<Slot> {
        loop {
            let current_slot = self
                .client
                .get_slot_with_commitment(CommitmentConfig {
                    commitment: self.commitment,
                })
                .await;

            from_slot = match current_slot {
                Ok(to_slot) if from_slot < to_slot => {
                    let mut current_slot = from_slot;
                    while current_slot < to_slot {
                        current_slot = self.preload_and_parse_blocks(
                            current_slot, to_slot, &block_sender, num_preload_blocks
                        ).await?;
                    }
                    to_slot
                }
                Ok(_) => {
                    tokio::time::sleep(interval).await;
                    break Ok(from_slot)
                },
                Err(err) => {
                    tracing::warn!(
                        "Unable to get latest {:?} slot from Solana: {:?}",
                        self.commitment,
                        err
                    );
                    tokio::time::sleep(interval).await;
                    from_slot
                }
            };
        }
    }

    pub async fn start(
        &self,
        start_slot: Slot,
        interval_ms: u64,
        block_sender: BlockSender,
        idx_started_tx: Option<oneshot::Sender<()>>,
    ) {
        let duration = Duration::from_millis(interval_ms);
        tracing::info!("Indexer starting from slot: {:?}", start_slot);

        match self
            .process_blocks_until_in_sync(start_slot, duration, block_sender.clone(), NUM_PRELOAD_BLOCKS)
            .await {
            Ok(mut from_slot) => {
                tracing::info!("Indexer is in sync with Solana validator");
                if let Some(idx_started_tx) = idx_started_tx {
                    idx_started_tx
                        .send(())
                        .expect("Failed to send indexer started signal");
                }

                loop {
                    from_slot = match self
                        .process_blocks_until_in_sync(from_slot, duration, block_sender.clone(), NUM_PRELOAD_BLOCKS)
                        .await {
                        Ok(res) => res,
                        Err(err) => {
                            tracing::warn!("Failed to process block interval starting from slot {:?}: {:?}", from_slot, err);
                            from_slot
                        }
                    };
                }
            }
            Err(err) => tracing::warn!("Failed to start indexer: {:?}", err),
        }
    }
}
