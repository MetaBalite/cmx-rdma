//! Prefix hash → block location index.
//!
//! Maps SHA-256 truncated prefix hashes (16 bytes) to block locations.
//! Uses `dashmap` for lock-free concurrent reads with sharded locking on writes.

use dashmap::DashMap;

use crate::block::BlockLocation;
use crate::eviction::LruPolicy;

/// An entry in the index: one prefix hash can map to multiple contiguous blocks.
#[derive(Debug, Clone)]
pub struct IndexEntry {
    /// Locations of all blocks for this prefix (ordered by sequence).
    pub blocks: Vec<BlockLocation>,
    /// Timestamp (monotonic) of when this entry was created.
    pub created_at: u64,
}

/// Concurrent prefix hash → block location index with LRU eviction.
pub struct BlockIndex {
    map: DashMap<[u8; 16], IndexEntry>,
    lru: LruPolicy,
    /// Maximum number of entries before eviction kicks in.
    capacity: usize,
}

impl BlockIndex {
    pub fn new(capacity: usize) -> Self {
        Self {
            map: DashMap::with_capacity(capacity),
            lru: LruPolicy::new(),
            capacity,
        }
    }

    /// Look up blocks for a prefix hash. Returns None if not found.
    /// Touches the LRU on hit.
    pub fn lookup(&self, prefix_hash: &[u8; 16]) -> Option<IndexEntry> {
        let entry = self.map.get(prefix_hash)?;
        self.lru.touch(prefix_hash);
        Some(entry.clone())
    }

    /// Insert blocks for a prefix hash. Evicts LRU entries if at capacity.
    /// Returns the prefix hashes of any evicted entries.
    pub fn insert(
        &self,
        prefix_hash: [u8; 16],
        blocks: Vec<BlockLocation>,
        created_at: u64,
    ) -> Vec<[u8; 16]> {
        let mut evicted = Vec::new();

        // Evict if at capacity (and this is a new entry, not an update).
        if !self.map.contains_key(&prefix_hash) {
            while self.map.len() >= self.capacity {
                if let Some(victim) = self.lru.evict_lru() {
                    self.map.remove(&victim);
                    evicted.push(victim);
                } else {
                    break;
                }
            }
        }

        let entry = IndexEntry { blocks, created_at };
        self.map.insert(prefix_hash, entry);
        self.lru.touch(&prefix_hash);

        evicted
    }

    /// Remove an entry by prefix hash.
    pub fn remove(&self, prefix_hash: &[u8; 16]) -> Option<IndexEntry> {
        let (_, entry) = self.map.remove(prefix_hash)?;
        self.lru.remove(prefix_hash);
        Some(entry)
    }

    /// Number of entries in the index.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Check if a prefix hash exists without touching LRU.
    pub fn contains(&self, prefix_hash: &[u8; 16]) -> bool {
        self.map.contains_key(prefix_hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(n: u8) -> [u8; 16] {
        let mut h = [0u8; 16];
        h[0] = n;
        h
    }

    fn dummy_location(gen: u64) -> BlockLocation {
        BlockLocation {
            node_id: 0,
            block_index: 0,
            offset: 0,
            block_size: 4096,
            generation: gen,
        }
    }

    #[test]
    fn test_insert_and_lookup() {
        let idx = BlockIndex::new(100);
        let h = hash(1);
        let blocks = vec![dummy_location(1)];

        idx.insert(h, blocks.clone(), 1000);
        let entry = idx.lookup(&h).unwrap();
        assert_eq!(entry.blocks.len(), 1);
        assert_eq!(entry.blocks[0].generation, 1);
        assert_eq!(entry.created_at, 1000);
    }

    #[test]
    fn test_lookup_miss() {
        let idx = BlockIndex::new(100);
        assert!(idx.lookup(&hash(99)).is_none());
    }

    #[test]
    fn test_remove() {
        let idx = BlockIndex::new(100);
        idx.insert(hash(1), vec![dummy_location(1)], 0);
        assert!(idx.contains(&hash(1)));

        let removed = idx.remove(&hash(1));
        assert!(removed.is_some());
        assert!(!idx.contains(&hash(1)));
    }

    #[test]
    fn test_eviction_at_capacity() {
        let idx = BlockIndex::new(3); // capacity of 3

        idx.insert(hash(1), vec![dummy_location(1)], 0);
        idx.insert(hash(2), vec![dummy_location(2)], 0);
        idx.insert(hash(3), vec![dummy_location(3)], 0);

        // This should evict hash(1) (LRU)
        let evicted = idx.insert(hash(4), vec![dummy_location(4)], 0);
        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0], hash(1));
        assert!(!idx.contains(&hash(1)));
        assert!(idx.contains(&hash(4)));
    }

    #[test]
    fn test_touch_prevents_eviction() {
        let idx = BlockIndex::new(3);

        idx.insert(hash(1), vec![dummy_location(1)], 0);
        idx.insert(hash(2), vec![dummy_location(2)], 0);
        idx.insert(hash(3), vec![dummy_location(3)], 0);

        // Touch hash(1) — it should no longer be LRU
        idx.lookup(&hash(1));

        // Insert hash(4) — should evict hash(2) (now the LRU)
        let evicted = idx.insert(hash(4), vec![dummy_location(4)], 0);
        assert_eq!(evicted[0], hash(2));
        assert!(idx.contains(&hash(1)));
    }

    #[test]
    fn test_update_existing_no_eviction() {
        let idx = BlockIndex::new(2);

        idx.insert(hash(1), vec![dummy_location(1)], 0);
        idx.insert(hash(2), vec![dummy_location(2)], 0);

        // Update hash(1) — should NOT trigger eviction
        let evicted = idx.insert(hash(1), vec![dummy_location(10)], 100);
        assert!(evicted.is_empty());
        assert_eq!(idx.len(), 2);

        let entry = idx.lookup(&hash(1)).unwrap();
        assert_eq!(entry.blocks[0].generation, 10);
    }
}
