use crate::error::ProgramResult;
use crate::error::RomeEvmError::Custom;
use crate::indexer::block_producers::block_producer::BlockProducer;
use crate::indexer::ethereum_block_storage::{ProducerParams, ReproduceParams};
use crate::indexer::parsers::{default_tx_parser::DefaultTxParser, TxParser};
use crate::indexer::produced_blocks::{BlockParams, ProducedBlocks};
use crate::indexer::{BlockParseResult, BlockParser, EthereumBlockStorage, SolanaBlockStorage};
use ethers::types::U64;
use solana_sdk::clock::Slot;
use std::collections::BTreeMap;
use std::ops::DerefMut;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinHandle;
use tokio::{sync::RwLock, time::Duration};

const MAX_PARSE_BEHIND: Slot = 64;
const BATCH_SIZE: usize = 1024;
const NUM_RETRIES: usize = 500;

pub struct RollupIndexer {
    tx_parser: Arc<RwLock<DefaultTxParser>>,
    block_parser: Arc<RwLock<BlockParser>>,
    solana_block_storage: Arc<dyn SolanaBlockStorage>,
    ethereum_block_storage: Arc<dyn EthereumBlockStorage>,
    block_producer: Option<Arc<dyn BlockProducer>>,
    max_slot_history: Option<Slot>,
}

impl RollupIndexer {
    pub fn new(
        block_parser: Arc<RwLock<BlockParser>>,
        solana_block_storage: Arc<dyn SolanaBlockStorage>,
        ethereum_block_storage: Arc<dyn EthereumBlockStorage>,
        block_producer: Option<Arc<dyn BlockProducer>>,
        max_slot_history: Option<Slot>,
    ) -> Self {
        Self {
            tx_parser: Arc::new(RwLock::new(DefaultTxParser::new())),
            block_parser,
            solana_block_storage,
            ethereum_block_storage,
            block_producer,
            max_slot_history,
        }
    }

    pub fn ethereum_block_storage(&self) -> Arc<dyn EthereumBlockStorage> {
        self.ethereum_block_storage.clone()
    }

    pub fn start(
        self,
        start_slot: Option<Slot>,
        interval_ms: u64,
        reorg_event_rx: UnboundedReceiver<Slot>,
    ) -> JoinHandle<ProgramResult<()>> {
        tokio::spawn(async move {
            let interval = Duration::from_millis(interval_ms);
            let start_slot = self.pre_run(start_slot, interval).await?;
            self.run(start_slot, interval, BATCH_SIZE as Slot, reorg_event_rx)
                .await
        })
    }

    async fn pre_run(&self, start_slot: Option<Slot>, interval: Duration) -> ProgramResult<Slot> {
        let Some(block_producer) = &self.block_producer else {
            return Ok(start_slot.unwrap_or_default());
        };

        let last_block_in_producer = block_producer.last_produced_block().await?;
        let last_block_in_storage = self.ethereum_block_storage.latest_block().await?;

        Ok(if let Some(last_block_in_storage) = last_block_in_storage {
            if last_block_in_storage > last_block_in_producer {
                self.reproduce_blocks_until_in_sync(
                    last_block_in_producer,
                    interval,
                    BATCH_SIZE as Slot,
                )
                .await?
            } else if let Some(result) = self
                .ethereum_block_storage
                .get_slot_for_eth_block(last_block_in_storage)
                .await?
            {
                result
            } else {
                return Err(Custom(format!(
                    "Slot not found for EthBlock {:?}",
                    last_block_in_storage
                )));
            }
        } else if let Some(last_slot_in_producer) = self
            .ethereum_block_storage
            .get_slot_for_eth_block(last_block_in_producer)
            .await?
        {
            last_slot_in_producer
        } else {
            start_slot.ok_or(Custom(
                "start_slot is not set and there's no produced blocks in BlockProducer".to_string(),
            ))?
        })
    }

    #[tracing::instrument(name = "rollup_indexer::reproduce_blocks_until_in_sync", skip(self), fields(height = ?last_known_block, batch_size = ?batch_size))]
    async fn reproduce_blocks_until_in_sync(
        &self,
        last_known_block: U64,
        interval: Duration,
        batch_size: Slot,
    ) -> ProgramResult<Slot> {
        let mut from_slot = self
            .ethereum_block_storage
            .get_slot_for_eth_block(last_known_block + 1)
            .await?
            .ok_or(Custom(format!(
                "Unable to find Solana slot for EthereumBlock {:?}",
                last_known_block + 1
            )))?;

        tracing::info!(
            "Reproducing Ethereum blocks starting from Solana slot {:?} ...",
            from_slot
        );

        loop {
            from_slot = match self.ethereum_block_storage.get_max_slot_produced().await {
                Ok(Some(mut to_slot)) if from_slot < to_slot + 1 => {
                    to_slot += 1;
                    let mut current_slot = from_slot;
                    while current_slot < to_slot {
                        current_slot = self
                            .reproduce_blocks(current_slot, to_slot, batch_size)
                            .await?;
                    }
                    to_slot
                }
                Ok(_) => {
                    tracing::info!("Block reproduction finished");
                    break Ok(from_slot);
                }
                Err(err) => {
                    tracing::warn!(
                        "Unable to get max produced Solana slot from EthereumBlockStorage: {:?}",
                        err
                    );
                    tokio::time::sleep(interval).await;
                    from_slot
                }
            }
        }
    }

    async fn reproduce_blocks(
        &self,
        from_slot: Slot,
        max_slot: Slot,
        batch_size: Slot,
    ) -> ProgramResult<Slot> {
        let Some(block_producer) = &self.block_producer else {
            return Ok(from_slot);
        };

        let to_slot = from_slot + std::cmp::min(max_slot - from_slot, batch_size);
        if let Some(ReproduceParams {
            producer_params,
            expected_results,
        }) = self
            .ethereum_block_storage
            .reproduce_blocks(from_slot, to_slot)
            .await?
        {
            let production_result = block_producer
                .produce_blocks(&producer_params, Some(batch_size as usize))
                .await?;

            // Check results of block reproduction
            for (slot_number, block_idx, expected_params) in expected_results.iter() {
                if let Some(new_block_params) = production_result
                    .produced_blocks
                    .get(slot_number, block_idx)
                {
                    if expected_params.ne(new_block_params) {
                        return Err(Custom(format!(
                            "Block reproduction failed at {:?}:\n Expected\n {:?} received {:?}",
                            (slot_number, block_idx),
                            expected_params,
                            new_block_params
                        )));
                    }
                } else {
                    return Err(Custom(format!(
                        "reproduce_blocks: Block {:?} was not reproduced",
                        (slot_number, block_idx)
                    )));
                }
            }
        }

        Ok(to_slot)
    }

    async fn run(
        self,
        mut from_slot: Slot,
        interval: Duration,
        batch_size: Slot,
        mut reorg_event_rx: UnboundedReceiver<Slot>,
    ) -> ProgramResult<()> {
        tracing::info!("Starting RollupIndexer from slot {from_slot}");
        loop {
            from_slot = tokio::select! {
                // Either continue parsing from latest slot of previous iteration
                res = self.restore_or_produce_until_in_sync(from_slot, interval, batch_size) => { res }

                // Or re-parse blocks after reorg happened
                res = reorg_event_rx.recv() => {
                    if let Some(res) = res {
                        self.ethereum_block_storage.clean_from_slot(res).await?;
                        Ok(res)
                    } else {
                        tracing::info!("Reorg event channel closed, exiting RollupIndexer");
                        break Ok(());
                    }
                }
            }?;
        }
    }

    #[tracing::instrument(name = "rollup_indexer::restore_or_produce_until_in_sync", skip(self), fields(from_slot = ?from_slot, batch_size = ?batch_size))]
    async fn restore_or_produce_until_in_sync(
        &self,
        from_slot: Slot,
        interval: Duration,
        batch_size: Slot,
    ) -> ProgramResult<Slot> {
        Ok(match self.solana_block_storage.get_last_slot().await {
            Ok(Some(to_slot)) if from_slot < to_slot => {
                let mut current_slot = from_slot;
                while current_slot < to_slot {
                    let (new_current_slot, parse_results) = self
                        .parse_blocks_batch(current_slot, to_slot, batch_size)
                        .await?;

                    self.ethereum_block_storage
                        .register_parse_results(parse_results)
                        .await?;

                    self.restore_or_produce_blocks(interval, batch_size as usize)
                        .await?;
                    self.retain_storages(current_slot).await;
                    current_slot = new_current_slot;
                }
                to_slot
            }
            Ok(_) => {
                tokio::time::sleep(interval).await;
                from_slot
            }
            Err(err) => {
                tracing::warn!("Unable to get latest slot from slot storage: {:?}", err);
                tokio::time::sleep(interval).await;
                from_slot
            }
        })
    }

    #[tracing::instrument(name = "rollup_indexer::parse_blocks_batch", skip(self), fields(from_slot = ?from_slot, max_slot = ?max_slot, batch_size = ?batch_size))]
    async fn parse_blocks_batch(
        &self,
        from_slot: Slot,
        max_slot: Slot,
        batch_size: Slot,
    ) -> ProgramResult<(Slot, BTreeMap<Slot, BlockParseResult>)> {
        let to_slot = from_slot + std::cmp::min(max_slot - from_slot, batch_size);
        tracing::debug!("Parsing blocks from_slot {from_slot}, to_slot {to_slot}");
        let mut parse_results = BTreeMap::new();
        let mut tx_parser = self.tx_parser.write().await;
        let mut block_parser = self.block_parser.write().await;

        for slot_number in from_slot..to_slot {
            if let Some(parse_result) = block_parser
                .parse(
                    slot_number,
                    MAX_PARSE_BEHIND as usize,
                    tx_parser.deref_mut(),
                )
                .await?
            {
                parse_results.insert(slot_number, parse_result);
            }
        }

        Ok((to_slot, parse_results))
    }

    #[tracing::instrument(name = "rollup_indexer::retain_storages", skip(self), fields(before_slot = ?before_slot))]
    async fn retain_storages(&self, before_slot: Slot) {
        if let Some(max_slot_history) = self.max_slot_history {
            let keep_slots = std::cmp::max(MAX_PARSE_BEHIND, max_slot_history);
            if before_slot > keep_slots {
                let from_slot = before_slot - keep_slots;
                if let Err(err) = self.solana_block_storage.retain_from_slot(from_slot).await {
                    tracing::warn!("Failed to remove Solana blocks: {:?}", err);
                } else {
                    self.tx_parser.write().await.retain_from_slot(from_slot);
                    if let Err(err) = self
                        .ethereum_block_storage
                        .retain_from_slot(from_slot)
                        .await
                    {
                        tracing::warn!("Failed to remove Ethereum blocks: {:?}", err);
                    }
                }
            }
        }
    }

    async fn restore_or_produce_blocks(
        &self,
        interval: Duration,
        batch_size: usize,
    ) -> ProgramResult<()> {
        let Some(block_producer) = &self.block_producer else {
            return Ok(());
        };

        let Some(mut producer_params) = self.ethereum_block_storage.get_pending_blocks().await?
        else {
            return Ok(());
        };

        let last_block_in_storage = self
            .ethereum_block_storage
            .latest_block()
            .await?
            .unwrap_or_default();
        let last_block_in_producer = block_producer.last_produced_block().await?;

        if last_block_in_storage + 1 < last_block_in_producer {
            self.restore_blocks(
                last_block_in_storage + 1,
                last_block_in_producer,
                producer_params,
                NUM_RETRIES,
                interval,
            )
            .await?;
        } else {
            loop {
                let production_result = block_producer
                    .produce_blocks(&producer_params, Some(batch_size))
                    .await?;

                if let Some((last_slot_number, last_block_idx, last_block_params)) =
                    production_result.produced_blocks.last_key_value()
                {
                    self.ethereum_block_storage
                        .blocks_produced(&producer_params, production_result.produced_blocks)
                        .await?;

                    producer_params.parent_hash = Some(last_block_params.hash);
                    producer_params
                        .pending_blocks
                        .retain_after(last_slot_number, last_block_idx);
                } else {
                    break;
                }
            }
        }

        Ok(())
    }

    #[tracing::instrument(name = "rollup_indexer::restore_blocks", skip(self, producer_params), fields(from_block = ?from_block, to_block = ?to_block))]
    async fn restore_blocks(
        &self,
        from_block: U64,
        to_block: U64,
        producer_params: ProducerParams,
        num_retries: usize,
        interval: Duration,
    ) -> ProgramResult<()> {
        let mut parent_hash = producer_params.parent_hash;
        let mut blocks_params = self
            .get_blocks_params(from_block, to_block, num_retries, interval)
            .await?;

        let mut produced_blocks = ProducedBlocks::default();
        for (slot_number, block_idx) in producer_params.pending_blocks.keys() {
            if let Some((_, block_params)) = blocks_params.pop_first() {
                if parent_hash != block_params.parent_hash {
                    return Err(Custom(format!(
                        "Ethereum block {:?} expected to have different parent_hash {:?}",
                        block_params, parent_hash
                    )));
                }

                parent_hash = Some(block_params.hash);
                produced_blocks.insert(*slot_number, *block_idx, block_params);
            } else {
                break;
            }
        }

        self.ethereum_block_storage
            .blocks_produced(&producer_params, produced_blocks)
            .await?;

        Ok(())
    }

    #[tracing::instrument(name = "rollup_indexer::get_blocks_params", skip(self), fields(from_block = ?from_block, to_block = ?to_block))]
    async fn get_blocks_params(
        &self,
        from_block: U64,
        to_block: U64,
        num_retries: usize,
        interval: Duration,
    ) -> ProgramResult<BTreeMap<U64, BlockParams>> {
        let futures = (from_block.as_u64()..(to_block + 1).as_u64()).map(|block_number| {
            self.get_block_params(U64::from(block_number), num_retries, interval)
        });

        let mut results = BTreeMap::new();
        for res in futures_util::future::join_all(futures).await {
            match res {
                Ok(params) => {
                    results.insert(params.number, params);
                }
                Err(e) => return Err(e),
            }
        }

        Ok(results)
    }

    async fn get_block_params(
        &self,
        block_number: U64,
        num_retries: usize,
        interval: Duration,
    ) -> ProgramResult<BlockParams> {
        let Some(block_producer) = &self.block_producer else {
            return Err(Custom(
                "BlockProducer is not set, can't get block params".to_string(),
            ));
        };

        let mut retries_remaining = num_retries;
        loop {
            match block_producer.get_block_params(block_number).await {
                Ok(params) => return Ok(params),
                Err(e) => {
                    retries_remaining -= 1;
                    if retries_remaining > 0 {
                        tracing::warn!(
                            "Failed to get params for block {:?}: {:?}",
                            block_number,
                            e
                        );

                        tokio::time::sleep(interval).await;
                        continue;
                    }

                    break Err(Custom(format!(
                        "Failed to load previously produced block {:?}",
                        block_number
                    )));
                }
            }
        }
    }
}
