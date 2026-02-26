//! Consistent hashing ring for cache placement.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Consistent hash ring with virtual nodes.
pub struct HashRing {
    ring: BTreeMap<u64, String>,
    vnodes_per_node: u32,
    nodes: Vec<String>,
}

impl HashRing {
    pub fn new(vnodes_per_node: u32) -> Self {
        Self {
            ring: BTreeMap::new(),
            vnodes_per_node,
            nodes: Vec::new(),
        }
    }

    /// Add a node to the ring, creating `vnodes_per_node` virtual nodes.
    pub fn add_node(&mut self, node_id: String) {
        if self.nodes.contains(&node_id) {
            return;
        }
        for i in 0..self.vnodes_per_node {
            let vnode_key = format!("{}:{}", node_id, i);
            let pos = Self::hash_position(&vnode_key);
            self.ring.insert(pos, node_id.clone());
        }
        self.nodes.push(node_id);
    }

    /// Remove a node and all its virtual nodes from the ring.
    pub fn remove_node(&mut self, node_id: &str) {
        self.ring.retain(|_, v| v != node_id);
        self.nodes.retain(|n| n != node_id);
    }

    /// Get the preferred node for a given prefix hash.
    pub fn preferred_node(&self, prefix_hash: &[u8; 16]) -> Option<&str> {
        if self.ring.is_empty() {
            return None;
        }
        let pos = Self::hash_position(&hex::encode(prefix_hash));
        // Find the first node at or after this position (wrapping around).
        self.ring
            .range(pos..)
            .next()
            .or_else(|| self.ring.iter().next())
            .map(|(_, node_id)| node_id.as_str())
    }

    /// Get N preferred nodes for a given prefix hash (for replication).
    pub fn preferred_nodes(&self, prefix_hash: &[u8; 16], n: usize) -> Vec<&str> {
        if self.ring.is_empty() {
            return Vec::new();
        }
        let pos = Self::hash_position(&hex::encode(prefix_hash));
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // Walk the ring from the hash position, collecting unique nodes.
        for (_, node_id) in self.ring.range(pos..).chain(self.ring.iter()) {
            if seen.insert(node_id.as_str()) {
                result.push(node_id.as_str());
                if result.len() >= n {
                    break;
                }
            }
        }
        result
    }

    /// Number of physical nodes in the ring.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    fn hash_position(key: &str) -> u64 {
        let hash = Sha256::digest(key.as_bytes());
        u64::from_be_bytes(hash[..8].try_into().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_ring_returns_none() {
        let ring = HashRing::new(128);
        let hash = [0u8; 16];
        assert!(ring.preferred_node(&hash).is_none());
    }

    #[test]
    fn single_node_always_preferred() {
        let mut ring = HashRing::new(128);
        ring.add_node("node-a".to_string());

        for i in 0..10u8 {
            let mut h = [0u8; 16];
            h[0] = i;
            assert_eq!(ring.preferred_node(&h), Some("node-a"));
        }
    }

    #[test]
    fn add_and_remove_node() {
        let mut ring = HashRing::new(128);
        ring.add_node("node-a".to_string());
        ring.add_node("node-b".to_string());
        assert_eq!(ring.node_count(), 2);

        ring.remove_node("node-a");
        assert_eq!(ring.node_count(), 1);

        // All lookups should return node-b now
        let h = [1u8; 16];
        assert_eq!(ring.preferred_node(&h), Some("node-b"));
    }

    #[test]
    fn distribution_is_reasonable() {
        let mut ring = HashRing::new(128);
        ring.add_node("node-a".to_string());
        ring.add_node("node-b".to_string());
        ring.add_node("node-c".to_string());

        let mut counts = std::collections::HashMap::new();
        for i in 0u16..1000 {
            let mut h = [0u8; 16];
            h[..2].copy_from_slice(&i.to_be_bytes());
            let node = ring.preferred_node(&h).unwrap();
            *counts.entry(node.to_string()).or_insert(0u32) += 1;
        }

        // Each node should get at least 15% of keys (expected ~33%)
        for count in counts.values() {
            assert!(*count > 150, "uneven distribution: {counts:?}");
        }
    }

    #[test]
    fn minimal_key_movement_on_add() {
        let mut ring = HashRing::new(128);
        ring.add_node("node-a".to_string());
        ring.add_node("node-b".to_string());

        // Record assignments
        let mut before = Vec::new();
        for i in 0u16..1000 {
            let mut h = [0u8; 16];
            h[..2].copy_from_slice(&i.to_be_bytes());
            before.push(ring.preferred_node(&h).unwrap().to_string());
        }

        // Add a third node
        ring.add_node("node-c".to_string());

        let mut moved = 0;
        for i in 0u16..1000 {
            let mut h = [0u8; 16];
            h[..2].copy_from_slice(&i.to_be_bytes());
            let after = ring.preferred_node(&h).unwrap();
            if after != before[i as usize] {
                moved += 1;
            }
        }

        // Ideal movement is ~1/3 of keys. Allow up to 50%.
        assert!(moved < 500, "too many keys moved: {moved}/1000");
    }

    #[test]
    fn preferred_nodes_returns_n_unique() {
        let mut ring = HashRing::new(128);
        ring.add_node("node-a".to_string());
        ring.add_node("node-b".to_string());
        ring.add_node("node-c".to_string());

        let h = [42u8; 16];
        let nodes = ring.preferred_nodes(&h, 3);
        assert_eq!(nodes.len(), 3);
        // All unique
        let unique: std::collections::HashSet<_> = nodes.iter().collect();
        assert_eq!(unique.len(), 3);
    }
}
