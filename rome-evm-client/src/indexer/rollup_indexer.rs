use crate::error::ProgramResult;
use crate::error::RomeEvmError::Custom;
use crate::indexer::parsers::tx_parser::TxParser;
use crate::indexer::{
    BlockParams, BlockParseResult, BlockParser, BlockProducer, EthereumBlockStorage, PendingBlocks,
    ProducedBlocks, SolanaBlockStorage,
};
use ethers::types::{H256, U64};
use solana_sdk::{clock::Slot, pubkey::Pubkey};
use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use tokio::task::JoinHandle;
use tokio::{sync::RwLock, time::Duration};

const MAX_PARSE_BEHIND: Slot = 32;
const BATCH_SIZE: usize = 1024;
const NUM_RETRIES: usize = 500;

#[derive(Clone)]
pub struct RollupIndexer<
    S: SolanaBlockStorage + 'static,
    E: EthereumBlockStorage + 'static,
    B: BlockProducer + 'static,
> {
    program_id: Pubkey,
    tx_parser: Arc<RwLock<TxParser>>,
    solana_block_storage: Arc<S>,
    ethereum_block_storage: Arc<E>,
    block_producer: B,
    max_slot_history: Option<Slot>,
}

impl<
        S: SolanaBlockStorage + 'static,
        E: EthereumBlockStorage + 'static,
        B: BlockProducer + 'static,
    > RollupIndexer<S, E, B>
{
    pub fn new(
        program_id: Pubkey,
        solana_block_storage: Arc<S>,
        ethereum_block_storage: Arc<E>,
        block_producer: B,
        max_slot_history: Option<Slot>,
    ) -> Self {
        Self {
            program_id,
            tx_parser: Arc::new(RwLock::new(TxParser::new())),
            solana_block_storage,
            ethereum_block_storage,
            block_producer,
            max_slot_history,
        }
    }

    pub async fn start(self, start_slot: Option<Slot>, interval_ms: u64) -> ProgramResult<()> {
        let interval = Duration::from_millis(interval_ms);

        let start_slot = self.pre_run(start_slot, interval).await?;
        self.run(start_slot, interval, BATCH_SIZE as Slot).await?
    }

    async fn pre_run(&self, start_slot: Option<Slot>, interval: Duration) -> ProgramResult<Slot> {
        let last_block_in_producer = self.block_producer.last_produced_block().await?;
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
        } else {
            if let Some(last_slot_in_producer) = self
                .ethereum_block_storage
                .get_slot_for_eth_block(last_block_in_producer)
                .await?
            {
                last_slot_in_producer
            } else {
                start_slot.ok_or(Custom(
                    "start_slot is not set and there's no produced blocks in BlockProducer"
                        .to_string(),
                ))?
            }
        })
    }

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
        let to_slot = from_slot + std::cmp::min(max_slot - from_slot, batch_size);
        tracing::info!("Reproducing blocks from_slot {from_slot}, to_slot {to_slot}");
        if let Some((parent_hash, reproduce_blocks)) = self
            .ethereum_block_storage
            .reproduce_blocks(from_slot, to_slot)
            .await?
        {
            let pending_blocks = reproduce_blocks
                .iter()
                .map(|(block_id, (pending, _))| (*block_id, pending.clone()))
                .collect();

            let new_produced_blocks = self
                .block_producer
                .produce_blocks(parent_hash, &pending_blocks)
                .await?;

            // Check results of block reproduction
            for (block_id, (_, old_block_params)) in reproduce_blocks {
                if let Some(new_block_params) = new_produced_blocks.get(&block_id) {
                    if &old_block_params != new_block_params {
                        return Err(Custom(format!(
                            "Block reproduction failed at {:?}:\n Expected\n {:?} received {:?}",
                            block_id, old_block_params, new_block_params
                        )));
                    }
                } else {
                    return Err(Custom(format!(
                        "reproduce_blocks: Block {:?} was not reproduced",
                        block_id
                    )));
                }
            }
        }

        Ok(to_slot)
    }

    fn run(
        self,
        mut from_slot: Slot,
        interval: Duration,
        batch_size: Slot,
    ) -> JoinHandle<ProgramResult<()>> {
        tokio::spawn(async move {
            loop {
                from_slot = match self.solana_block_storage.get_last_slot().await {
                    Ok(Some(to_slot)) if from_slot < to_slot => {
                        let mut current_slot = from_slot;
                        while current_slot < to_slot {
                            let (new_current_slot, parse_results) = self
                                .parse_blocks_batch(current_slot, to_slot, batch_size)
                                .await?;

                            if let Some((parent_hash, pending_blocks)) = self
                                .ethereum_block_storage
                                .register_parse_results(parse_results)
                                .await?
                            {
                                self.restore_or_produce_blocks(
                                    parent_hash,
                                    pending_blocks,
                                    interval,
                                )
                                .await?;
                            }

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
                };
            }
        })
    }

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
        let mut block_parser = BlockParser::new(
            self.solana_block_storage.deref(),
            tx_parser.deref_mut(),
            self.program_id,
            self.ethereum_block_storage.chain(),
        );

        for slot_number in from_slot..to_slot {
            if let Some(parse_result) = block_parser
                .parse(slot_number, MAX_PARSE_BEHIND as usize)
                .await?
            {
                parse_results.insert(slot_number, parse_result);
            }
        }

        Ok((to_slot, parse_results))
    }

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
        parent_hash: Option<H256>,
        pending_blocks: PendingBlocks,
        interval: Duration,
    ) -> ProgramResult<()> {
        let last_block_in_storage = self
            .ethereum_block_storage
            .latest_block()
            .await?
            .unwrap_or_default();
        let last_block_in_producer = self.block_producer.last_produced_block().await?;

        if last_block_in_storage + 1 < last_block_in_producer {
            self.restore_blocks(
                last_block_in_storage + 1,
                last_block_in_producer,
                parent_hash,
                pending_blocks,
                NUM_RETRIES,
                interval,
            )
            .await?;
        } else {
            let produced_blocks = self
                .block_producer
                .produce_blocks(parent_hash, &pending_blocks)
                .await?;

            self.ethereum_block_storage
                .blocks_produced(pending_blocks, produced_blocks)
                .await?;
        }

        Ok(())
    }

    async fn restore_blocks(
        &self,
        from_block: U64,
        to_block: U64,
        mut parent_hash: Option<H256>,
        pending_blocks: PendingBlocks,
        num_retries: usize,
        interval: Duration,
    ) -> ProgramResult<()> {
        let mut blocks_params = self
            .get_blocks_params(from_block, to_block, num_retries, interval)
            .await?;

        let mut produced_blocks = ProducedBlocks::new();
        for (block_id, _) in &pending_blocks {
            if let Some((_, block_params)) = blocks_params.pop_first() {
                if parent_hash != block_params.parent_hash {
                    return Err(Custom(format!(
                        "Ethereum block {:?} expected to have different parent_hash {:?}",
                        block_params, parent_hash
                    )));
                }

                parent_hash = Some(block_params.hash);
                produced_blocks.insert(*block_id, block_params);
            } else {
                break;
            }
        }

        self.ethereum_block_storage
            .blocks_produced(pending_blocks, produced_blocks)
            .await?;

        Ok(())
    }

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
        let mut retries_remaining = num_retries;
        loop {
            match self.block_producer.get_block_params(block_number).await {
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
