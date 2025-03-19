use crate::indexer::{
    BlockParseResult, BlockType, ProducedBlocks, ProducerParams, ProgramResult, ReproduceParams,
};
use async_trait::async_trait;
use ethers::types::{Transaction, TransactionReceipt, H256, U64};
use solana_sdk::clock::Slot;
use std::collections::BTreeMap;

pub struct EthereumBlockStorage;

#[async_trait]
impl crate::indexer::EthereumBlockStorage for EthereumBlockStorage {
    async fn get_pending_blocks(&self) -> ProgramResult<Option<ProducerParams>> {
        Ok(None)
    }

    async fn reproduce_blocks(
        &self,
        _from_slot: Slot,
        _to_slot: Slot,
    ) -> ProgramResult<Option<ReproduceParams>> {
        Ok(None)
    }

    async fn register_parse_results(
        &self,
        _parse_results: BTreeMap<Slot, BlockParseResult>,
    ) -> ProgramResult<()> {
        Ok(())
    }

    async fn latest_block(&self) -> ProgramResult<Option<U64>> {
        Ok(None)
    }

    async fn get_block_number(&self, _hash: &H256) -> ProgramResult<Option<U64>> {
        Ok(None)
    }

    async fn get_block_by_number(
        &self,
        _number: U64,
        _full_transactions: bool,
    ) -> ProgramResult<Option<BlockType>> {
        Ok(None)
    }

    async fn blocks_produced(
        &self,
        _producer_params: &ProducerParams,
        _produced_blocks: ProducedBlocks,
    ) -> ProgramResult<()> {
        Ok(())
    }

    async fn get_max_slot_produced(&self) -> ProgramResult<Option<u64>> {
        Ok(None)
    }

    async fn retain_from_slot(&self, _from_slot: Slot) -> ProgramResult<()> {
        Ok(())
    }

    async fn get_transaction_receipt(
        &self,
        _tx_hash: &H256,
    ) -> ProgramResult<Option<TransactionReceipt>> {
        Ok(None)
    }

    async fn get_transaction(&self, _tx_hash: &H256) -> ProgramResult<Option<Transaction>> {
        Ok(None)
    }

    async fn get_slot_for_eth_block(&self, _block_number: U64) -> ProgramResult<Option<Slot>> {
        Ok(None)
    }

    async fn clean_from_slot(&self, _from_slot: Slot) -> ProgramResult<()> {
        Ok(())
    }
}
