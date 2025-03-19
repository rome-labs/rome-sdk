use crate::indexer::TxResult;
use ethers::types::{Address, Transaction, H256, U256};
use serde::Serialize;
use solana_program::clock::Slot;
use std::collections::BTreeMap;

#[derive(Clone, Default, Serialize)]
pub struct PendingL1Block {
    pub blockhash: H256,
    pub timestamp: U256,
    pub l2_blocks: BTreeMap<usize, PendingL2Block>,
}

#[derive(Clone, Default, Serialize)]
pub struct PendingL2Block {
    pub transactions: BTreeMap<usize, (Transaction, TxResult)>,
    pub gas_recipient: Option<Address>,
}

#[derive(Clone, Serialize, Default)]
pub struct PendingBlocks(pub(crate) BTreeMap<Slot, PendingL1Block>);

impl PendingBlocks {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn get(&self, slot_number: &Slot, block_idx: &usize) -> Option<&PendingL2Block> {
        self.0
            .get(slot_number)
            .and_then(|slot_blocks| slot_blocks.l2_blocks.get(block_idx))
    }

    pub fn retain_after(&mut self, min_slot_number: Slot, min_block_idx: usize) {
        self.0.retain(
            |slot_number, pending_l1_block| match slot_number.cmp(&min_slot_number) {
                std::cmp::Ordering::Greater => true,
                std::cmp::Ordering::Equal => {
                    pending_l1_block
                        .l2_blocks
                        .retain(|block_idx, _| block_idx > &min_block_idx);
                    !pending_l1_block.l2_blocks.is_empty()
                }
                std::cmp::Ordering::Less => false,
            },
        )
    }

    pub fn contains(&self, slot_number: &Slot, block_idx: &usize) -> bool {
        let Some(slot_blocks) = self.0.get(slot_number) else {
            return false;
        };

        slot_blocks.l2_blocks.contains_key(block_idx)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Slot, &PendingL1Block, &usize, &PendingL2Block)> {
        self.0.iter().flat_map(|(slot_number, pending_l1_block)| {
            pending_l1_block
                .l2_blocks
                .iter()
                .map(move |(block_idx, pending_l2_block)| {
                    (slot_number, pending_l1_block, block_idx, pending_l2_block)
                })
        })
    }

    pub fn keys(&self) -> impl Iterator<Item = (&Slot, &usize)> {
        self.0.iter().flat_map(|(slot_number, pending_l1_block)| {
            pending_l1_block
                .l2_blocks
                .keys()
                .map(move |block_idx| (slot_number, block_idx))
        })
    }
}
