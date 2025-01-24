use crate::error::ProgramResult;
use crate::error::RomeEvmError::Custom;
use crate::indexer::SolanaBlockStorage;
use rome_solana::types::AsyncAtomicRpcClient;
use solana_client::client_error::ClientError;
use solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config;
use solana_program::clock::Slot;
use solana_program::pubkey::Pubkey;
use solana_rpc_client_api::client_error::ErrorKind;
use solana_rpc_client_api::config::{RpcBlockConfig, RpcTransactionConfig};
use solana_rpc_client_api::request::RpcError;
use solana_sdk::commitment_config::{CommitmentConfig, CommitmentLevel};
use solana_sdk::signature::Signature;
use solana_transaction_status::{
    EncodedConfirmedTransactionWithStatusMeta, EncodedTransactionWithStatusMeta,
    TransactionDetails, UiConfirmedBlock, UiTransactionEncoding,
};
use std::collections::{BTreeMap, HashSet};
use std::iter;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

const BLOCK_RETRIES: usize = 500;
const TX_RETRIES: usize = 500;

pub struct SolanaBlockLoader<S: SolanaBlockStorage + 'static> {
    solana_block_storage: Arc<S>,
    client: AsyncAtomicRpcClient,
    commitment: CommitmentLevel,
    program_id: Pubkey,
}

#[tracing::instrument(skip(client))]
async fn load_transaction(
    client: &AsyncAtomicRpcClient,
    program_id: Pubkey,
    signature: Signature,
    commitment: CommitmentLevel,
) -> ProgramResult<Option<EncodedConfirmedTransactionWithStatusMeta>> {
    match client
        .get_transaction_with_config(
            &signature,
            RpcTransactionConfig {
                encoding: Some(UiTransactionEncoding::Base58),
                commitment: Some(CommitmentConfig { commitment }),
                max_supported_transaction_version: Some(0),
            },
        )
        .await
    {
        Ok(tx) => {
            if let Some(vt) = tx.transaction.transaction.decode() {
                let accounts = vt.message.static_account_keys();
                for instruction in vt.message.instructions() {
                    if accounts[instruction.program_id_index as usize] == program_id {
                        return Ok(Some(tx));
                    }
                }
            }

            Ok(None)
        }

        Err(solana_rpc_client_api::client_error::Error { kind, .. }) => {
            match kind {
                ErrorKind::SerdeJson(_) => Ok(None),
                ErrorKind::RpcError(err) => {
                    match err {
                        RpcError::RpcResponseError { code, message, .. } => {
                            match code {
                                -32005 => {
                                    tracing::error!("Node is unhealthy: {:?}", message);
                                    Err(Custom(message))
                                }
                                -32011 => panic!("{:?}", message),
                                -32007 | -32009 => Ok(None), // Slot skipped
                                -32004 => Ok(None),          // Block for slot not available
                                _ => panic!("Unexpected RPC error: {:?}", message),
                            }
                        }
                        _ => Ok(None),
                    }
                }
                err => Err(Custom(format!(
                    "Failed to load block transactions: {:?}",
                    err
                ))),
            }
        }
    }
}

async fn load_transaction_with_retries(
    client: AsyncAtomicRpcClient,
    commitment: CommitmentLevel,
    program_id: Pubkey,
    signature: Signature,
    tx_retries: usize,
    retry_int: Duration,
) -> ProgramResult<Option<EncodedConfirmedTransactionWithStatusMeta>> {
    let mut retries_remaining = tx_retries;
    loop {
        match load_transaction(&client, program_id, signature, commitment).await {
            Ok(res) => break Ok(res),
            Err(err) => {
                tracing::info!("Failed to load transaction {:?}: {:?}", signature, err);
                retries_remaining -= 1;

                if retries_remaining > 0 {
                    tracing::info!("Will retry tx {:?} after {:?}", signature, retry_int);
                    tokio::time::sleep(retry_int).await;
                    continue;
                } else {
                    break Err(Custom(format!("Retries exhausted {:?}", signature)));
                }
            }
        }
    }
}

async fn load_transactions(
    client: AsyncAtomicRpcClient,
    commitment: CommitmentLevel,
    program_id: Pubkey,
    signatures: Vec<Signature>,
    tx_retries: usize,
    retry_int: Duration,
) -> ProgramResult<Option<Vec<EncodedTransactionWithStatusMeta>>> {
    let futures = signatures.into_iter().map(|signature| {
        load_transaction_with_retries(
            client.clone(),
            commitment,
            program_id,
            signature,
            tx_retries,
            retry_int,
        )
    });

    let txs = futures_util::future::join_all(futures)
        .await
        .into_iter()
        .collect::<ProgramResult<Vec<Option<EncodedConfirmedTransactionWithStatusMeta>>>>()?
        .into_iter()
        .flatten()
        .map(|tx| tx.transaction)
        .collect::<Vec<_>>();

    if txs.is_empty() {
        Ok(None)
    } else {
        Ok(Some(txs))
    }
}

async fn load_block_transactions(
    client: AsyncAtomicRpcClient,
    program_id: Pubkey,
    mut block: UiConfirmedBlock,
    commitment: CommitmentLevel,
    tx_retries: usize,
    retry_int: Duration,
) -> ProgramResult<UiConfirmedBlock> {
    let before = block
        .signatures
        .as_ref()
        .map(|sigs| sigs.last())
        .and_then(|sig| sig.and_then(|sig| Signature::from_str(sig).ok()));

    let until = block
        .signatures
        .as_ref()
        .map(|sigs| sigs.first())
        .and_then(|sig| sig.and_then(|sig| Signature::from_str(sig).ok()));

    // not needed anymore
    block.signatures = None;

    if let (Some(before), Some(until)) = (before, until) {
        let tx_signatures = iter::once(before)
            .chain(
                client
                    .get_signatures_for_address_with_config(
                        &program_id,
                        GetConfirmedSignaturesForAddress2Config {
                            commitment: Some(CommitmentConfig { commitment }),
                            before: Some(before),
                            until: Some(until),
                            limit: None,
                        },
                    )
                    .await?
                    .into_iter()
                    .filter_map(|status_with_signature| {
                        Signature::from_str(&status_with_signature.signature).ok()
                    }),
            )
            .chain(iter::once(until))
            .rev()
            .collect();

        block.transactions = load_transactions(
            client,
            commitment,
            program_id,
            tx_signatures,
            tx_retries,
            retry_int,
        )
        .await?;
    }

    Ok(block)
}

async fn load_block<S: SolanaBlockStorage + 'static>(
    client: AsyncAtomicRpcClient,
    commitment: CommitmentLevel,
    program_id: Pubkey,
    slot_number: Slot,
    solana_block_storage: Arc<S>,
    tx_retries: usize,
    retry_int: Duration,
) -> ProgramResult<Option<Arc<UiConfirmedBlock>>> {
    if solana_block_storage.get_block(slot_number).await?.is_none() {
        match client
            .get_block_with_config(
                slot_number,
                RpcBlockConfig {
                    encoding: Some(UiTransactionEncoding::Base58),
                    transaction_details: Some(TransactionDetails::Signatures),
                    rewards: None,
                    commitment: Some(CommitmentConfig { commitment }),
                    max_supported_transaction_version: Some(0),
                },
            )
            .await
        {
            Ok(block) => Ok(Some(Arc::new(
                load_block_transactions(
                    client, program_id, block, commitment, tx_retries, retry_int,
                )
                .await?,
            ))),
            Err(ClientError { request: _, kind }) => {
                match kind {
                    ErrorKind::SerdeJson(_) => Ok(None),
                    ErrorKind::RpcError(err) => {
                        match err {
                            RpcError::RpcResponseError { code, message, .. } => {
                                match code {
                                    -32005 => Err(Custom(message)),    // Node is unhealthy
                                    -32011 => panic!("{:?}", message), // Transaction history is not available from this node
                                    -32007 => Ok(None),
                                    -32009 => Err(Custom(message)), // Slot skipped
                                    -32004 => Err(Custom(message)), // Block for slot not available
                                    _ => Err(Custom(message)),
                                }
                            }
                            _ => Ok(None),
                        }
                    }
                    err => Err(Custom(format!("Failed to load block: {:?}", err))),
                }
            }
        }
    } else {
        Ok(None)
    }
}

impl<S: SolanaBlockStorage + 'static> SolanaBlockLoader<S> {
    pub fn new(
        solana_block_storage: Arc<S>,
        client: AsyncAtomicRpcClient,
        commitment: CommitmentLevel,
        program_id: Pubkey,
    ) -> Self {
        Self {
            solana_block_storage,
            client,
            commitment,
            program_id,
        }
    }

    async fn load_blocks(
        &self,
        mut slots: HashSet<Slot>,
    ) -> ProgramResult<BTreeMap<Slot, Arc<UiConfirmedBlock>>> {
        let mut retries_remaining = BLOCK_RETRIES;
        let retry_int = Duration::from_millis(500);

        let mut results = BTreeMap::new();
        loop {
            let futures = slots.iter().map(|slot_number| {
                load_block(
                    self.client.clone(),
                    self.commitment,
                    self.program_id,
                    *slot_number,
                    self.solana_block_storage.clone(),
                    TX_RETRIES,
                    retry_int,
                )
            });

            slots = slots
                .iter()
                .zip(futures_util::future::join_all(futures).await.into_iter())
                .filter_map(|(slot, result)| match result {
                    Err(err) => {
                        tracing::warn!("Failed to load block {:?}: {:?}", slot, err);
                        Some(*slot)
                    }
                    Ok(Some(block)) => {
                        results.insert(*slot, block);
                        None
                    }
                    _ => None,
                })
                .collect();

            retries_remaining -= 1;
            if !slots.is_empty() {
                if retries_remaining > 0 {
                    tracing::info!("Will retry loading...");
                    tokio::time::sleep(retry_int).await;
                    continue;
                } else {
                    return Err(Custom("Failed to load blocks".to_string()));
                }
            } else {
                break;
            }
        }

        Ok(results)
    }

    async fn preload_blocks(
        &self,
        from_slot: Slot,
        max_slot: Slot,
        num_preload_blocks: Slot,
    ) -> ProgramResult<Slot> {
        let to_slot = from_slot + std::cmp::min(max_slot - from_slot, num_preload_blocks);
        self.solana_block_storage
            .store_blocks(self.load_blocks((from_slot..to_slot).collect()).await?)
            .await?;
        Ok(to_slot)
    }

    async fn preload_blocks_until_in_sync(
        &self,
        mut from_slot: Slot,
        interval: Duration,
        num_preload_blocks: Slot,
    ) -> ProgramResult<Slot> {
        loop {
            from_slot = match self
                .client
                .get_slot_with_commitment(CommitmentConfig {
                    commitment: self.commitment,
                })
                .await
            {
                Ok(to_slot) if from_slot < to_slot => {
                    let mut current_slot = from_slot;
                    while current_slot < to_slot {
                        current_slot = self
                            .preload_blocks(current_slot, to_slot, num_preload_blocks)
                            .await?;
                    }
                    to_slot
                }
                Ok(_) => {
                    tokio::time::sleep(interval).await;
                    break Ok(from_slot);
                }
                Err(err) => {
                    tracing::warn!("Unable to get latest slot from Solana: {:?}", err);
                    tokio::time::sleep(interval).await;
                    from_slot
                }
            }
        }
    }

    pub async fn start(
        self,
        start_slot: Option<Slot>,
        interval_ms: u64,
        idx_started_tx: Option<oneshot::Sender<()>>,
        batch_size: Slot,
    ) -> ProgramResult<()> {
        let mut from_slot = self
            .solana_block_storage
            .get_last_slot()
            .await?
            .or(start_slot)
            .ok_or(Custom(
                "start_slot is not set and there's no registered blocks in SolanaBlockStorage"
                    .to_string(),
            ))?;

        tracing::info!("SolanaBlockLoader starting from slot: {:?}", from_slot);
        let sleep_duration = Duration::from_millis(interval_ms);
        from_slot = self
            .preload_blocks_until_in_sync(from_slot, sleep_duration, batch_size)
            .await?;

        tracing::info!("SolanaBlockLoader is in sync with Solana validator");
        if let Some(idx_started_tx) = idx_started_tx {
            idx_started_tx
                .send(())
                .expect("Failed to send SolanaBlockLoader started signal");
        }

        loop {
            from_slot = match self
                .preload_blocks_until_in_sync(from_slot, sleep_duration, batch_size)
                .await
            {
                Ok(res) => res,
                Err(err) => {
                    tracing::warn!(
                        "Failed to preload blocks starting from slot {:?}: {:?}",
                        from_slot,
                        err
                    );
                    from_slot
                }
            };
        }
    }
}
