use std::fmt::Debug;

use anyhow::bail;
use futures_util::Future;

use tokio::time::Duration;

/// A helper for running an indefinite service with a retry mechanism
#[derive(clap::Args, Debug, Clone, Copy)]
pub struct ServiceRunner {
    /// Number of retries before giving up on an unhealthy service
    #[arg(long, default_value_t = 5)]
    pub unhealthy_retries: u32,
    /// If the service is not healthy within this time, it will be retried
    /// until it is healthy up to `retries` times
    #[arg(long, default_value_t = 5000)]
    pub healthy_within_ms: u64,
    /// Upper limit of retries before giving up
    #[arg(long, default_value_t = u32::MAX)]
    pub max_retries: u32,
    /// Retry interval in milliseconds
    #[arg(long, default_value_t = 100)]
    pub retry_interval_ms: u64,
}

impl Default for ServiceRunner {
    fn default() -> Self {
        Self {
            unhealthy_retries: 5,
            healthy_within_ms: 5000,
            max_retries: u32::MAX,
            retry_interval_ms: 100,
        }
    }
}

impl ServiceRunner {
    /// Run the given future factory up to `max_retries` with a retry mechanism
    /// classifying the service as unhealthy if it exits too quickly
    pub async fn run<F, Fut>(&self, mut future_factory: F) -> anyhow::Result<()>
    where
        F: FnMut() -> Fut + Send + 'static,
        Fut: Future + Send + 'static,
        Fut::Output: Send + Debug + 'static,
    {
        let max_unhealthy_retries = self.unhealthy_retries;
        let max_retries = self.max_retries;
        let healthy_within = Duration::from_millis(self.healthy_within_ms);
        let retry_interval = Duration::from_millis(self.retry_interval_ms);

        let mut retries = 0;
        let mut unhealthy_retries = 0;

        loop {
            let start_time = tokio::time::Instant::now();

            tracing::debug!("Starting service");

            let result = future_factory().await;
            tracing::warn!("Service exited with: {:?}", result);

            if start_time.elapsed() < healthy_within {
                tracing::error!("Service exited too quickly, retrying");
                unhealthy_retries += 1;
            } else {
                tracing::error!("Service exited unexpectedly, retrying");
                unhealthy_retries = 0;
            }

            // Increment total number of retries ever
            retries += 1;

            // check if we should give up
            if unhealthy_retries >= max_unhealthy_retries || retries >= max_retries {
                bail!("Service failed too many times, giving up");
            }

            // Wait a moment before retrying
            if retry_interval.as_millis() > 0 {
                tokio::time::sleep(retry_interval).await;
            }
        }
    }
}
