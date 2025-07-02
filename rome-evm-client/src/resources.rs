use crate::error::ProgramResult;
use {
    ethers::types::Address,
    rome_solana::payer::SolanaKeyPayer,
    solana_sdk::{
        pubkey::Pubkey,
        signer::{keypair::Keypair, Signer},
    },
    std::{
        mem::size_of,
        path::PathBuf,
        sync::{Arc, Mutex},
    },
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ResourceType {
    FeeRecipients(Vec<Address>),
    Holders(u64),
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PayerConfig {
    payer_keypair: PathBuf,
    fee_recipients: Option<Vec<Address>>,
    number_holders: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct Payer {
    payer_keypair: Arc<Keypair>,
    resource_type: ResourceType,
}

impl PayerConfig {
    pub fn fee_recipients(&self) -> &Option<Vec<Address>> {
        &self.fee_recipients
    }
}

impl Payer {
    pub async fn from_config(cfg: &PayerConfig) -> anyhow::Result<Payer> {
        let solana_payer = SolanaKeyPayer::read_from_file(&cfg.payer_keypair).await?;
        if cfg.number_holders.is_none() && cfg.fee_recipients.is_none()
            || cfg.number_holders.is_some() && cfg.fee_recipients.is_some()
        {
            return Err(anyhow::anyhow!(
                "Failed to parse payers from config: fee_recipients or holders expected"
            ));
        }

        let resource_type = if let Some(accs) = cfg.fee_recipients.as_ref() {
            ResourceType::FeeRecipients(accs.clone())
        } else {
            ResourceType::Holders(cfg.number_holders.unwrap())
        };

        Ok(Self {
            payer_keypair: Arc::new(solana_payer.payer),
            resource_type,
        })
    }

    pub async fn from_config_list(list: &[PayerConfig]) -> anyhow::Result<Vec<Payer>> {
        let mut vec = vec![];

        for cfg in list.iter() {
            vec.push(Self::from_config(cfg).await?)
        }

        Ok(vec)
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
struct ResourceItem {
    payer_keypair: Arc<Keypair>,
    holder: u64,
    fee_recipient: Option<Address>,
}

impl ResourceItem {
    pub fn from_payer(payer: Payer) -> Vec<Self> {
        match payer.resource_type {
            ResourceType::FeeRecipients(fee) => fee
                .into_iter()
                .enumerate()
                .map(|(ix, recipient)| ResourceItem {
                    payer_keypair: payer.payer_keypair.clone(),
                    holder: ix as u64,
                    fee_recipient: Some(recipient),
                })
                .collect::<Vec<_>>(),
            ResourceType::Holders(number) => (0_u64..number)
                .map(|holder| Self {
                    payer_keypair: payer.payer_keypair.clone(),
                    holder,
                    fee_recipient: None,
                })
                .collect::<Vec<_>>(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ResourceFactory(Arc<Mutex<Vec<ResourceItem>>>);

impl ResourceFactory {
    pub fn from_payers(payers: Vec<Payer>) -> Self {
        let items = payers
            .into_iter()
            .map(ResourceItem::from_payer)
            .collect::<Vec<_>>()
            .into_iter()
            .flatten()
            .rev()
            .collect::<Vec<_>>();

        Self(Arc::new(Mutex::new(items)))
    }

    pub async fn get(&self) -> ProgramResult<Resource> {
        loop {
            {
                let mut lock = self.0.lock()?;
                if let Some(item) = lock.pop() {
                    return Ok(Resource {
                        item: item,
                        factory: self.0.clone(),
                    });
                }
            }

            tracing::debug!("no available resourced, yielding");
            tokio::task::yield_now().await;
        }
    }
}

pub struct Resource {
    item: ResourceItem,
    factory: Arc<Mutex<Vec<ResourceItem>>>,
}

impl Resource {
    pub fn holder(&self) -> [u8; size_of::<u64>()] {
        self.item.holder.to_le_bytes()
    }
    pub fn holder_index(&self) -> u64 {
        self.item.holder
    }
    pub fn payer(&self) -> Arc<Keypair> {
        self.item.payer_keypair.clone()
    }
    pub fn payer_key(&self) -> Pubkey {
        self.item.payer_keypair.pubkey()
    }
    pub fn fee_recipient(&self) -> Vec<u8> {
        if self.item.fee_recipient.is_none() {
            return vec![0];
        }

        let mut data = vec![1];
        data.extend_from_slice(self.item.fee_recipient.unwrap().as_bytes());
        data
    }
    pub fn fee_recipient_address(&self) -> Option<Address> {
        self.item.fee_recipient
    }
}

impl Drop for Resource {
    fn drop(&mut self) {
        match self.factory.lock() {
            Ok(mut lock) => lock.push(self.item.clone()),
            Err(e) => tracing::error!("Resources get mutex error: {}", e),
        }
    }
}
