use std::time::Duration;

use kafka::client::RequiredAcks;
use kafka::producer::{AsBytes, Producer, Record};
use serde::Serialize;

use super::records::GeyserKafkaRecord;

/// Geyser Kafka Producer interface
pub struct GeyserKaftaProducer {
    producer: Producer,
}

impl GeyserKaftaProducer {
    /// Create a new [KafkaMessageProducer] instance.
    pub fn new(brokers: Vec<String>) -> anyhow::Result<Self> {
        Producer::from_hosts(brokers)
            // ~ give the brokers one second time to ack the message
            .with_ack_timeout(Duration::from_secs(1))
            // ~ require only one broker to ack the message
            .with_required_acks(RequiredAcks::One)
            // ~ build the producer with the above settings
            .create()
            // - handle errors
            .map_err(anyhow::Error::msg)
            // - return the producer
            .map(|producer| Self { producer })
    }

    /// Raw send a message to a topic.
    pub fn send_raw<M: AsBytes>(
        &mut self,
        topic: &str,
        key: &str,
        message: M,
    ) -> anyhow::Result<()> {
        let record = Record {
            key,
            value: message,
            topic,
            partition: -1,
        };

        self.producer.send(&record).map_err(anyhow::Error::msg)
    }

    /// [GeyserKafkaRecord] to bytes
    pub fn record_to_bytes<T: GeyserKafkaRecord + Serialize>(record: T) -> anyhow::Result<Vec<u8>> {
        bincode::serialize(&record).map_err(|e| anyhow::anyhow!("Failed to serialize: {:?}", e))
    }

    /// Send a message to a topic.
    pub fn send(&mut self, record: impl GeyserKafkaRecord + Serialize) -> anyhow::Result<()> {
        let topic = record.topic();
        let key = record.key();
        let message = Self::record_to_bytes(record)?;

        self.send_raw(topic, &key, message)
    }
}
