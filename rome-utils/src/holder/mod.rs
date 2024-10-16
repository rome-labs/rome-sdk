use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

mod factory;

pub use factory::HolderFactory;

/// A holder of a value
#[derive(Debug)]
pub struct Holder {
    /// The index of the holder
    index: u64,
    /// A bitset to track the usage of holders
    atomic_bitset: Arc<AtomicU64>,
}

impl Holder {
    /// Create a new instance of [Holder]
    #[tracing::instrument]
    pub async fn new(atomic_bitset: Arc<AtomicU64>) -> Self {
        loop {
            let Some(holder) = Self::try_new(atomic_bitset.clone()) else {
                // Yield the task and try again
                tracing::debug!("No available index, yielding");
                tokio::task::yield_now().await;
                // Try again
                continue;
            };

            return holder;
        }
    }

    #[tracing::instrument]
    pub fn try_new(atomic_bitset: Arc<AtomicU64>) -> Option<Self> {
        let mut bitset = atomic_bitset.load(Ordering::Acquire);

        tracing::debug!("Trying to get next available index from bitset: {}", bitset);

        for index in 0..64 {
            let mask = 1 << index;

            // Check if the bit is zero (meaning the index is available)
            if (bitset & mask) == 0 {
                // Atomically try to set the bit at index `i`
                let new_bitset = bitset | mask;

                // If the bit was successfully set, return the index
                match atomic_bitset.compare_exchange(
                    bitset,
                    new_bitset,
                    Ordering::Release,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        tracing::debug!("Using index: {}", index);

                        return Some(Self {
                            index,
                            atomic_bitset,
                        });
                    }
                    Err(current_bitset) => {
                        // Another thread/task modified the bitset, update the local copy and retry
                        bitset = current_bitset;
                    }
                }
            }
        }

        None
    }

    /// Get the index of the holder
    #[tracing::instrument]
    pub fn get_index(&self) -> u64 {
        tracing::debug!("returning holder index {}", self.index);
        self.index
    }

    /// Get the bitset
    pub fn get_bitset(&self) -> u64 {
        self.atomic_bitset.load(Ordering::Relaxed)
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        let mask = 1 << self.index;
        loop {
            let current = self.atomic_bitset.load(Ordering::Acquire);
            if current & mask == 0 {
                // Bit is already clear, nothing to do
                break;
            }
            match self.atomic_bitset.compare_exchange_weak(
                current,
                current & !mask,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(_) => continue,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use tokio::time::{timeout, Duration};

    // Assuming the Holder is in a crate named "my_crate"
    use super::Holder;

    #[tokio::test]
    async fn test_holder_creation() {
        let bitset = Arc::new(AtomicU64::new(0));
        let holder = Holder::new(bitset.clone()).await;
        assert_eq!(holder.get_index(), 0);
        assert_eq!(holder.get_bitset(), 1);
    }

    #[tokio::test]
    async fn test_multiple_holders() {
        let bitset = Arc::new(AtomicU64::new(0));
        let holder1 = Holder::new(bitset.clone()).await;
        let holder2 = Holder::new(bitset.clone()).await;
        let holder3 = Holder::new(bitset.clone()).await;

        assert_ne!(holder1.get_index(), holder2.get_index());
        assert_ne!(holder1.get_index(), holder3.get_index());
        assert_ne!(holder2.get_index(), holder3.get_index());

        assert_eq!(holder3.get_bitset(), 0b111);
    }

    #[tokio::test]
    async fn test_holder_drop() {
        let bitset = Arc::new(AtomicU64::new(0));
        {
            let holder = Holder::new(bitset.clone()).await;
            assert_eq!(holder.get_bitset(), 1);
        }
        // After the holder is dropped, the bitset should be 0 again
        assert_eq!(bitset.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn test_all_bits_set() {
        let bitset = Arc::new(AtomicU64::new(u64::MAX));
        let result = timeout(Duration::from_millis(100), Holder::new(bitset.clone())).await;
        assert!(result.is_err(), "Expected timeout when all bits are set");
    }

    #[tokio::test]
    async fn test_concurrent_holders() {
        let bitset = Arc::new(AtomicU64::new(0));

        let handles = (0..64).map(|_| {
            let bitset = bitset.clone();
            // Spawn a new task to create a holder
            tokio::spawn(async move {
                // Yield the task to allow other tasks to run
                tokio::task::yield_now().await;
                // Create a new holder
                Holder::new(bitset).await
            })
        });

        let holders = futures::future::join_all(handles).await;

        let result = timeout(Duration::from_millis(100), Holder::new(bitset.clone())).await;
        assert!(result.is_err(), "Expected timeout when all bits are set");

        for holder in holders {
            // Unwrap the result
            let holder = holder.expect("Failed to create holder");
            // Get the index of the holder
            let holder_index = holder.get_index();
            // drop the holder
            drop(holder);
            // check if the index bit is cleared
            assert_eq!(bitset.load(Ordering::Relaxed) & (1 << holder_index), 0);
        }
    }
}
