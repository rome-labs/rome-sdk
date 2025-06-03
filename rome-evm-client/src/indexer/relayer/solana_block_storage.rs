use crate::error::{ProgramResult, RomeEvmError};
use crate::indexer::pg_storage::PgPool;
use crate::indexer::SolanaBlockStorage;
use diesel::prelude::QueryableByName;
use diesel::{sql_types, RunQueryDsl};
use moka::future::Cache;
use rome_relayer::client::RelayerClient;
use rome_relayer::{get_block_request::Filter, GetBlockRequest, GetBlockResponse, GetSlotRequest};
use solana_sdk::clock::Slot;
use solana_sdk::transaction::{Legacy, TransactionVersion};
use solana_transaction_status::option_serializer::OptionSerializer;
use solana_transaction_status::{
    EncodedTransactionWithStatusMeta, TransactionBinaryEncoding, UiConfirmedBlock,
    UiTransactionStatusMeta,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::task;
use tokio::time::sleep;
use tracing::{error, warn};

/// [RelayerSolanaBlockStorage] is a [SolanaBlockStorage]
/// implementation that stores blocks in memory.
#[derive(Debug, Clone)]
pub struct RelayerSolanaBlockStorage {
    /// The client to interact with the relayer.
    client: Arc<Mutex<RelayerClient>>,
    pool: PgPool,
    block_cache: Cache<i64, Arc<UiConfirmedBlock>>,
}

#[derive(QueryableByName, Debug)]
struct FinalizedSlotRow {
    #[diesel(sql_type = sql_types::BigInt)]
    slot_number: i64,

    #[diesel(sql_type = sql_types::Bytea)]
    block_data: Vec<u8>,
}

impl RelayerSolanaBlockStorage {
    /// Create a new instance of [RelayerClient]
    pub async fn new(
        url: impl ToString,
        pool: PgPool,
    ) -> ProgramResult<Arc<dyn SolanaBlockStorage>> {
        let client = RelayerClient::init(url)
            .await
            .map_err(RomeEvmError::TransportError)?;
        Ok(Arc::new(Self {
            client: Arc::new(Mutex::new(client)),
            pool,
            block_cache: Cache::builder()
                .max_capacity(CACHE_MAX_CAPACITY)
                .time_to_idle(Duration::from_secs(CACHE_TIME_TO_IDLE_SECS))
                .time_to_live(Duration::from_secs(CACHE_TIME_TO_LIVE_SECS))
                .build(),
        }))
    }
}

#[async_trait::async_trait]
impl SolanaBlockStorage for RelayerSolanaBlockStorage {
    async fn store_blocks(
        &self,
        _blocks: BTreeMap<Slot, Arc<UiConfirmedBlock>>,
        _finalized_slot: Slot,
    ) -> ProgramResult<()> {
        panic!(
            "Unexpected call to RelayerSolanaBlockStorage::store_blocks.\
            Probably, you misconfigured RollupIndexer. \
            Relayer is not supposed to be used alongside with block loader."
        )
    }

    async fn update_finalized_blocks(
        &self,
        _blocks: BTreeMap<Slot, Arc<UiConfirmedBlock>>,
    ) -> ProgramResult<()> {
        panic!(
            "Unexpected call to RelayerSolanaBlockStorage::update_finalized_blocks.\
            Probably, you misconfigured RollupIndexer. \
            Relayer is not supposed to be used alongside with block loader."
        )
    }

    /// Get a block by slot number
    async fn get_block(&self, slot: Slot) -> ProgramResult<Option<Arc<UiConfirmedBlock>>> {
        let slot = slot.try_into().map_err(RomeEvmError::InvalidSlot)?;

        if let Some(cached_ui_block) = self.block_cache.get(&slot).await {
            return Ok(Some(cached_ui_block.clone()));
        }

        let block = self
            .client
            .lock()
            .await
            .get_block(GetBlockRequest {
                filter: Some(Filter::Slot(slot)),
            })
            .await;

        match block {
            Ok(block) => {
                let (block, is_finalized) = cast_block_response_to_ui_block(block.into_inner());
                let block = Arc::new(block);
                diesel::sql_query(
                    "INSERT INTO sol_slot (slot_number, parent_slot, status, blockhash, timestamp)
                            VALUES ($1, $2, $3::slotstatus, $4, $5)
                            ON CONFLICT DO NOTHING",
                )
                .bind::<sql_types::BigInt, _>(slot)
                .bind::<sql_types::BigInt, _>(block.parent_slot as i64)
                .bind::<sql_types::Text, _>(if is_finalized {
                    "Finalized"
                } else {
                    "Confirmed"
                })
                .bind::<sql_types::Text, _>(block.blockhash.clone())
                .bind::<sql_types::BigInt, _>(block.block_time.unwrap())
                .execute(&mut self.pool.get()?)?;

                if !is_finalized {
                    let relayer_storage = self.clone();
                    task::spawn(async move {
                        sleep(Duration::from_secs(WAIT_FOR_FINALIZATION)).await;

                        loop {
                            match relayer_storage
                                .client
                                .lock()
                                .await
                                .get_block(GetBlockRequest {
                                    filter: Some(Filter::Slot(slot)),
                                })
                                .await
                            {
                                Ok(resp) => {
                                    let (_, now_finalized) =
                                        cast_block_response_to_ui_block(resp.into_inner());
                                    if now_finalized {
                                        if let Err(err) = relayer_storage
                                            .set_finalized_slot(
                                                slot
                                                    .try_into()
                                                    .expect("Irrecoverable Error: Type Conversion cannot fail here!"),
                                            )
                                            .await
                                        {
                                            error!("Failed to set finalized slot {}: {}", slot, err);
                                        }
                                        break;
                                    }
                                }
                                Err(status) => {
                                    warn!(
                                        "Re-check error for slot {}: code={} msg=\"{}\"",
                                        slot,
                                        status.code(),
                                        status.message()
                                    );
                                }
                            }

                            sleep(Duration::from_secs(FINALIZATION_RE_CHECK_INTERVAL_SECS)).await;
                        }
                    });
                }

                self.block_cache.insert(slot, block.clone()).await;
                Ok(Some(block))
            }
            Err(status) => {
                if status.code() == tonic::Code::NotFound {
                    Ok(None)
                } else {
                    Err(RomeEvmError::RelayerError(status))
                }
            }
        }
    }

    async fn retain_from_slot(&self, _from_slot: Slot) -> ProgramResult<()> {
        // The relayer automatically retains the blocks.
        Ok(())
    }

    /// Get the last slot
    async fn get_last_slot(&self) -> ProgramResult<Option<Slot>> {
        let slot = self
            .client
            .lock()
            .await
            .get_slot(GetSlotRequest { finalized: false })
            .await;

        match slot {
            Ok(slot) => Ok(Some(slot.into_inner().slot)),
            Err(status) => {
                if status.code() == tonic::Code::NotFound {
                    Ok(None)
                } else {
                    Err(RomeEvmError::RelayerError(status))
                }
            }
        }
    }

    async fn set_finalized_slot(
        &self,
        slot: Slot,
    ) -> ProgramResult<BTreeMap<Slot, UiConfirmedBlock>> {
        let rows: Vec<FinalizedSlotRow> =
            diesel::sql_query("SELECT * FROM set_finalized_slot($1);")
                .bind::<sql_types::BigInt, _>(slot as i64)
                .load(&mut self.pool.get()?)?;

        let mut result = BTreeMap::new();
        for row in rows {
            result.insert(
                row.slot_number as Slot,
                serde_json::from_slice::<UiConfirmedBlock>(&row.block_data)?,
            );
        }

        Ok(result)
    }
}

fn cast_block_response_to_ui_block(block: GetBlockResponse) -> (UiConfirmedBlock, bool) {
    let GetBlockResponse {
        slot: _,
        block_hash: blockhash,
        parent_slot,
        prev_block_hash: previous_blockhash,
        block_time,
        block_height,
        txs,
        finalized,
    } = block;

    let (sigs, txs) = txs
        .into_iter()
        .map(|tx| {
            let rome_relayer::EncodedTransaction {
                signature,
                body,
                is_legacy,
                error,
                logs,
                version,
                fee,
            } = tx;

            let transaction = if is_legacy {
                solana_transaction_status::EncodedTransaction::LegacyBinary(body)
            } else {
                solana_transaction_status::EncodedTransaction::Binary(
                    body,
                    TransactionBinaryEncoding::Base58,
                )
            };

            let err = error
                .map(|x| bincode::deserialize(&x))
                .transpose()
                .unwrap_or_default();

            let meta = UiTransactionStatusMeta {
                err,
                // This field is deprecated and should be removed in the future.
                status: Ok(()),
                fee,
                // Not used in the relayer.
                pre_balances: Vec::new(),
                // Not used in the relayer.
                post_balances: Vec::new(),
                inner_instructions: OptionSerializer::None,
                log_messages: OptionSerializer::Some(logs),
                pre_token_balances: OptionSerializer::None,
                post_token_balances: OptionSerializer::None,
                rewards: OptionSerializer::None,
                loaded_addresses: OptionSerializer::None,
                return_data: OptionSerializer::None,
                compute_units_consumed: OptionSerializer::None,
            };

            let version = match version {
                -1 => TransactionVersion::Legacy(Legacy::Legacy),
                _ => TransactionVersion::Number(version as u8),
            };

            (
                signature,
                EncodedTransactionWithStatusMeta {
                    transaction,
                    meta: Some(meta),
                    version: Some(version),
                },
            )
        })
        .collect::<(Vec<_>, Vec<_>)>();

    (
        UiConfirmedBlock {
            blockhash,
            previous_blockhash,
            parent_slot,
            transactions: Some(txs),
            signatures: Some(sigs),
            rewards: None,
            num_reward_partitions: None,
            block_time: Some(block_time),
            block_height,
        },
        finalized,
    )
}

const CACHE_MAX_CAPACITY: u64 = 100;
const CACHE_TIME_TO_IDLE_SECS: u64 = 5 * 60;
const CACHE_TIME_TO_LIVE_SECS: u64 = 120 * 60;
const WAIT_FOR_FINALIZATION: u64 = 16;
const FINALIZATION_RE_CHECK_INTERVAL_SECS: u64 = 1;
