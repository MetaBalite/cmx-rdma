//! Block header and data layout for KV cache blocks.
//!
//! Each block stores a fixed number of tokens' KV cache data (default: 16 tokens,
//! matching vLLM PagedAttention page size). The header contains metadata for
//! integrity checking and staleness detection.

use crc32fast::Hasher;

/// Number of tokens per block (matches vLLM PagedAttention page size).
pub const TOKENS_PER_BLOCK: u32 = 16;

/// Block header stored at the beginning of each allocated block.
/// Kept small to minimize overhead — the rest of the block is raw KV tensor data.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct BlockHeader {
    /// CRC32C checksum of the data region (not including the header).
    pub crc32: u32,
    /// Generation counter. Incremented on each write. Readers compare against
    /// the generation in metadata to detect stale reads after eviction/rewrite.
    pub generation: u64,
    /// Actual data size in bytes (may be less than block capacity).
    pub data_size: u32,
    /// Reserved for future use (alignment padding).
    _reserved: u32,
}

impl BlockHeader {
    pub const SIZE: usize = std::mem::size_of::<BlockHeader>();

    pub fn new(data: &[u8], generation: u64) -> Self {
        let mut hasher = Hasher::new();
        hasher.update(data);
        Self {
            crc32: hasher.finalize(),
            generation,
            data_size: data.len() as u32,
            _reserved: 0,
        }
    }

    /// Verify the CRC32C checksum against the given data.
    pub fn verify(&self, data: &[u8]) -> bool {
        let mut hasher = Hasher::new();
        hasher.update(data);
        hasher.finalize() == self.crc32
    }

    /// Write header to a byte slice (must be at least `BlockHeader::SIZE` bytes).
    ///
    /// # Safety
    /// The caller must ensure `dst` points to at least `BlockHeader::SIZE` writable bytes.
    pub fn write_to(&self, dst: &mut [u8]) {
        assert!(dst.len() >= Self::SIZE);
        // Safety: BlockHeader is repr(C) with no padding concerns for these field types.
        let bytes =
            unsafe { std::slice::from_raw_parts(self as *const Self as *const u8, Self::SIZE) };
        dst[..Self::SIZE].copy_from_slice(bytes);
    }

    /// Read header from a byte slice.
    ///
    /// # Safety
    /// The caller must ensure `src` contains a valid BlockHeader written by `write_to`.
    pub fn read_from(src: &[u8]) -> Option<Self> {
        if src.len() < Self::SIZE {
            return None;
        }
        // Safety: We're reading repr(C) struct from bytes written by write_to.
        let header = unsafe { std::ptr::read_unaligned(src.as_ptr() as *const Self) };
        Some(header)
    }
}

/// Location of a block within the memory pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockLocation {
    /// Node that owns this block (local node ID or remote node ID).
    pub node_id: u32,
    /// Index of the block within the memory pool.
    pub block_index: u32,
    /// Byte offset from the start of the memory pool.
    pub offset: u64,
    /// Size of the block in bytes (including header).
    pub block_size: u64,
    /// Generation counter at time of write.
    pub generation: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_header_roundtrip() {
        let data = b"hello world KV cache data";
        let header = BlockHeader::new(data, 42);

        assert_eq!(header.generation, 42);
        assert_eq!(header.data_size, data.len() as u32);
        assert!(header.verify(data));
        assert!(!header.verify(b"corrupted data"));
    }

    #[test]
    fn test_header_serialize() {
        let data = b"test data";
        let header = BlockHeader::new(data, 7);

        let mut buf = vec![0u8; BlockHeader::SIZE + 16];
        header.write_to(&mut buf);

        let restored = BlockHeader::read_from(&buf).unwrap();
        assert_eq!(restored.crc32, header.crc32);
        assert_eq!(restored.generation, header.generation);
        assert_eq!(restored.data_size, header.data_size);
        assert!(restored.verify(data));
    }

    #[test]
    fn test_header_size() {
        // Header: u32 + padding(4) + u64 + u32 + u32 = 24 bytes (8-byte aligned for u64)
        assert_eq!(BlockHeader::SIZE, 24);
    }
}
