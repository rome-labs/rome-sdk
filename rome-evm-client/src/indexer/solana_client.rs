use async_trait::async_trait;
use solana_program::clock::Slot;
use solana_program::pubkey::Pubkey;
use solana_rpc_client_api::config::RpcBlockConfig;
use solana_rpc_client_api::response::RpcConfirmedTransactionStatusWithSignature;
use solana_rpc_client_api::{client_error::Result as ClientResult, config::RpcTransactionConfig};
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::signature::Signature;
use solana_transaction_status::{EncodedConfirmedTransactionWithStatusMeta, UiConfirmedBlock};

#[derive(Clone, Debug, Default)]
pub struct GetConfirmedSignaturesForAddress2ConfigClone {
    pub before: Option<Signature>,
    pub until: Option<Signature>,
    pub limit: Option<usize>,
    pub commitment: Option<CommitmentConfig>,
}

#[async_trait]
pub trait SolanaClient: Send + Sync {
    async fn get_transaction_with_config(
        &self,
        signature: &Signature,
        config: RpcTransactionConfig,
    ) -> ClientResult<EncodedConfirmedTransactionWithStatusMeta>;

    async fn get_signatures_for_address_with_config(
        &self,
        address: &Pubkey,
        config: GetConfirmedSignaturesForAddress2ConfigClone,
    ) -> ClientResult<Vec<RpcConfirmedTransactionStatusWithSignature>>;

    async fn get_block_with_config(
        &self,
        slot: Slot,
        config: RpcBlockConfig,
    ) -> ClientResult<UiConfirmedBlock>;

    async fn get_slot_with_commitment(
        &self,
        commitment_config: CommitmentConfig,
    ) -> ClientResult<Slot>;
}
