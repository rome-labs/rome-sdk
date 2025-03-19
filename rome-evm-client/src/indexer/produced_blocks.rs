use ethers::prelude::{H256, U256, U64};
use serde::Deserialize;
use solana_program::clock::Slot;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BlockParams {
    pub hash: H256,
    pub parent_hash: Option<H256>,
    pub number: U64,
    pub timestamp: U256,
}

#[derive(Clone, Deserialize, Default)]
pub struct ProducedBlocks(pub(crate) BTreeMap<Slot, BTreeMap<usize, BlockParams>>);

impl ProducedBlocks {
    #![allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn first(&self) -> Option<&BlockParams> {
        self.0
            .first_key_value()
            .and_then(|(_, slot_blocks)| slot_blocks.first_key_value())
            .map(|(_, block_params)| block_params)
    }

    pub fn get(&self, slot_number: &Slot, block_idx: &usize) -> Option<&BlockParams> {
        self.0
            .get(slot_number)
            .and_then(|slot_blocks| slot_blocks.get(block_idx))
    }

    pub fn last_key_value(&self) -> Option<(Slot, usize, BlockParams)> {
        let (slot_number, slot_blocks) = self.0.last_key_value()?;
        let (block_idx, block_params) = slot_blocks.last_key_value()?;
        Some((*slot_number, *block_idx, block_params.clone()))
    }

    pub fn insert(&mut self, slot_number: Slot, block_idx: usize, block_params: BlockParams) {
        self.0
            .entry(slot_number)
            .or_default()
            .insert(block_idx, block_params);
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Slot, &usize, &BlockParams)> {
        self.0.iter().flat_map(|(slot_number, l2_blocks)| {
            l2_blocks
                .iter()
                .map(move |(block_idx, block_params)| (slot_number, block_idx, block_params))
        })
    }

    pub fn keys(&self) -> impl Iterator<Item = (&Slot, &usize)> {
        self.0.iter().flat_map(|(slot_number, l2_blocks)| {
            l2_blocks
                .keys()
                .map(move |block_idx| (slot_number, block_idx))
        })
    }
}
