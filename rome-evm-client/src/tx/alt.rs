use {
    super::builder::TxBuilder,
    crate::{
        error::{ProgramResult, RomeEvmError, RomeEvmError::AddressLookupTableNotFound},
        Resource,
    },
    async_trait::async_trait,
    emulator::get_alt,
    rome_solana::batch::{AdvanceTx, IxExecStepBatch, OwnedAtomicIxBatch, TxVersion},
    rome_utils::iter::into_chunks,
    solana_sdk::{
        account::Account,
        address_lookup_table::{state::AddressLookupTable, AddressLookupTableAccount},
        pubkey::Pubkey,
        signature::Keypair,
    },
    std::sync::Arc,
};

#[derive(Clone)]
enum Steps {
    Execute,
    WaitNextSlot,
    End,
}
/// address lookup table transaction
pub struct AltTx {
    pub tx_builder: TxBuilder,
    pub resource: Arc<Resource>,
    session: u64,
    pub keys: Option<Vec<Pubkey>>,
    pub recent_slot: u64,
    step: Steps,
    alt_raw_account: Option<(Pubkey, Account)>,
}

impl AltTx {
    pub fn new(
        tx_builder: TxBuilder,
        resource: Arc<Resource>,
        keys: Vec<Pubkey>,
    ) -> ProgramResult<Self> {
        let recent_slot = tx_builder.client_cloned().get_slot()? - 5;

        Ok(Self {
            tx_builder,
            resource,
            session: rand::random(),
            keys: Some(keys),
            recent_slot,
            step: Steps::Execute,
            alt_raw_account: None,
        })
    }
    pub fn tx_data_alloc(&mut self) -> Vec<Vec<u8>> {
        let keys = self.keys.take().expect("accounts expected");

        let keys_bin = into_chunks(keys, 25)
            .into_iter()
            .map(|chunk| {
                chunk
                    .iter()
                    .map(|key| key.to_bytes())
                    .collect::<Vec<_>>()
                    .into_flattened()
            })
            .collect::<Vec<_>>();

        let mut data = vec![emulator::Instruction::AltAlloc as u8];
        data.extend(self.resource.holder());
        data.extend(self.tx_builder.chain_id.to_le_bytes());
        data.extend(self.session.to_le_bytes());
        data.extend(self.recent_slot.to_le_bytes());

        keys_bin
            .into_iter()
            .map(|accs| vec![data.clone(), accs].concat())
            .collect::<Vec<_>>()
    }
    pub fn tx_data_dealloc(&self) -> Vec<u8> {
        let mut data = vec![emulator::Instruction::AltDealloc as u8];
        data.extend(self.resource.holder());
        data.extend(self.tx_builder.chain_id.to_le_bytes());
        data.extend(self.session.to_le_bytes());

        data
    }

    pub fn ixs_alloc(&mut self) -> ProgramResult<Vec<OwnedAtomicIxBatch>> {
        let data_vec = self.tx_data_alloc();
        let emulation = self
            .tx_builder
            .emulate(data_vec.last().unwrap(), &self.resource.payer_key())?;

        let ixs = data_vec
            .into_iter()
            .map(|x| {
                let ix = self.tx_builder.build_ix(&emulation, x);
                OwnedAtomicIxBatch::new_owned(vec![ix])
            })
            .collect();

        Ok(ixs)
    }

    pub fn ix_dealloc(&self) -> ProgramResult<OwnedAtomicIxBatch> {
        let data = self.tx_data_dealloc();
        let emulation = self.tx_builder.emulate(&data, &self.resource.payer_key())?;

        let ix = self.tx_builder.build_ix(&emulation, data);

        Ok(OwnedAtomicIxBatch::new_composible_owned(ix))
    }

    fn alt_raw_account(&self) -> ProgramResult<(Pubkey, Account)> {
        let mut data = vec![];
        data.extend(self.resource.holder());
        data.extend(self.tx_builder.chain_id.to_le_bytes());

        let key = get_alt(
            self.tx_builder.program_id(),
            &data,
            &self.resource.payer_key(),
            self.tx_builder.client_cloned(),
        )?
        .ok_or(AddressLookupTableNotFound)?;

        let acc = self.tx_builder.client_cloned().get_account(&key)?;

        Ok((key, acc))
    }
    pub fn alt_account_(&self) -> ProgramResult<AddressLookupTableAccount> {
        let (key, acc) = self.alt_raw_account.as_ref().expect("alt_account expected");
        Self::alt_account(key, acc)
    }
    pub fn alt_account(key: &Pubkey, acc: &Account) -> ProgramResult<AddressLookupTableAccount> {
        let state = AddressLookupTable::deserialize(&acc.data)?;

        let alt = AddressLookupTableAccount {
            key: *key,
            addresses: state.addresses.to_vec(),
        };

        Ok(alt)
    }

    pub fn last_extended_slot(&self) -> ProgramResult<u64> {
        let (_, acc) = self.alt_raw_account.as_ref().expect("alt_account expected");
        let state = AddressLookupTable::deserialize(&acc.data)?;

        Ok(state.meta.last_extended_slot)
    }
}

#[async_trait]
impl AdvanceTx<'_> for AltTx {
    type Error = RomeEvmError;

    fn advance(&mut self) -> ProgramResult<IxExecStepBatch<'static>> {
        match &mut self.step {
            Steps::Execute => {
                let mut ixs = vec![self.ix_dealloc()?];
                let mut alloc = self.ixs_alloc()?;
                ixs.append(&mut alloc);

                self.step = Steps::WaitNextSlot;

                Ok(IxExecStepBatch::Parallel(ixs, TxVersion::Legacy))
            }
            Steps::WaitNextSlot => {
                self.step = Steps::End;

                self.alt_raw_account = Some(self.alt_raw_account()?);
                let slot = self.last_extended_slot()?;

                Ok(IxExecStepBatch::WaitNextSlot(slot))
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
        self.resource.payer()
    }
}
