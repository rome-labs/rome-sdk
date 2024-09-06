use super::Poller;
use std::fmt::Debug;

use tokio::sync::mpsc::UnboundedSender;
use url::Url;

/// Constant Poller uses [Poller] to poll the address and send the data to the channel
/// with no mutation of payload and interval
#[derive(Clone)]
pub struct ConstantPoller<T: serde::de::DeserializeOwned + Debug> {
    /// Channel to send the polled data
    pub tx: UnboundedSender<T>,
    /// Address to poll
    pub addr: Url,
    /// Payload to send
    pub payload: serde_json::Value,
    /// Polling interval
    pub interval: std::time::Duration,
}

impl<T: serde::de::DeserializeOwned + Debug> ConstantPoller<T> {
    /// Poll the address and send the data to the channel
    pub async fn poll(&self) -> anyhow::Result<()> {
        tracing::info!("Polling: {}", &self.addr);

        let client = reqwest::Client::new();
        let addr = self.addr.as_str();

        let mut interval = tokio::time::interval(self.interval);

        loop {
            let res = Poller::poll_step(&client, addr, &self.payload).await?;
            self.tx.send(res).expect("Poller channel broken");

            interval.tick().await;
        }
    }
}
