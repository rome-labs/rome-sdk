use {
    super::{AtomicTx, AltTx,},
    crate::{error::{ProgramResult, RomeEvmError}, },
    async_trait::async_trait,
    emulator::Emulation,
    rome_solana::batch::{AdvanceTx, IxExecStepBatch, OwnedAtomicIxBatch, TxVersion},
    solana_sdk::{
        signature::Keypair, pubkey::Pubkey, instruction::Instruction, 
        address_lookup_table::AddressLookupTableAccount
    },
    std::sync::Arc,
};

enum Steps {
    Execute,
    End,
}
/// A composite transaction consisting of atomic_tx and SVM-instructions
pub struct AtomicSvm {
    atomic_tx: AtomicTx,
    pub alts: Option<Vec<AddressLookupTableAccount>>,
    step: Steps,
}

impl AtomicSvm {
    pub fn new(
        mut atomic_tx: AtomicTx, 
        svm: Vec<Instruction>, 
        alt_keys: Option<Vec<Pubkey>>
    ) -> ProgramResult<Self> {
    
        atomic_tx.ix()?;
        let ix = atomic_tx.ix.as_mut().unwrap();
        ix.push(svm);

        let alts =  if let Some(keys) = alt_keys {
            let alts = keys
                .iter()
                .map(|key| {
                    let acc = atomic_tx.tx_builder.client_cloned().get_account(&key)?;
                    AltTx::alt_account(&key, &acc)
                })
                .collect::<ProgramResult<Vec<_>>>()?;
            Some(alts)
        } else {
            None
        };

        Ok(Self {
            atomic_tx,
            alts,
            step: Steps::Execute,
        })
    }
}

impl AtomicSvm {
    pub fn ix(&self) -> &OwnedAtomicIxBatch {
        self.atomic_tx.ix.as_ref().unwrap()
    }
    pub fn emulation (&self) -> &Emulation {
        self.atomic_tx.emulation.as_ref().unwrap()
    }
}

#[async_trait]
impl AdvanceTx<'_> for AtomicSvm {
    type Error = RomeEvmError;

    fn advance(&mut self) -> ProgramResult<IxExecStepBatch<'static>> {
        match &mut self.step {
            Steps::Execute => {
                self.step = Steps::End;

                if let Some(alt) = self.alts.take() {
                    self.atomic_tx.advance_with_version(TxVersion::V0(alt))
                } else {
                    self.atomic_tx.advance()
                }
            }
            _ => Ok(IxExecStepBatch::End),
        }
    }

    fn advance_with_version(
        &mut self,
        _: TxVersion,
    ) -> Result<IxExecStepBatch<'static>, Self::Error> {
        unreachable!()
    }
    
    fn payer(&self) -> Arc<Keypair> {
        self.atomic_tx.resource.payer()
    }
}
