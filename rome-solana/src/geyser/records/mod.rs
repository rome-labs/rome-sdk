/// Transaction record
pub mod tx;

/// Account record
pub mod account;

/// Slot record
pub mod slot;

/// Trait to implement for all records that can be sent to the kafka topics
pub trait GeyserKafkaRecord {
    /// The topic to send the record to
    fn topic(&self) -> &'static str;
    /// The key to use for the record
    fn key(&self) -> String;
}
