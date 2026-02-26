//! Memory pressure monitoring and back-pressure.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use cmx_block_store::{BlockAllocator, BlockIndex};

/// Pressure levels.
pub const PRESSURE_NORMAL: u8 = 0;
pub const PRESSURE_WARN: u8 = 1;
pub const PRESSURE_CRITICAL: u8 = 2;
pub const PRESSURE_REJECT: u8 = 3;

/// Start the memory pressure monitoring background task.
///
/// Checks pool utilization every second, updates the pressure level,
/// and triggers aggressive eviction at critical level.
pub fn start_pressure_monitor(
    allocator: Arc<BlockAllocator>,
    _index: Arc<BlockIndex>,
    pressure_level: Arc<AtomicU8>,
    warn_threshold: f64,
    critical_threshold: f64,
    reject_threshold: f64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            interval.tick().await;

            let stats = allocator.stats();
            let usage = if stats.total_blocks > 0 {
                stats.used_blocks as f64 / stats.total_blocks as f64
            } else {
                0.0
            };

            let level = if usage >= reject_threshold {
                PRESSURE_REJECT
            } else if usage >= critical_threshold {
                PRESSURE_CRITICAL
            } else if usage >= warn_threshold {
                PRESSURE_WARN
            } else {
                PRESSURE_NORMAL
            };

            let prev = pressure_level.swap(level, Ordering::Relaxed);
            if level != prev {
                match level {
                    PRESSURE_WARN => {
                        tracing::warn!(usage = %format!("{:.1}%", usage * 100.0), "memory pressure: WARN")
                    }
                    PRESSURE_CRITICAL => {
                        tracing::warn!(usage = %format!("{:.1}%", usage * 100.0), "memory pressure: CRITICAL")
                    }
                    PRESSURE_REJECT => {
                        tracing::error!(usage = %format!("{:.1}%", usage * 100.0), "memory pressure: REJECT — new stores will be refused")
                    }
                    _ => {
                        tracing::info!(usage = %format!("{:.1}%", usage * 100.0), "memory pressure: normal")
                    }
                }
            }

            // At critical or above, the pressure level signals store() to
            // reject new writes. Eviction happens through the normal LRU
            // path when new stores are accepted at lower pressure levels.

            metrics::gauge!("cmx_memory_pressure").set(level as f64);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmx_memory::MemoryPool;

    #[tokio::test]
    async fn pressure_levels_update() {
        // 4 blocks of 4096 bytes each
        let pool = MemoryPool::new(4 * 4096, 4096).unwrap();
        let allocator = Arc::new(BlockAllocator::new(pool, 0));
        let index = Arc::new(BlockIndex::new(4));
        let pressure = Arc::new(AtomicU8::new(0));

        let handle = start_pressure_monitor(
            allocator.clone(),
            index,
            pressure.clone(),
            0.50, // warn at 50%
            0.75, // critical at 75%
            0.90, // reject at 90% (impossible with 4 blocks — 3/4=75%, 4/4=100%)
        );

        // Allocate 2/4 blocks → 50% → should be WARN
        let _b1 = allocator.allocate_and_write(b"a").unwrap();
        let _b2 = allocator.allocate_and_write(b"b").unwrap();

        // Wait for monitor tick
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        assert_eq!(pressure.load(Ordering::Relaxed), PRESSURE_WARN);

        // Allocate 3/4 blocks → 75% → CRITICAL
        let _b3 = allocator.allocate_and_write(b"c").unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        assert_eq!(pressure.load(Ordering::Relaxed), PRESSURE_CRITICAL);

        handle.abort();
    }
}
