use crate::error::ProgramResult;
use crate::error::RomeEvmError;
use emulator::Emulation;
use solana_client::client_error::{ClientError, ClientErrorKind};
use solana_program::{hash::Hash, instruction::Instruction};
use solana_rpc_client_api::{request, response};
use solana_sdk::{
    compute_budget::ComputeBudgetInstruction,
    signature::{Keypair, Signer},
    transaction::{Transaction, TransactionError},
};

pub const TRANSMIT_TX_SIZE: usize = 800;
pub const MULTIPLE_ITERATIONS: u64 = 2; // estimated number of iterations is multiplied by this value

pub fn ro_lock_overrides(emulation: &Emulation) -> ProgramResult<Vec<u8>> {
    if emulation.accounts.len() > u8::MAX as usize {
        return Err(RomeEvmError::TooManyAccounts);
    }

    let mut overrides = vec![];
    for (ix, (_, item)) in emulation.accounts.iter().enumerate() {
        if !item.writable {
            overrides.push(ix as u8);
        }
    }

    Ok(overrides)
}

pub fn preflight_error(err: ClientError) -> ProgramResult<()> {
    match &err.kind {
        ClientErrorKind::RpcError(request::RpcError::RpcResponseError {
            data:
                request::RpcResponseErrorData::SendTransactionPreflightFailure(
                    response::RpcSimulateTransactionResult {
                        err: Some(TransactionError::InstructionError(_, _)),
                        logs: Some(log),
                        ..
                    },
                ),
            ..
        }) => {
            let complete = log
                .iter()
                .any(|a| a.starts_with("Program log: Error: UnnecessaryIteration"));

            if complete {
                Ok(())
            } else {
                Err(err.into())
            }
        }
        _ => Err(err.into()),
    }
}

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
