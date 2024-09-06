use super::Poller;

use std::fmt::Debug;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::Mutex;
use url::Url;

/// Atomic Poller uses [Poller] to poll the address and send the data to the channel
/// while keeping a track of updated payload and interval
#[derive(Clone)]
pub struct AtomicPoller<T: serde::de::DeserializeOwned + Debug> {
    /// Channel to send the polled data
    pub tx: Arc<UnboundedSender<T>>,
    /// Address to poll
    pub addr: Arc<Url>,
    /// Payload to send
    pub payload: Arc<Mutex<serde_json::Value>>,
    /// Polling interval
    pub interval_ms: Arc<AtomicU64>,
}

impl<T: serde::de::DeserializeOwned + Debug> AtomicPoller<T> {
    /// Create a new atomic poller
    pub fn new(
        tx: UnboundedSender<T>,
        addr: Url,
        payload: serde_json::Value,
        interval_ms: u64,
    ) -> Self {
        Self {
            tx: Arc::new(tx),
            addr: Arc::new(addr),
            payload: Arc::new(Mutex::new(payload)),
            interval_ms: Arc::new(AtomicU64::new(interval_ms)),
        }
    }

    /// Update the poller payload
    pub async fn update_payload(&self, payload: serde_json::Value) {
        *self.payload.lock().await = payload;
    }

    /// Poll the address and send the data to the channel
    pub async fn poll(&self) -> anyhow::Result<()> {
        tracing::info!("Polling: {}", &self.addr);

        let client = reqwest::Client::new();
        let addr = self.addr.as_str();

        loop {
            let mut interval = tokio::time::interval(Duration::from_millis(
                self.interval_ms.load(std::sync::atomic::Ordering::Relaxed),
            ));

            let payload = self.payload.lock().await.clone();

            let res = Poller::poll_step(&client, addr, &payload).await?;

            self.tx.send(res).expect("Poller channel broken");

            interval.tick().await;
        }
    }
}
