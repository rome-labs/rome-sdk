use crate::indexer::solana_client::{GetConfirmedSignaturesForAddress2ConfigClone, SolanaClient};
use rome_solana::types::AsyncAtomicRpcClient;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config;
use solana_program::clock::Slot;
use solana_program::pubkey::Pubkey;
use solana_rpc_client_api::client_error::{Error, ErrorKind, Result as ClientResult};
use solana_rpc_client_api::config::{RpcBlockConfig, RpcTransactionConfig};
use solana_rpc_client_api::response::RpcConfirmedTransactionStatusWithSignature;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::signature::Signature;
use solana_transaction_status::{EncodedConfirmedTransactionWithStatusMeta, UiConfirmedBlock};
use std::future::Future;
use std::sync::Arc;

pub struct MultiplexedSolanaClient {
    pub providers: Vec<Arc<RpcClient>>,
}

impl MultiplexedSolanaClient {
    async fn plex<'a, T, F, Fut>(&'a self, op: F) -> ClientResult<T>
    where
        F: Fn(&'a RpcClient) -> Fut + Clone,
        Fut: Future<Output = ClientResult<T>>,
    {
        // create oneshot channel
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);

        // create futures for each provider
        let futs = self.providers.iter().map(|client| {
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
}

#[async_trait::async_trait]
impl SolanaClient for MultiplexedSolanaClient {
    async fn get_transaction_with_config(
        &self,
        signature: &Signature,
        config: RpcTransactionConfig,
    ) -> ClientResult<EncodedConfirmedTransactionWithStatusMeta> {
        self.plex(
            |client| async move { client.get_transaction_with_config(signature, config).await },
        )
        .await
    }

    async fn get_signatures_for_address_with_config(
        &self,
        address: &Pubkey,
        config: GetConfirmedSignaturesForAddress2ConfigClone,
    ) -> ClientResult<Vec<RpcConfirmedTransactionStatusWithSignature>> {
        self.plex(|client| async move {
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
    ) -> ClientResult<UiConfirmedBlock> {
        self.plex(|client| async move { client.get_block_with_config(slot, config).await })
            .await
    }

    async fn get_slot_with_commitment(
        &self,
        commitment_config: CommitmentConfig,
    ) -> ClientResult<Slot> {
        self.plex(|client| async move { client.get_slot_with_commitment(commitment_config).await })
            .await
    }
}

impl From<AsyncAtomicRpcClient> for MultiplexedSolanaClient {
    fn from(client: AsyncAtomicRpcClient) -> Self {
        Self {
            providers: vec![client.clone()],
        }
    }
}
