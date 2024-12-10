use super::utils::TRANSMIT_TX_SIZE;
use rome_solana::batch::{AdvanceTx, IxExecStepBatch, OwnedAtomicIxBatch};
use solana_program::entrypoint::MAX_PERMITTED_DATA_INCREASE;

use super::builder::TxBuilder;
use crate::{
    error::{ProgramResult, RomeEvmError},
    Resource,
};
use async_trait::async_trait;
use ethers::types::{Bytes, TxHash};
use rome_utils::iter::into_chunks;
use solana_sdk::signature::Keypair;
use std::sync::Arc;

pub struct TransmitTx {
    pub tx_builder: TxBuilder,
    pub rlp: Bytes,
    pub hash: TxHash,
    pub resource: Resource,
    step: Steps,
}

#[derive(Clone)]
enum Steps {
    Init,
    Execute(Vec<Vec<OwnedAtomicIxBatch>>),
    Complete,
}

impl TransmitTx {
    pub fn new(tx_builder: TxBuilder, resource: Resource, rlp: Bytes, hash: TxHash) -> Self {
        Self {
            tx_builder,
            rlp,
            hash,
            resource,
            step: Steps::Init,
        }
    }

    pub fn tx_data(&self, offset: u64, bin: Vec<u8>) -> Vec<u8> {
        let mut data = vec![emulator::Instruction::TransmitTx as u8];
        data.extend(self.resource.holder());
        data.extend(offset.to_le_bytes());
        data.extend(self.hash.as_bytes());
        data.extend(self.tx_builder.chain_id.to_le_bytes());
        data.extend(bin);

        data
    }

    fn ixs(&self) -> ProgramResult<Vec<OwnedAtomicIxBatch>> {
        let data = self.tx_data(0, self.rlp.to_vec());
        let emulation = self.tx_builder.emulate(&data, &self.resource.payer_key())?;

        let mut offset = 0;

        let ixs = into_chunks(self.rlp.to_vec(), TRANSMIT_TX_SIZE)
            .into_iter()
            .map(|chunk| {
                let new_offset = offset + chunk.len() as u64;
                let data = self.tx_data(offset, chunk);
                offset = new_offset;
                data
            })
            .map(|data| self.tx_builder.build_ix(&emulation, data))
            .map(|ix| OwnedAtomicIxBatch::new_owned(vec![ix]))
            .collect();

        Ok(ixs)
    }
}

#[async_trait]
impl AdvanceTx<'_> for TransmitTx {
    type Error = RomeEvmError;
    fn advance(&mut self) -> ProgramResult<IxExecStepBatch<'static>> {
        match &mut self.step {
            Steps::Init => {
                let ixs = self.ixs()?;
                let limit = MAX_PERMITTED_DATA_INCREASE / TRANSMIT_TX_SIZE;
                let mut batches = into_chunks(ixs, limit);
                batches.reverse();

                self.step = Steps::Execute(batches);
                self.advance()
            }
            Steps::Execute(batches) => {
                if let Some(batch) = batches.pop() {
                    Ok(IxExecStepBatch::Parallel(batch))
                } else {
                    self.step = Steps::Complete;
                    self.advance()
                }
            }
            _ => Ok(IxExecStepBatch::End),
        }
    }
    fn payer(&self) -> Arc<Keypair> {
        self.resource.payer()
    }
}
