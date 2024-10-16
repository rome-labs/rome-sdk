use super::utils::TRANSMIT_TX_SIZE;
use rome_solana::batch::{AdvanceTx, IxExecStepBatch, OwnedAtomicIxBatch};
use rome_utils::holder::Holder;
use solana_program::{entrypoint::MAX_PERMITTED_DATA_INCREASE, pubkey::Pubkey};

use super::builder::TxBuilder;
use crate::error::{ProgramResult, RomeEvmError};
use async_trait::async_trait;
use ethers::types::{Bytes, TxHash};
use rome_utils::iter::into_chunks;

pub struct TransmitTx {
    pub tx_builder: TxBuilder,
    pub rlp: Bytes,
    pub hash: TxHash,
    pub holder: Holder,
    step: Steps,
}

#[derive(Clone)]
enum Steps {
    Init,
    Execute(Vec<Vec<OwnedAtomicIxBatch>>),
    Complete,
}

impl TransmitTx {
    pub fn new(tx_builder: TxBuilder, holder: Holder, rlp: Bytes, hash: TxHash) -> Self {
        Self {
            tx_builder,
            rlp,
            hash,
            holder,
            step: Steps::Init,
        }
    }

    pub fn tx_data(
        offset: u64,
        bin: Vec<u8>,
        holder: &Holder,
        hash: TxHash,
        chain: u64,
    ) -> Vec<u8> {
        let mut data = vec![emulator::Instruction::TransmitTx as u8];
        data.extend(holder.get_index().to_le_bytes());
        data.extend(offset.to_le_bytes());
        data.extend(hash.as_bytes());
        data.extend(chain.to_le_bytes());
        data.extend(bin);

        data
    }

    fn ixs(&self, payer: &Pubkey) -> ProgramResult<Vec<OwnedAtomicIxBatch>> {
        let data = Self::tx_data(
            0,
            self.rlp.to_vec(),
            &self.holder,
            self.hash,
            self.tx_builder.chain_id,
        );
        let emulation = self.tx_builder.emulate(&data, payer)?;

        let mut offset = 0;

        let ixs = into_chunks(self.rlp.to_vec(), TRANSMIT_TX_SIZE)
            .into_iter()
            .map(|chunk| {
                let new_offset = offset + chunk.len() as u64;
                let data = Self::tx_data(
                    offset,
                    chunk,
                    &self.holder,
                    self.hash,
                    self.tx_builder.chain_id,
                );
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
    fn advance(&mut self, payer: &Pubkey) -> ProgramResult<IxExecStepBatch<'static>> {
        match &mut self.step {
            Steps::Init => {
                let ixs = self.ixs(payer)?;
                let limit = MAX_PERMITTED_DATA_INCREASE / TRANSMIT_TX_SIZE;
                let mut batches = into_chunks(ixs, limit);
                batches.reverse();

                self.step = Steps::Execute(batches);
                self.advance(payer)
            }
            Steps::Execute(batches) => {
                if let Some(batch) = batches.pop() {
                    Ok(IxExecStepBatch::Parallel(batch))
                } else {
                    self.step = Steps::Complete;
                    self.advance(payer)
                }
            }
            _ => Ok(IxExecStepBatch::End),
        }
    }
}
