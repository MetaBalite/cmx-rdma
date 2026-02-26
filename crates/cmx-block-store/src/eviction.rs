//! LRU eviction policy for the block store.
//!
//! Tracks access recency per prefix hash. When the block store is full,
//! the least recently used entries are evicted to make room.

use std::collections::HashMap;

use parking_lot::Mutex;

/// An entry in the LRU list.
#[derive(Debug)]
struct LruEntry {
    prefix_hash: [u8; 16],
    prev: Option<usize>,
    next: Option<usize>,
}

/// LRU eviction tracker using an intrusive doubly-linked list backed by a Vec (slab).
///
/// - O(1) touch (move to front)
/// - O(1) evict (remove from back)
/// - O(1) remove by key (via index lookup)
pub struct LruPolicy {
    inner: Mutex<LruInner>,
}

struct LruInner {
    /// Slab storage for list nodes.
    entries: Vec<LruEntry>,
    /// Free indices in the slab (reusable slots).
    free_indices: Vec<usize>,
    /// Map from prefix_hash → slab index.
    index: HashMap<[u8; 16], usize>,
    /// Head of the list (most recently used).
    head: Option<usize>,
    /// Tail of the list (least recently used).
    tail: Option<usize>,
}

impl LruPolicy {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(LruInner {
                entries: Vec::new(),
                free_indices: Vec::new(),
                index: HashMap::new(),
                head: None,
                tail: None,
            }),
        }
    }

    /// Record an access (insert or touch). Moves the entry to the front (most recent).
    pub fn touch(&self, prefix_hash: &[u8; 16]) {
        let mut inner = self.inner.lock();
        if let Some(&idx) = inner.index.get(prefix_hash) {
            // Already exists — unlink and move to front.
            inner.unlink(idx);
            inner.push_front(idx);
        } else {
            // New entry — allocate a slot and push to front.
            let idx = inner.alloc_entry(*prefix_hash);
            inner.push_front(idx);
            inner.index.insert(*prefix_hash, idx);
        }
    }

    /// Remove an entry from the LRU (e.g., on explicit delete).
    pub fn remove(&self, prefix_hash: &[u8; 16]) {
        let mut inner = self.inner.lock();
        if let Some(idx) = inner.index.remove(prefix_hash) {
            inner.unlink(idx);
            inner.free_indices.push(idx);
        }
    }

    /// Evict the least recently used entry. Returns its prefix_hash, or None if empty.
    pub fn evict_lru(&self) -> Option<[u8; 16]> {
        let mut inner = self.inner.lock();
        let tail_idx = inner.tail?;
        let hash = inner.entries[tail_idx].prefix_hash;
        inner.index.remove(&hash);
        inner.unlink(tail_idx);
        inner.free_indices.push(tail_idx);
        Some(hash)
    }

    /// Number of entries tracked.
    pub fn len(&self) -> usize {
        self.inner.lock().index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for LruPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl LruInner {
    fn alloc_entry(&mut self, prefix_hash: [u8; 16]) -> usize {
        if let Some(idx) = self.free_indices.pop() {
            self.entries[idx] = LruEntry {
                prefix_hash,
                prev: None,
                next: None,
            };
            idx
        } else {
            let idx = self.entries.len();
            self.entries.push(LruEntry {
                prefix_hash,
                prev: None,
                next: None,
            });
            idx
        }
    }

    fn unlink(&mut self, idx: usize) {
        let prev = self.entries[idx].prev;
        let next = self.entries[idx].next;

        if let Some(p) = prev {
            self.entries[p].next = next;
        } else {
            self.head = next;
        }

        if let Some(n) = next {
            self.entries[n].prev = prev;
        } else {
            self.tail = prev;
        }

        self.entries[idx].prev = None;
        self.entries[idx].next = None;
    }

    fn push_front(&mut self, idx: usize) {
        self.entries[idx].prev = None;
        self.entries[idx].next = self.head;

        if let Some(old_head) = self.head {
            self.entries[old_head].prev = Some(idx);
        }

        self.head = Some(idx);

        if self.tail.is_none() {
            self.tail = Some(idx);
        }
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

    #[test]
    fn test_evict_order() {
        let lru = LruPolicy::new();
        lru.touch(&hash(1));
        lru.touch(&hash(2));
        lru.touch(&hash(3));

        // LRU order: 3 (head) → 2 → 1 (tail)
        assert_eq!(lru.evict_lru(), Some(hash(1)));
        assert_eq!(lru.evict_lru(), Some(hash(2)));
        assert_eq!(lru.evict_lru(), Some(hash(3)));
        assert_eq!(lru.evict_lru(), None);
    }

    #[test]
    fn test_touch_moves_to_front() {
        let lru = LruPolicy::new();
        lru.touch(&hash(1));
        lru.touch(&hash(2));
        lru.touch(&hash(3));

        // Touch 1 again — moves it to front
        lru.touch(&hash(1));

        // LRU order: 1 (head) → 3 → 2 (tail)
        assert_eq!(lru.evict_lru(), Some(hash(2)));
        assert_eq!(lru.evict_lru(), Some(hash(3)));
        assert_eq!(lru.evict_lru(), Some(hash(1)));
    }

    #[test]
    fn test_remove() {
        let lru = LruPolicy::new();
        lru.touch(&hash(1));
        lru.touch(&hash(2));
        lru.touch(&hash(3));

        lru.remove(&hash(2));
        assert_eq!(lru.len(), 2);

        assert_eq!(lru.evict_lru(), Some(hash(1)));
        assert_eq!(lru.evict_lru(), Some(hash(3)));
    }

    #[test]
    fn test_empty_evict() {
        let lru = LruPolicy::new();
        assert_eq!(lru.evict_lru(), None);
        assert!(lru.is_empty());
    }
}
