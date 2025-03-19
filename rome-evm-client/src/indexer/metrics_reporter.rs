use solana_sdk::clock::Slot;

pub trait MetricsReporter: Send + Sync {
    fn report_latest_solana_block(&self, slot: Slot);

    fn report_stored_solana_block(&self, slot: Slot);

    fn report_processed_solana_block(&self, slot: Slot);
}
