mod atomic;
mod constant;

pub use atomic::*;
pub use constant::*;

use std::fmt::Debug;
use std::sync::Arc;

use anyhow::{bail, Context};

use super::ServiceRunner;

/// Different strategies for polling
pub enum PollerStrategy<T: serde::de::DeserializeOwned + Debug> {
    /// Poller strategy that uses atomic polling
    Atomic(AtomicPoller<T>),
    /// Poller strategy that uses constant polling
    Constant(ConstantPoller<T>),
}

/// Poller is a generic implementation that polls a given address with a given payload
pub struct Poller<T: serde::de::DeserializeOwned + Debug> {
    strategy: PollerStrategy<T>,
}

impl<T: serde::de::DeserializeOwned + Debug> Poller<T> {
    /// Create a new poller with a given poller strategy
    pub fn new(strategy: PollerStrategy<T>) -> Self {
        Self { strategy }
    }

    /// Create a new poller with an atomic poller strategy
    pub fn new_atomic(atomic_poller: AtomicPoller<T>) -> Self {
        Self {
            strategy: PollerStrategy::Atomic(atomic_poller),
        }
    }

    /// Create a new poller with a constant poller strategy
    pub fn new_constant(constant_poller: ConstantPoller<T>) -> Self {
        Self {
            strategy: PollerStrategy::Constant(constant_poller),
        }
    }

    /// Sends a request to the address with the payload and returns the response
    pub async fn poll_step(
        client: &reqwest::Client,
        addr: &str,
        payload: &serde_json::Value,
    ) -> anyhow::Result<T> {
        let res = client
            .post(addr)
            .json(payload)
            .send()
            .await
            .context("Failed to send request")?;

        if !res.status().is_success() {
            bail!("Failed to send request: {:?}", res.error_for_status());
        }

        let payload = res.bytes().await.context("Failed to get response text")?;

        serde_json::from_slice(&payload)
            .map_err(|e| anyhow::anyhow!("Failed to parse response: {:?}", e))
    }

    /// Use [PollerStrategy] to poll
    /// upto 5 times before exiting
    pub async fn listen(&self) -> anyhow::Result<()> {
        match &self.strategy {
            PollerStrategy::Atomic(atomic_poller) => atomic_poller.poll().await,
            PollerStrategy::Constant(constant_poller) => constant_poller.poll().await,
        }
    }
}

impl<T: serde::de::DeserializeOwned + Debug + Send + 'static> Poller<T> {
    /// Use [PollerStrategy] to poll
    /// with a service runner
    pub async fn listen_with_service_runner(
        self: Arc<Self>,
        service_runner: ServiceRunner,
    ) -> anyhow::Result<()> {
        service_runner
            .run(move || {
                let this = self.clone();

                async move { this.listen().await }
            })
            .await
    }
}
