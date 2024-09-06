use {
    crate::error::RomeEvmError::NoAvailableHolders,
    solana_sdk::signer::keypair::Keypair,
    std::{
        collections::BTreeSet,
        sync::{Arc, Mutex},
    },
};

pub struct Holder {
    pub index: u64,
    pub holders: Arc<Mutex<BTreeSet<u64>>>,
}

impl Drop for Holder {
    fn drop(&mut self) {
        match self.holders.lock() {
            Ok(mut lock) => {
                if !lock.insert(self.index) {
                    tracing::warn!("collision of holder index usage")
                }
            }
            Err(e) => {
                tracing::warn!("holders mutex error: {}", e)
            }
        }
    }
}

#[derive(Clone)]
pub struct TransactionSigner {
    /// payer
    keypair: Arc<Keypair>,
    /// Holder account indices
    holders: Arc<Mutex<BTreeSet<u64>>>,
}

impl TransactionSigner {
    pub fn new(keypair: Arc<Keypair>, number_holders: u64) -> Self {
        let holders = BTreeSet::from_iter((0_u64..number_holders).into_iter());

        Self {
            keypair,
            holders: Arc::new(Mutex::new(holders)),
        }
    }

    pub fn lock_holder(&self) -> crate::error::Result<Holder> {
        let mut lock = self.holders.lock()?;
        let index = lock.pop_first().ok_or(NoAvailableHolders)?;
        Ok(Holder {
            index,
            holders: Arc::clone(&self.holders),
        })
    }

    pub fn keypair(&self) -> &Arc<Keypair> {
        &self.keypair
    }
}
