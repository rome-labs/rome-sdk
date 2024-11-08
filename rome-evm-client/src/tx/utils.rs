use crate::error::ProgramResult;
use solana_program::{hash::Hash, instruction::Instruction};
use solana_sdk::{
    compute_budget::ComputeBudgetInstruction,
    signature::{Keypair, Signer},
    transaction::Transaction,
};

pub const TRANSMIT_TX_SIZE: usize = 800;
pub const MULTIPLE_ITERATIONS: f64 = 1.2; // estimated number of iterations is multiplied by this value

pub fn build_solana_tx(
    recent_blockhash: Hash,
    payer: &Keypair,
    instructions: &[Instruction],
) -> ProgramResult<Transaction> {
    Ok(Transaction::new_signed_with_payer(
        &[
            &[
                ComputeBudgetInstruction::set_compute_unit_limit(1_400_000),
                ComputeBudgetInstruction::request_heap_frame(256 * 1024),
            ],
            instructions,
        ]
        .concat(),
        Some(&payer.pubkey()),
        &[payer],
        recent_blockhash,
    ))
}
