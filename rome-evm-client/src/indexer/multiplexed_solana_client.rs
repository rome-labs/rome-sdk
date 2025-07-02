use crate::error::RomeEvmError::Custom;
use crate::indexer::solana_client::{GetConfirmedSignaturesForAddress2ConfigClone, SolanaClient};
use crate::indexer::ProgramResult;
use rome_solana::types::AsyncAtomicRpcClient;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config;
use solana_program::clock::Slot;
use solana_program::pubkey::Pubkey;
use solana_rpc_client_api::client_error::{Error, ErrorKind, Result as ClientResult};
use solana_rpc_client_api::config::{RpcBlockConfig, RpcTransactionConfig};
use solana_rpc_client_api::request::RpcError;
use solana_rpc_client_api::response::RpcConfirmedTransactionStatusWithSignature;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::signature::Signature;
use solana_transaction_status::{EncodedConfirmedTransactionWithStatusMeta, UiConfirmedBlock};
use std::future::Future;
use std::sync::Arc;

async fn plex<'a, T, F, Fut>(providers: &'a Vec<Arc<RpcClient>>, op: F) -> ClientResult<T>
where
    F: Fn(&'a RpcClient) -> Fut + Clone,
    Fut: Future<Output = ClientResult<T>>,
{
    // create oneshot channel
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);

    // create futures for each provider
    let futs = providers.iter().map(|client| {
        // clone the channel
        let tx = tx.clone();
        let op = op.clone();

        async move {
            // send the request to the provider
            // and match the result
            match op(client).await {
                Ok(res) => {
                    // send the value to the channel
                    // ignore the result
                    let _ = tx.send(res).await;

                    // wait for the channel to close
                    tx.closed().await;

                    // the above future will be cancelled
                    unreachable!("Future should be cancelled");
                }
                Err(err) => err,
            }
        }
    });

    // wait for the first provider to respond
    // or for all providers to finish with errors
    tokio::select! {
        biased;

        // wait for the first provider to respond
        res = rx.recv() => {
            // close the channel
            rx.close();

            // the channel is not dropped yet
            // so we have a value
            Ok(res.unwrap())
        }

        // wait for all providers to finish
        mut errors = futures::future::join_all(futs) => {
            Err(if let Some(Error {kind, ..}) = errors.pop() {
                Error::from(kind)
            } else {
                ErrorKind::Custom(format!("All providers failed: {:?}", errors)).into()
            })
        }
    }
}

fn process_plex_result<T>(result: ClientResult<T>) -> ProgramResult<Option<T>> {
    match result {
        Ok(val) => Ok(Some(val)),
        Err(Error { kind, .. }) => {
            match kind {
                ErrorKind::SerdeJson(_) => return Ok(None),
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
                err => Err(Custom(format!("{:?}", err))),
            }
        }
    }
}

pub struct MultiplexedSolanaClient {
    pub providers: Vec<Arc<RpcClient>>,
    pub emergency_providers: Option<Vec<Arc<RpcClient>>>,
}

impl MultiplexedSolanaClient {
    async fn plex_with_retries<'a, T, F, Fut>(
        &'a self,
        op_name: &str,
        num_retries: usize,
        retry_delay: std::time::Duration,
        op: F,
    ) -> ProgramResult<Option<T>>
    where
        F: Fn(&'a RpcClient) -> Fut + Clone,
        Fut: Future<Output = ClientResult<T>>,
    {
        let mut retries_remaining = num_retries;
        loop {
            let err = match process_plex_result(plex(&self.providers, op.clone()).await) {
                Ok(res) => return Ok(res),
                Err(err) => err,
            };

            tracing::info!("Failed to run operation {:?}: {:?}", op_name, err);
            retries_remaining -= 1;

            if retries_remaining > 0 {
                tracing::info!(
                    "plex_with_retries({:?}) will retry after {:?}",
                    op_name,
                    retry_delay
                );
                tokio::time::sleep(retry_delay).await;
                continue;
            } else {
                break if let Some(emergency_providers) = &self.emergency_providers {
                    tracing::info!(
                        "plex_with_retries({:?}) will retry with emergency providers",
                        op_name
                    );
                    process_plex_result(plex(emergency_providers, op.clone()).await)
                } else {
                    Err(Custom(format!(
                        "plex_with_retries({:?}) retries exhausted",
                        op_name
                    )))
                };
            }
        }
    }
}

#[async_trait::async_trait]
impl SolanaClient for MultiplexedSolanaClient {
    async fn get_transaction_with_config(
        &self,
        signature: &Signature,
        config: RpcTransactionConfig,
        num_retries: usize,
        retry_delay: std::time::Duration,
    ) -> ProgramResult<Option<EncodedConfirmedTransactionWithStatusMeta>> {
        self.plex_with_retries(
            "get_transaction_with_config",
            num_retries,
            retry_delay,
            |client| async move { client.get_transaction_with_config(signature, config).await },
        )
        .await
    }

    async fn get_signatures_for_address_with_config(
        &self,
        address: &Pubkey,
        config: GetConfirmedSignaturesForAddress2ConfigClone,
    ) -> ClientResult<Vec<RpcConfirmedTransactionStatusWithSignature>> {
        plex(&self.providers, |client| async move {
            // Doing all this copy-paste just because the default solana implementation does not support Clone trait
            client
                .get_signatures_for_address_with_config(
                    address,
                    GetConfirmedSignaturesForAddress2Config {
                        before: config.before,
                        until: config.until,
                        limit: config.limit,
                        commitment: config.commitment,
                    },
                )
                .await
        })
        .await
    }

    async fn get_block_with_config(
        &self,
        slot: Slot,
        config: RpcBlockConfig,
        num_retries: usize,
        retry_delay: std::time::Duration,
    ) -> ProgramResult<Option<UiConfirmedBlock>> {
        self.plex_with_retries(
            "get_block_with_config",
            num_retries,
            retry_delay,
            |client| async move { client.get_block_with_config(slot, config).await },
        )
        .await
    }

    async fn get_slot_with_commitment(
        &self,
        commitment_config: CommitmentConfig,
    ) -> ClientResult<Slot> {
        plex(&self.providers, |client| async move {
            client.get_slot_with_commitment(commitment_config).await
        })
        .await
    }
}

impl From<AsyncAtomicRpcClient> for MultiplexedSolanaClient {
    fn from(client: AsyncAtomicRpcClient) -> Self {
        Self {
            providers: vec![client.clone()],
            emergency_providers: None,
        }
    }
}
