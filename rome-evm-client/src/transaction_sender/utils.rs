use {
    crate::{
        error::{Result, RomeEvmError::*},
        transaction_sender::{transaction_builder::TransactionBuilder, transaction_signer::Holder},
    },
    emulator::{Emulation, Instruction::*},
    ethers::types::TxHash,
    solana_client::client_error::{ClientError, ClientErrorKind},
    solana_program::{hash::Hash, instruction::Instruction},
    solana_rpc_client_api::{request, response},
    solana_sdk::{
        compute_budget::ComputeBudgetInstruction,
        signature::{Keypair, Signer},
        transaction::{Transaction, TransactionError},
    },
};

pub const TRANSMIT_TX_SIZE: usize = 800;
pub const MULTIPLE_ITERATIONS: u64 = 2; // estimated number of iterations is multiplied by this value

pub fn ro_lock_overrides(emulation: &Emulation) -> Result<Vec<u8>> {
    if emulation.accounts.len() > u8::MAX as usize {
        return Err(TooManyAccounts);
    }

    let mut overrides = vec![];
    for (ix, (_, item)) in emulation.accounts.iter().enumerate() {
        if !item.writable {
            overrides.push(ix as u8);
        }
    }

    Ok(overrides)
}

pub fn preflight_error(err: ClientError) -> Result<()> {
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

pub fn transmit_tx_data(offset: u64, bin: &[u8], holder: &Holder, hash: TxHash) -> Vec<u8> {
    let mut data = vec![TransmitTx as u8];
    data.extend(holder.index.to_le_bytes());
    data.extend(offset.to_le_bytes());
    data.extend(hash.as_bytes());
    data.extend_from_slice(bin);

    data
}

pub fn transmit_tx_list(
    rlp: &[u8],
    holder: &Holder,
    hash: TxHash,
    tx_sender: &TransactionBuilder,
    emulation: &Emulation,
) -> Vec<Instruction> {
    let mut rlp = rlp;
    let mut offset = 0;
    let mut result = vec![];

    let mut f = |a: &[u8]| {
        let data = transmit_tx_data(offset, a, holder, hash);
        offset += a.len() as u64;

        result.push(tx_sender.build_solana_instr(&emulation, &data));
    };

    while rlp.len() > TRANSMIT_TX_SIZE {
        let (left, right) = rlp.split_at(TRANSMIT_TX_SIZE);
        f(left);
        rlp = right;
    }
    f(rlp);

    result
}

pub fn build_solana_tx(
    recent_blockhash: Hash,
    payer: &Keypair,
    instructions: &[Instruction],
) -> Result<Transaction> {
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
