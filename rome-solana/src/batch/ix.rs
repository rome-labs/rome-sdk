use std::borrow::Cow;

use solana_sdk::compute_budget::ComputeBudgetInstruction;
use solana_sdk::hash::Hash;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

/// An atomic batch of instructions that can be composed into a single transaction
#[derive(Clone)]
pub struct AtomicIxBatch<'a>(Cow<'a, [Instruction]>);

/// [AtomicIxBatch] which owns the instructions with a static lifetime
pub type OwnedAtomicIxBatch = AtomicIxBatch<'static>;

impl AtomicIxBatch<'static> {
    /// # Safety
    ///
    /// Create a new owned [IxBatch] from a list of [Instruction]s
    /// assuming batch size is less than or equal to the maximum transaction size
    pub fn new_owned(ixs: Vec<Instruction>) -> AtomicIxBatch<'static> {
        AtomicIxBatch(Cow::Owned(ixs))
    }

    /// Create multiple composible [IxBatch] from a list of [Instruction]s
    pub fn new_composible_batches_owned(ixs: Vec<Instruction>) -> Vec<AtomicIxBatch<'static>> {
        ixs.into_iter().map(Self::new_composible_owned).collect()
    }
    /// Add system instructions
    pub fn new_composible_owned(ix: Instruction) -> AtomicIxBatch<'static> {
        AtomicIxBatch(Cow::Owned(vec![
            ComputeBudgetInstruction::set_compute_unit_limit(1_400_000),
            ComputeBudgetInstruction::request_heap_frame(256 * 1024),
            ix,
        ]))
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
    /// Compose a [Transaction] from [IxBatch]
    pub fn compose_solana_tx(&self, payer: &Keypair, blockhash: Hash) -> Transaction {
        let message = Message::new(&self.0, Some(&payer.pubkey()));

        Transaction::new(&[payer], message, blockhash)
    }

    /// Compose a [Transaction] from [IxBatch] and signers
    pub fn compose_solana_tx_with_signers(
        &self,
        payer: &Keypair,
        signers: &[&Keypair],
        blockhash: Hash,
    ) -> Transaction {
        println!("compose_solana_tx_with_signers");
        let message = Message::new(&self.0, Some(&payer.pubkey()));

        let mut all_signers = vec![payer]; // Start with payer
        all_signers.extend_from_slice(signers); // Add other signers

        all_signers.iter().for_each(|signer| {
            println!("signer pubkey: {:?}", signer.pubkey());
        });
        Transaction::new(&all_signers, message, blockhash)
    }
}

impl std::ops::Deref for AtomicIxBatch<'_> {
    type Target = [Instruction];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
