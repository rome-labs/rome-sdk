use std::sync::Arc;

use anyhow::{bail, Context};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_config::RpcBlockConfig;
use solana_sdk::clock::Slot;
use solana_transaction_status::{TransactionDetails, UiTransactionEncoding};

use crate::config::SolanaConfig;
use crate::types::{BlockSender, SlotReceiver, SlotSender};
use rome_utils::services::ServiceRunner;

/// Polls the Jito Mempool for transactions that mention Solana programs
#[derive(clap::Args, Debug)]
pub struct SolanaBlockIndexerParams {
    /// solana rpc config
    #[clap(flatten)]
    pub solana_config: SolanaConfig,

    /// service runner
    #[clap(flatten)]
    pub service_runner: ServiceRunner,

    /// number of concurrent requests to `getBlock`
    #[clap(long, default_value_t = 10)]
    pub get_block_concurrency: usize,
}

/// Indexes the blocks from the solana node
#[derive(Clone)]
pub struct SolanaBlockIndexer {
    client: Arc<RpcClient>,
    service_runner: Arc<ServiceRunner>,
    get_block_concurrency: usize,
}

impl From<SolanaBlockIndexerParams> for SolanaBlockIndexer {
    fn from(params: SolanaBlockIndexerParams) -> Self {
        Self::new(params)
    }
}

impl SolanaBlockIndexer {
    /// Create a new [SolanaBlockIndexer] instance
    pub fn new(params: SolanaBlockIndexerParams) -> Self {
        Self {
            client: Arc::new(params.solana_config.into()),
            service_runner: Arc::new(params.service_runner),
            get_block_concurrency: params.get_block_concurrency,
        }
    }

    /// consumes slot channel and sends blocks to block channel
    pub async fn index_blocks(
        self,
        mut slot_rx: SlotReceiver,
        block_tx: BlockSender,
    ) -> anyhow::Result<()> {
        let semaphore = Arc::new(tokio::sync::Semaphore::new(self.get_block_concurrency));

        while let Some(slot) = slot_rx.recv().await {
            let permit = semaphore
                .clone()
                .acquire_owned()
                .await
                .expect("Semaphore closed");

            let block_tx = block_tx.clone();
            let client = self.client.clone();
            let service_runner = self.service_runner.clone();

            tokio::spawn(async move {
                service_runner
                    .run(move || {
                        let block_tx = block_tx.clone();
                        let client = client.clone();

                        async move {
                            let block = client
                                .get_block_with_config(
                                    slot,
                                    RpcBlockConfig {
                                        encoding: Some(UiTransactionEncoding::Base58),
                                        transaction_details: Some(TransactionDetails::Full),
                                        rewards: None,
                                        commitment: Some(client.commitment()),
                                        max_supported_transaction_version: None,
                                    },
                                )
                                .await
                                .with_context(|| format!("Failed to get block on slot {}", slot))?;

                            block_tx.send(block).expect("Block channel closed");

                            Ok::<(), anyhow::Error>(())
                        }
                    })
                    .await?;

                drop(permit);

                Ok::<(), anyhow::Error>(())
            });
        }

        panic!("Slot channel closed")
    }

    /// listen to slots with blocks
    pub async fn index_slots(self, tx: SlotSender, start_slot: Option<Slot>) -> anyhow::Result<()> {
        let mut slot = match start_slot {
            Some(value) => value,
            None => self.client.get_slot().await?,
        };

        loop {
            let blocks = self.client.get_blocks(slot, None).await?;

            if let Some(last) = blocks.last() {
                slot = last + 1;
            }

            for block_slot in blocks {
                tx.send(block_slot).expect("Slot channel closed");
            }
        }
    }

    /// Index the blocks from the solana node and
    /// send them to the `block_tx` channel
    pub async fn index(
        self,
        block_tx: BlockSender,
        start_slot: Option<Slot>,
    ) -> anyhow::Result<()> {
        let (slot_tx, slot_rx) = tokio::sync::mpsc::unbounded_channel();

        let slot_service_jh = {
            let this = self.clone();

            self.service_runner
                .run(move || this.clone().index_slots(slot_tx.clone(), start_slot))
        };

        let block_service_jh = tokio::spawn(self.clone().index_blocks(slot_rx, block_tx));

        tokio::select! {
            res = slot_service_jh => {
                bail!("Failed to index slots {res:?}");
            }
            res = block_service_jh => {
                bail!("Failed to index blocks {res:?}");
            }
        }
    }
}
