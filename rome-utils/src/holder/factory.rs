use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use super::Holder;

/// A factory for creating holders
#[derive(Clone, Default)]
pub struct HolderFactory(Arc<AtomicU64>);

impl HolderFactory {
    /// Create a new instance of [HolderFactory]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new instance of [Holder]
    pub async fn lock_holder(&self) -> Holder {
        Holder::new(self.0.clone()).await
    }

    /// Get the number of available holders
    pub fn available_holders(&self) -> u8 {
        let bitset = self.0.load(std::sync::atomic::Ordering::Relaxed);

        (0..64).fold(0, |acc, i| acc + ((bitset >> i) & 1) as u8)
    }
}
