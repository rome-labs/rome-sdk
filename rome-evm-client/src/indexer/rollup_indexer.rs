use crate::error::ProgramResult;
use crate::error::RomeEvmError::Custom;
use crate::indexer::parsers::tx_parser::TxParser;
use crate::indexer::{
    BlockParseResult, BlockParser, BlockProducer, EthereumBlockStorage, SolanaBlockStorage,
};
use solana_sdk::{clock::Slot, pubkey::Pubkey};
use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use tokio::{sync::RwLock, time::Duration};

const MAX_PARSE_BEHIND: Slot = 32;

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

    async fn parse_blocks(
        &self,
        from_slot: Slot,
        to_slot: Slot,
    ) -> ProgramResult<BTreeMap<Slot, BlockParseResult>> {
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

        Ok(parse_results)
    }

    async fn reg_blocks<P: BlockProducer>(
        &self,
        parse_results: BTreeMap<Slot, BlockParseResult>,
        block_producer: &P,
    ) -> ProgramResult<()> {
        if let Some((parent_hash, pending_blocks)) = self
            .ethereum_block_storage
            .register_parse_results(parse_results)
            .await?
        {
            let produced_blocks = block_producer
                .produce_blocks(parent_hash, &pending_blocks)
                .await?;

            self.ethereum_block_storage
                .blocks_produced(pending_blocks, produced_blocks)
                .await?;
        }

        Ok(())
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

    async fn parse_blocks_until_in_sync<P: BlockProducer>(
        &self,
        mut from_slot: Slot,
        interval: Duration,
        block_producer: &P,
    ) -> ProgramResult<Slot> {
        loop {
            from_slot = match self.solana_block_storage.get_last_slot().await {
                Ok(Some(to_slot)) if from_slot < to_slot => {
                    self.reg_blocks(self.parse_blocks(from_slot, to_slot).await?, block_producer)
                        .await?;
                    self.retain_storages(from_slot).await;
                    to_slot
                }
                Ok(_) => {
                    tokio::time::sleep(interval).await;
                    break Ok(from_slot);
                }
                Err(err) => {
                    tracing::warn!("Unable to get latest slot from slot storage: {:?}", err);
                    tokio::time::sleep(interval).await;
                    from_slot
                }
            };
        }
    }

    pub async fn start(self, start_slot: Option<Slot>, interval_ms: u64) -> ProgramResult<()> {
        let mut from_slot = self
            .ethereum_block_storage
            .get_max_slot_produced()
            .await?
            .or(start_slot)
            .ok_or(Custom(
                "start_slot is not set and there's no produced blocks in EthereumBlocksStorage"
                    .to_string(),
            ))?;

        tracing::info!("Indexer starting from slot: {:?}", from_slot);
        let duration = Duration::from_millis(interval_ms);
        loop {
            from_slot = self
                .parse_blocks_until_in_sync(from_slot, duration, &self.block_producer)
                .await?;
        }
    }
}
