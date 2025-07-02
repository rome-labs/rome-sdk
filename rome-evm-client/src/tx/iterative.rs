use {
    super::{builder::TxBuilder, AltTx},
    crate::{
        error::{ProgramResult, RomeEvmError},
        Resource,
    },
    async_trait::async_trait,
    emulator::Emulation,
    ethers::{prelude::TxHash, types::Bytes, utils::keccak256},
    rome_solana::batch::{AdvanceTx, IxExecStepBatch, OwnedAtomicIxBatch, TxVersion},
    solana_sdk::signature::Keypair,
    std::sync::Arc,
};

pub const MULTIPLE_ITERATIONS: f64 = 1.2; // estimated number of iterations is multiplied by this value

enum Steps {
    Execute,
    Confirm,
    Complete,
}

pub struct IterativeTx {
    tx_builder: TxBuilder,
    rlp: Bytes,
    pub resource: Arc<Resource>,
    pub ixs: Option<Vec<OwnedAtomicIxBatch>>,
    step: Steps,
    hash: TxHash,
    session: u64,
    pub alt_tx: Option<AltTx>,
}

impl IterativeTx {
    pub fn new(tx_builder: TxBuilder, resource: Arc<Resource>, rlp: Bytes) -> ProgramResult<Self> {
        let hash: TxHash = keccak256(rlp.as_ref()).into();

        Ok(Self {
            tx_builder,
            rlp,
            resource,
            ixs: None,
            step: Steps::Execute,
            hash,
            session: rand::random(),
            alt_tx: None,
        })
    }

    fn emulation_data(&self) -> Vec<u8> {
        let mut data = vec![emulator::Instruction::DoTxIterative as u8];
        data.extend(self.session.to_le_bytes());
        data.extend(self.resource.holder());
        data.append(&mut self.resource.fee_recipient());
        data.extend_from_slice(self.rlp.as_ref());

        data
    }

    fn tx_data(&self, emulation: &Emulation, unique: u64) -> ProgramResult<Vec<u8>> {
        let overrides_len = emulation.lock_overrides.len() as u64;

        let mut data = vec![emulator::Instruction::DoTxIterative as u8];
        data.extend(unique.to_le_bytes());
        data.extend(self.session.to_le_bytes());
        data.extend(self.resource.holder());
        data.append(&mut self.resource.fee_recipient());
        data.extend(overrides_len.to_le_bytes());
        data.append(&mut emulation.lock_overrides.clone());
        data.extend_from_slice(self.rlp.as_ref());

        Ok(data)
    }

    pub fn ixs(&mut self) -> ProgramResult<()> {
        let data = self.emulation_data();
        let emulation = self.tx_builder.emulate(&data, &self.resource.payer_key())?;

        let vm = emulation.vm.as_ref().expect("vm expected");
        let count = (vm.iteration_count as f64 * MULTIPLE_ITERATIONS) as u64;

        let ixs = (0..count)
            .map(|unique| self.tx_data(&emulation, unique))
            .collect::<ProgramResult<Vec<_>>>()?
            .into_iter()
            .map(|data| self.tx_builder.build_ix(&emulation, data))
            .collect();

        self.ixs = Some(OwnedAtomicIxBatch::new_composible_batches_owned(ixs));

        Ok(())
    }
}

#[macro_export]
macro_rules! advance_with_version_it {
    {} => {
        fn advance_with_version(&mut self, version: TxVersion) -> Result<IxExecStepBatch<'static>, Self::Error> {
            let ix = self.advance();
            match ix {
                Ok(IxExecStepBatch::ParallelUnchecked(ix_, _)) =>
                    Ok(IxExecStepBatch::ParallelUnchecked(ix_, version)),
                _ => ix,
            }
        }
    }
}
pub(crate) use advance_with_version_it;

#[async_trait]
impl AdvanceTx<'_> for IterativeTx {
    type Error = RomeEvmError;
    fn advance(&mut self) -> ProgramResult<IxExecStepBatch<'static>> {
        match &mut self.step {
            Steps::Execute => {
                if self.ixs.is_none() {
                    self.ixs()?;
                }

                self.step = Steps::Confirm;
                let ixs = self.ixs.take().unwrap();

                Ok(IxExecStepBatch::ParallelUnchecked(ixs, TxVersion::Legacy))
            }
            Steps::Confirm => {
                self.step = Steps::Complete;

                let confirm = self.tx_builder.confirm_tx_iterative(
                    self.resource.holder_index(),
                    self.hash,
                    &self.resource.payer_key(),
                    self.session,
                )?;

                Ok(IxExecStepBatch::ConfirmationIterativeTx(confirm))
            }
            _ => Ok(IxExecStepBatch::End),
        }
    }
    advance_with_version_it!();
    fn payer(&self) -> Arc<Keypair> {
        self.resource.payer()
    }
}
