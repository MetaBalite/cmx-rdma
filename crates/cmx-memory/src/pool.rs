use std::sync::atomic::{AtomicU32, Ordering};

use parking_lot::Mutex;
use thiserror::Error;

/// Global counter for assigning unique pool IDs.
static NEXT_POOL_ID: AtomicU32 = AtomicU32::new(0);

#[derive(Debug, Error)]
pub enum PoolError {
    #[error("mmap failed: {0}")]
    MmapFailed(std::io::Error),
    #[error("total_size ({total_size}) must be a positive multiple of block_size ({block_size})")]
    InvalidSize {
        total_size: usize,
        block_size: usize,
    },
    #[error("block_size must be greater than zero")]
    ZeroBlockSize,
}

/// Statistics about pool utilization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolStats {
    pub total_blocks: usize,
    pub used_blocks: usize,
    pub free_blocks: usize,
    pub total_bytes: usize,
    pub used_bytes: usize,
}

/// Handle to an allocated block within a [`MemoryPool`].
///
/// # Safety invariant
///
/// A `BlockHandle` must not outlive the `MemoryPool` that issued it. Using a
/// handle after the pool is dropped is undefined behavior. The pool does **not**
/// enforce this at the type level -- callers are responsible for ensuring the
/// lifetime relationship holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockHandle {
    pub pool_id: u32,
    pub block_index: u32,
    pub offset: usize,
    pub size: usize,
}

/// A pinned DRAM slab pool.
///
/// Pre-allocates a large anonymous memory region via `mmap` and hands out
/// fixed-size blocks through an O(1) free-list allocator.
///
/// # Safety invariant
///
/// The pool owns the mapped memory region. All [`BlockHandle`]s returned by
/// [`allocate`](MemoryPool::allocate) borrow from this region and **must not**
/// outlive the pool.
pub struct MemoryPool {
    pool_id: u32,
    /// Pointer to the start of the mmap'd region.
    base_ptr: *mut u8,
    total_size: usize,
    block_size: usize,
    total_blocks: usize,
    /// Free-list implemented as a stack of block indices. Protected by a mutex
    /// because allocations are infrequent relative to reads.
    free_list: Mutex<Vec<usize>>,
}

// SAFETY: The mmap'd region is owned exclusively by this struct and access is
// synchronized via the Mutex on the free list. Raw pointer sends are safe
// because we control the lifetime of the underlying memory.
unsafe impl Send for MemoryPool {}
unsafe impl Sync for MemoryPool {}

impl MemoryPool {
    /// Create a new memory pool.
    ///
    /// `total_size` bytes are allocated via `mmap(MAP_ANONYMOUS | MAP_PRIVATE)`.
    /// On Linux the pages are pinned with `mlock`; if `mlock` fails (e.g. no
    /// `CAP_IPC_LOCK`) a warning is logged and execution continues so that
    /// dev/test environments work without elevated privileges.
    ///
    /// `total_size` must be a positive multiple of `block_size`.
    pub fn new(total_size: usize, block_size: usize) -> Result<Self, PoolError> {
        if block_size == 0 {
            return Err(PoolError::ZeroBlockSize);
        }
        if total_size == 0 || !total_size.is_multiple_of(block_size) {
            return Err(PoolError::InvalidSize {
                total_size,
                block_size,
            });
        }

        let total_blocks = total_size / block_size;

        // SAFETY: We request an anonymous private mapping with no file
        // descriptor backing. The kernel returns a zeroed region or fails.
        let base_ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                total_size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_ANONYMOUS | libc::MAP_PRIVATE,
                -1, // no fd
                0,  // no offset
            )
        };

        if base_ptr == libc::MAP_FAILED {
            return Err(PoolError::MmapFailed(std::io::Error::last_os_error()));
        }

        let base_ptr = base_ptr as *mut u8;

        // SAFETY: `base_ptr` is a valid mapping of `total_size` bytes.
        // mlock pins the pages so they are not swapped out, which is important
        // for RDMA registered memory. Failure is non-fatal for dev/test.
        #[cfg(target_os = "linux")]
        {
            let ret = unsafe { libc::mlock(base_ptr as *const libc::c_void, total_size) };
            if ret != 0 {
                tracing::warn!(
                    error = %std::io::Error::last_os_error(),
                    "mlock failed — pages may be swapped; \
                     consider raising RLIMIT_MEMLOCK for production use"
                );
            }
        }

        // Build the free list with all block indices (0..total_blocks).
        let free_list: Vec<usize> = (0..total_blocks).collect();

        let pool_id = NEXT_POOL_ID.fetch_add(1, Ordering::Relaxed);

        tracing::info!(
            pool_id,
            total_size,
            block_size,
            total_blocks,
            "memory pool created"
        );

        Ok(Self {
            pool_id,
            base_ptr,
            total_size,
            block_size,
            total_blocks,
            free_list: Mutex::new(free_list),
        })
    }

    /// Allocate a single block, returning `None` if the pool is exhausted.
    pub fn allocate(&self) -> Option<BlockHandle> {
        let mut free = self.free_list.lock();
        let block_index = free.pop()?;
        let offset = block_index * self.block_size;
        Some(BlockHandle {
            pool_id: self.pool_id,
            block_index: block_index as u32,
            offset,
            size: self.block_size,
        })
    }

    /// Return a block to the pool.
    ///
    /// # Panics
    ///
    /// Panics if the handle does not belong to this pool (pool_id mismatch) or
    /// if the block index is out of range.
    ///
    /// # Double-free behavior
    ///
    /// Deallocating the same handle twice pushes the index onto the free list
    /// twice, which means a subsequent `allocate` may hand out the same block
    /// to two callers. This is a logic error but does **not** cause undefined
    /// behavior on its own -- it is the caller's responsibility to avoid it.
    pub fn deallocate(&self, handle: BlockHandle) {
        assert_eq!(
            handle.pool_id, self.pool_id,
            "BlockHandle pool_id {} does not match this pool {}",
            handle.pool_id, self.pool_id
        );
        assert!(
            (handle.block_index as usize) < self.total_blocks,
            "block_index {} out of range (total_blocks = {})",
            handle.block_index,
            self.total_blocks
        );

        let mut free = self.free_list.lock();
        free.push(handle.block_index as usize);
    }

    /// Get a raw mutable pointer to the start of a block's memory.
    ///
    /// # Panics
    ///
    /// Panics if the handle does not belong to this pool.
    pub fn get_ptr(&self, handle: &BlockHandle) -> *mut u8 {
        assert_eq!(handle.pool_id, self.pool_id, "BlockHandle pool_id mismatch");
        // SAFETY: `handle.offset` is always `block_index * block_size` and was
        // validated at allocation time, so it is within the mapped region.
        unsafe { self.base_ptr.add(handle.offset) }
    }

    /// Get an immutable byte slice over a block's memory.
    ///
    /// # Safety
    ///
    /// The caller must ensure no mutable references to this block exist
    /// concurrently. The pool cannot enforce this because handles are `Copy`.
    ///
    /// # Panics
    ///
    /// Panics if the handle does not belong to this pool.
    pub fn get_slice(&self, handle: &BlockHandle) -> &[u8] {
        let ptr = self.get_ptr(handle);
        // SAFETY: The mapped region is valid for `self.block_size` bytes
        // starting at `ptr`. The caller upholds the aliasing invariant.
        unsafe { std::slice::from_raw_parts(ptr, self.block_size) }
    }

    /// Get a mutable byte slice over a block's memory.
    ///
    /// # Safety
    ///
    /// The caller must ensure exclusive access to this block. The pool cannot
    /// enforce this because handles are `Copy`. This method intentionally takes
    /// `&self` (not `&mut self`) because the pool manages a raw mmap'd region
    /// and multiple blocks can be mutated concurrently by different callers,
    /// similar to `UnsafeCell` interior mutability.
    ///
    /// # Panics
    ///
    /// Panics if the handle does not belong to this pool.
    #[allow(clippy::mut_from_ref)]
    pub fn get_slice_mut(&self, handle: &BlockHandle) -> &mut [u8] {
        let ptr = self.get_ptr(handle);
        // SAFETY: Same as `get_slice`, plus the caller guarantees exclusivity.
        unsafe { std::slice::from_raw_parts_mut(ptr, self.block_size) }
    }

    /// Block size in bytes.
    pub fn block_size(&self) -> usize {
        self.block_size
    }

    /// Return current pool utilization statistics.
    pub fn stats(&self) -> PoolStats {
        let free = self.free_list.lock();
        let free_blocks = free.len();
        let used_blocks = self.total_blocks.saturating_sub(free_blocks);
        PoolStats {
            total_blocks: self.total_blocks,
            used_blocks,
            free_blocks,
            total_bytes: self.total_size,
            used_bytes: used_blocks * self.block_size,
        }
    }
}

impl Drop for MemoryPool {
    fn drop(&mut self) {
        // SAFETY: `self.base_ptr` was obtained from a successful `mmap` call
        // with size `self.total_size`. We unmap the entire region.
        let ret = unsafe { libc::munmap(self.base_ptr as *mut libc::c_void, self.total_size) };
        if ret != 0 {
            tracing::error!(
                error = %std::io::Error::last_os_error(),
                pool_id = self.pool_id,
                "munmap failed during MemoryPool drop"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK_SIZE: usize = 4096;
    const NUM_BLOCKS: usize = 4;
    const TOTAL_SIZE: usize = BLOCK_SIZE * NUM_BLOCKS;

    #[test]
    fn allocate_deallocate_cycle() {
        let pool = MemoryPool::new(TOTAL_SIZE, BLOCK_SIZE).unwrap();

        let handle = pool.allocate().expect("should allocate");
        assert_eq!(handle.size, BLOCK_SIZE);

        let stats = pool.stats();
        assert_eq!(stats.used_blocks, 1);
        assert_eq!(stats.free_blocks, NUM_BLOCKS - 1);

        pool.deallocate(handle);

        let stats = pool.stats();
        assert_eq!(stats.used_blocks, 0);
        assert_eq!(stats.free_blocks, NUM_BLOCKS);
    }

    #[test]
    fn pool_exhaustion_returns_none() {
        let pool = MemoryPool::new(TOTAL_SIZE, BLOCK_SIZE).unwrap();

        let mut handles = Vec::new();
        for _ in 0..NUM_BLOCKS {
            handles.push(pool.allocate().expect("should allocate"));
        }

        assert!(pool.allocate().is_none(), "pool should be exhausted");

        // Deallocate one and verify we can allocate again.
        pool.deallocate(handles.pop().unwrap());
        assert!(pool.allocate().is_some(), "should allocate after dealloc");
    }

    #[test]
    fn double_deallocate_is_safe() {
        // Double-deallocate pushes the same index twice onto the free list.
        // This is a logic error but must not cause UB or a panic.
        let pool = MemoryPool::new(TOTAL_SIZE, BLOCK_SIZE).unwrap();

        let handle = pool.allocate().unwrap();
        pool.deallocate(handle);
        pool.deallocate(handle); // second dealloc -- same handle

        let stats = pool.stats();
        // free_blocks can exceed total_blocks after a double-free;
        // used_blocks saturates at 0 instead of underflowing.
        assert_eq!(stats.free_blocks, NUM_BLOCKS + 1);
        assert_eq!(stats.used_blocks, 0);
    }

    #[test]
    fn read_write_through_block_handles() {
        let pool = MemoryPool::new(TOTAL_SIZE, BLOCK_SIZE).unwrap();

        let handle = pool.allocate().unwrap();

        // Write a pattern.
        let slice = pool.get_slice_mut(&handle);
        for (i, byte) in slice.iter_mut().enumerate() {
            *byte = (i % 256) as u8;
        }

        // Read it back.
        let slice = pool.get_slice(&handle);
        for (i, &byte) in slice.iter().enumerate() {
            assert_eq!(byte, (i % 256) as u8);
        }

        pool.deallocate(handle);
    }

    #[test]
    fn stats_accuracy() {
        let pool = MemoryPool::new(TOTAL_SIZE, BLOCK_SIZE).unwrap();

        let stats = pool.stats();
        assert_eq!(stats.total_blocks, NUM_BLOCKS);
        assert_eq!(stats.used_blocks, 0);
        assert_eq!(stats.free_blocks, NUM_BLOCKS);
        assert_eq!(stats.total_bytes, TOTAL_SIZE);
        assert_eq!(stats.used_bytes, 0);

        let h1 = pool.allocate().unwrap();
        let h2 = pool.allocate().unwrap();

        let stats = pool.stats();
        assert_eq!(stats.used_blocks, 2);
        assert_eq!(stats.used_bytes, 2 * BLOCK_SIZE);

        pool.deallocate(h1);
        pool.deallocate(h2);
    }

    #[test]
    fn zero_block_size_is_error() {
        assert!(MemoryPool::new(4096, 0).is_err());
    }

    #[test]
    fn misaligned_total_size_is_error() {
        assert!(MemoryPool::new(4097, 4096).is_err());
    }

    #[test]
    fn zero_total_size_is_error() {
        assert!(MemoryPool::new(0, 4096).is_err());
    }

    #[test]
    fn blocks_have_distinct_memory() {
        let pool = MemoryPool::new(TOTAL_SIZE, BLOCK_SIZE).unwrap();

        let h1 = pool.allocate().unwrap();
        let h2 = pool.allocate().unwrap();

        // Write different patterns.
        pool.get_slice_mut(&h1).fill(0xAA);
        pool.get_slice_mut(&h2).fill(0xBB);

        // Verify they didn't interfere.
        assert!(pool.get_slice(&h1).iter().all(|&b| b == 0xAA));
        assert!(pool.get_slice(&h2).iter().all(|&b| b == 0xBB));

        pool.deallocate(h1);
        pool.deallocate(h2);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    /// Operation in a random alloc/dealloc sequence.
    #[derive(Debug, Clone)]
    enum Op {
        Allocate,
        Deallocate,
    }

    fn op_strategy() -> impl Strategy<Value = Op> {
        prop_oneof![Just(Op::Allocate), Just(Op::Deallocate),]
    }

    proptest! {
        #[test]
        fn random_alloc_dealloc_sequences(ops in proptest::collection::vec(op_strategy(), 1..200)) {
            let block_size = 128;
            let num_blocks = 16;
            let pool = MemoryPool::new(block_size * num_blocks, block_size).unwrap();

            let mut live_handles: Vec<BlockHandle> = Vec::new();

            for op in &ops {
                match op {
                    Op::Allocate => {
                        if let Some(handle) = pool.allocate() {
                            // Write a recognizable pattern.
                            pool.get_slice_mut(&handle).fill(0xCD);
                            live_handles.push(handle);
                        }
                        // None is fine — pool exhausted.
                    }
                    Op::Deallocate => {
                        if let Some(handle) = live_handles.pop() {
                            pool.deallocate(handle);
                        }
                    }
                }
            }

            // Invariant: used_blocks == live_handles.len()
            let stats = pool.stats();
            prop_assert_eq!(stats.used_blocks, live_handles.len());
            prop_assert_eq!(stats.free_blocks, num_blocks - live_handles.len());

            // Cleanup
            for h in live_handles {
                pool.deallocate(h);
            }
        }
    }
}
