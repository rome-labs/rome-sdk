use solana_sdk::clock::Slot;

use super::GeyserKafkaRecord;

/// Record for a slot update.
pub struct SlotRecord {
    /// The slot
    pub slot: Slot,
}

impl GeyserKafkaRecord for SlotRecord {
    fn topic(&self) -> &'static str {
        "slot"
    }

    fn key(&self) -> String {
        format!("{:?}", self.slot)
    }
}

impl From<Slot> for SlotRecord {
    fn from(slot: Slot) -> Self {
        Self { slot }
    }
}
