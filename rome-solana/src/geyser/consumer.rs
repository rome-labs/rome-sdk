use std::time::Duration;

use kafka::consumer::Consumer;
use serde::de::DeserializeOwned;
use tokio::sync::mpsc::Sender;
use tokio::time::interval;

use super::config::GeyserKafkaConfig;
use super::records::GeyserKafkaRecord;

/// Geyser Kafka Consumer interface
#[derive(Debug)]
pub struct GeyserKafkaConsumer {
    /// Kafka client
    consumer: Consumer,
    /// Poll interval
    poll_interval: Duration,
}

impl GeyserKafkaConsumer {
    /// Create a new instance of [GeyserClient]
    pub fn new(config: GeyserKafkaConfig) -> anyhow::Result<Self> {
        let consumer = Consumer::from_hosts(config.kafka_hosts)
            .create()
            .map_err(|e| anyhow::anyhow!("Failed to create consumer: {:?}", e))?;

        let poll_interval = Duration::from_millis(config.kafka_poll_interval_ms);

        Ok(Self {
            consumer,
            poll_interval,
        })
    }

    /// Subscribe to a topic
    pub fn subscribe<T: GeyserKafkaRecord>(&self, _ix: Sender<T>) {}

    /// Bytes to [GeyserKafkaRecord]
    pub fn bytes_to_record<T: GeyserKafkaRecord + DeserializeOwned>(
        bytes: &[u8],
    ) -> anyhow::Result<T> {
        bincode::deserialize(bytes).map_err(|e| anyhow::anyhow!("Failed to deserialize: {:?}", e))
    }

    /// Listen
    pub async fn listen(mut self) -> anyhow::Result<()> {
        let mut interval = interval(self.poll_interval);

        loop {
            let message = self.consumer.poll()?;

            if message.is_empty() {
                interval.tick().await;
                continue;
            }

            for message_set in message.iter() {
                let _topic = message_set.topic();

                for message in message_set.messages() {
                    let _value = message.value;
                }
            }
        }
    }
}
