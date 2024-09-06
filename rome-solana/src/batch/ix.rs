use std::borrow::Cow;

use solana_sdk::hash::Hash;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use crate::{
    SOLANA_COST_PER_BYTE, SOLANA_IX_BASE_CU, SOLANA_MAX_TX_COMPUTE_UNITS, SOLANA_MAX_TX_SIZE,
};

/// An atomic batch of instructions that can be composed into a single transaction
pub struct AtomicIxBatch<'a>(Cow<'a, [Instruction]>);

impl AtomicIxBatch<'static> {
    /// # Safety
    ///
    /// Unsafely Create a new owned [IxBatch] from a list of [Instruction]s
    /// assuming batch size is less than or equal to the maximum transaction size
    pub unsafe fn new_owned(ixs: Vec<Instruction>) -> AtomicIxBatch<'static> {
        AtomicIxBatch(Cow::Owned(ixs))
    }

    /// Create multiple composible [IxBatch] from a list of [Instruction]s
    pub fn new_composible_batches_owned(ixs: Vec<Instruction>) -> Vec<AtomicIxBatch<'static>> {
        let bucket_sizes = Self::split_ixs_into_composable_ixs(&ixs)
            .into_iter()
            .map(|refs| refs.len())
            .collect::<Vec<_>>();

        let mut ixs_iter = ixs.into_iter();
        let mut batch = Vec::with_capacity(bucket_sizes.len());

        for size in bucket_sizes.iter() {
            let mut ixs = Vec::with_capacity(*size);

            for _ in 0..*size {
                ixs.push(ixs_iter.next().unwrap());
            }

            batch.push(Self(Cow::Owned(ixs)));
        }

        batch
    }
}

impl AtomicIxBatch<'_> {
    /// # Safety
    ///
    /// Unsafely Create a new borrowed [IxBatch] from a list of [Instruction]s
    /// assuming batch size is less than or equal to the maximum transaction size
    pub unsafe fn new_borrowed(ixs: &[Instruction]) -> AtomicIxBatch {
        AtomicIxBatch(Cow::Borrowed(ixs))
    }

    /// Create multiple composible [IxBatch] from a list of [Instruction]s
    pub fn new_composable_batches(ixs: &[Instruction]) -> Vec<AtomicIxBatch> {
        AtomicIxBatch::split_ixs_into_composable_ixs(ixs)
            .into_iter()
            .map(|ixs| AtomicIxBatch(Cow::Borrowed(ixs)))
            .collect()
    }

    /// Compose a [Transaction] from [IxBatch]
    pub fn compose_solana_tx(&self, payer: &Keypair, blockhash: Hash) -> Transaction {
        let message = Message::new(&self.0, Some(&payer.pubkey()));

        Transaction::new(&[payer], message, blockhash)
    }

    /// Given a list of instructions, split them into composable instruction lists.
    /// This is useful when the instructions are too large to fit or
    /// are too computationally expensive to fit into a single transaction.
    ///
    /// Note: expensive call, instructions are estimated based on the size of the instruction
    /// calculated using bincode serialization
    ///
    /// The cost of an instruction is calculated as:
    /// A base cost of 500 compute units + 10 compute units per byte.
    pub fn split_ixs_into_composable_ixs(instructions: &[Instruction]) -> Vec<&[Instruction]> {
        let mut result: Vec<&[Instruction]> = Vec::with_capacity(instructions.len() / 2);

        let mut current_batch_size: usize = 0;
        let mut current_batch_cu: usize = 0;

        let mut last_batch_index = 0;

        for (index, ix) in instructions.iter().enumerate() {
            let instruction_size = bincode::serialized_size(&ix).unwrap() as usize;
            let instruction_cu = SOLANA_IX_BASE_CU + (SOLANA_COST_PER_BYTE * instruction_size);

            // Check if adding this instruction would exceed the limits
            if current_batch_size + instruction_size > SOLANA_MAX_TX_SIZE
                || current_batch_cu + instruction_cu > SOLANA_MAX_TX_COMPUTE_UNITS
            {
                // If yes, save the current batch and start a new one
                result.push(&instructions[last_batch_index..index]);
                last_batch_index = index;
                current_batch_size = 0;
                current_batch_cu = 0;
            }

            current_batch_size += instruction_size;
            current_batch_cu += instruction_cu;
        }

        // Don't forget to push the last batch if it contains any instructions
        if current_batch_size != 0 {
            result.push(&instructions[last_batch_index..]);
        }

        result
    }
}
