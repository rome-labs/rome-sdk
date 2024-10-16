use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Duration;

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::clock::DEFAULT_MS_PER_SLOT;
use solana_sdk::hash::Hash;
use tokio::sync::RwLock;

use crate::types::AsyncAtomicRpcClient;

/// Clock indexer for the Solana cluster
#[derive(Clone)]
pub struct SolanaClockIndexer {
    /// RPC client to interact with the Solana cluster
    client: AsyncAtomicRpcClient,
    /// Current clock
    clock: SolanaClock,
}

/// Clock info of the Solana cluster
#[derive(Clone, Debug)]
pub struct SolanaClock {
    /// Current slot
    pub slot: Arc<AtomicU64>,
    /// Current blockhash
    pub blockhash: Arc<RwLock<Hash>>,
}

impl SolanaClockIndexer {
    /// Create a new [SolanaClockIndexer] instance
    pub async fn new(client: AsyncAtomicRpcClient) -> anyhow::Result<Self> {
        let clock = SolanaClock::new(&client).await?;

        Ok(Self { client, clock })
    }

    /// Get the current clock
    pub fn get_current_clock(&self) -> SolanaClock {
        self.clock.clone()
    }

    /// Start the clock indexer
    pub async fn start(self) -> anyhow::Result<()> {
        let clock_interval = Duration::from_millis(DEFAULT_MS_PER_SLOT);
        let mut clock_interval = tokio::time::interval(clock_interval);

        loop {
            clock_interval.tick().await;

            let old_slot = self.clock.get_current_slot();

            self.clock.sync(&self.client).await?;

            let new_slot = self.clock.get_current_slot();

            if new_slot == old_slot {
                clock_interval.tick().await;
            }
        }
    }
}

impl SolanaClock {
    /// New instance of [SolanaClock]
    pub async fn new(rpc_client: &RpcClient) -> anyhow::Result<Self> {
        let (slot, blockhash) = Self::fetch_network_clock_info(rpc_client).await?;

        Ok(Self {
            slot: Arc::new(AtomicU64::new(slot)),
            blockhash: Arc::new(RwLock::new(blockhash)),
        })
    }

    /// Get the current slot
    pub fn get_current_slot(&self) -> u64 {
        self.slot.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Get the current blockhash
    pub async fn get_current_blockhash(&self) -> Hash {
        *self.blockhash.read().await
    }

    /// Get clock information from [RpcClient]
    pub async fn fetch_network_clock_info(client: &RpcClient) -> anyhow::Result<(u64, Hash)> {
        let blockhash = client.get_latest_blockhash();
        let slot = client.get_slot();

        let (slot_res, blockhash_res) = tokio::join!(slot, blockhash);

        Ok((slot_res?, blockhash_res?))
    }

    /// Sync the clock
    pub async fn sync(&self, client: &RpcClient) -> anyhow::Result<()> {
        let (slot, blockhash) = Self::fetch_network_clock_info(client).await?;

        self.slot.store(slot, std::sync::atomic::Ordering::Relaxed);
        *self.blockhash.write().await = blockhash;

        Ok(())
    }
}
