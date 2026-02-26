//! Slab allocator for fixed-size KV cache blocks.
//!
//! Wraps a `cmx_memory::MemoryPool` and adds block-level management:
//! header writing, CRC32C integrity, generation tracking.

use cmx_memory::pool::{BlockHandle, MemoryPool};

use crate::block::{BlockHeader, BlockLocation};

/// Allocates and manages KV cache blocks from a memory pool.
pub struct BlockAllocator {
    pool: MemoryPool,
    /// Local node ID (used in BlockLocation).
    node_id: u32,
    /// Monotonically increasing generation counter.
    generation: std::sync::atomic::AtomicU64,
}

impl BlockAllocator {
    pub fn new(pool: MemoryPool, node_id: u32) -> Self {
        Self {
            pool,
            node_id,
            generation: std::sync::atomic::AtomicU64::new(1),
        }
    }

    /// Allocate a block and write data into it with a header.
    /// Returns the block location and handle, or None if the pool is exhausted.
    pub fn allocate_and_write(&self, data: &[u8]) -> Option<(BlockLocation, BlockHandle)> {
        let max_data = self.pool.block_size() - BlockHeader::SIZE;
        if data.len() > max_data {
            tracing::warn!(
                data_len = data.len(),
                max_data,
                "data exceeds block capacity"
            );
            return None;
        }

        let handle = self.pool.allocate()?;
        let gen = self
            .generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // Write header + data into the block
        let block_slice = self.pool.get_slice_mut(&handle);
        let header = BlockHeader::new(data, gen);
        header.write_to(&mut block_slice[..BlockHeader::SIZE]);
        block_slice[BlockHeader::SIZE..BlockHeader::SIZE + data.len()].copy_from_slice(data);

        let location = BlockLocation {
            node_id: self.node_id,
            block_index: handle.block_index,
            offset: handle.offset as u64,
            block_size: self.pool.block_size() as u64,
            generation: gen,
        };

        Some((location, handle))
    }

    /// Read data from a block, verifying CRC32C integrity.
    /// Returns the data (without header) if checksum matches.
    pub fn read_verified(&self, handle: &BlockHandle) -> Option<Vec<u8>> {
        let block_slice = self.pool.get_slice(handle);
        let header = BlockHeader::read_from(block_slice)?;

        let data_start = BlockHeader::SIZE;
        let data_end = data_start + header.data_size as usize;
        if data_end > block_slice.len() {
            return None;
        }
        let data = &block_slice[data_start..data_end];

        if !header.verify(data) {
            tracing::warn!(
                block_index = handle.block_index,
                "CRC32C mismatch — data corruption detected"
            );
            return None;
        }

        Some(data.to_vec())
    }

    /// Deallocate a block, returning it to the free pool.
    pub fn deallocate(&self, handle: BlockHandle) {
        self.pool.deallocate(handle);
    }

    /// Get pool statistics.
    pub fn stats(&self) -> cmx_memory::pool::PoolStats {
        self.pool.stats()
    }

    /// Get the raw memory pointer for a block (for RDMA registration).
    pub fn block_ptr(&self, handle: &BlockHandle) -> *mut u8 {
        self.pool.get_ptr(handle)
    }

    /// Current generation counter value.
    pub fn current_generation(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pool(num_blocks: usize, block_size: usize) -> MemoryPool {
        MemoryPool::new(num_blocks * block_size, block_size).unwrap()
    }

    #[test]
    fn test_allocate_and_read() {
        let pool = make_pool(4, 4096);
        let alloc = BlockAllocator::new(pool, 1);

        let data = b"hello KV cache";
        let (loc, handle) = alloc.allocate_and_write(data).unwrap();

        assert_eq!(loc.node_id, 1);
        assert_eq!(loc.generation, 1);

        let readback = alloc.read_verified(&handle).unwrap();
        assert_eq!(readback, data);
    }

    #[test]
    fn test_generation_increments() {
        let pool = make_pool(4, 4096);
        let alloc = BlockAllocator::new(pool, 0);

        let (loc1, _h1) = alloc.allocate_and_write(b"a").unwrap();
        let (loc2, _h2) = alloc.allocate_and_write(b"b").unwrap();

        assert_eq!(loc1.generation, 1);
        assert_eq!(loc2.generation, 2);
    }

    #[test]
    fn test_data_too_large() {
        let block_size = 64;
        let pool = make_pool(2, block_size);
        let alloc = BlockAllocator::new(pool, 0);

        let data = vec![0u8; block_size]; // exactly block_size, but header needs space too
        assert!(alloc.allocate_and_write(&data).is_none());
    }

    #[test]
    fn test_deallocate_and_reuse() {
        let pool = make_pool(1, 4096);
        let alloc = BlockAllocator::new(pool, 0);

        let (_loc, handle) = alloc.allocate_and_write(b"first").unwrap();
        assert!(alloc.allocate_and_write(b"second").is_none()); // pool full

        alloc.deallocate(handle);
        let result = alloc.allocate_and_write(b"third");
        assert!(result.is_some()); // slot reused
    }
}
