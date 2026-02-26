//! Remote block index — tracks blocks available on other nodes.

use dashmap::DashMap;

/// Information about a block stored on a remote node.
#[derive(Debug, Clone)]
pub struct RemoteBlockInfo {
    pub node_id: String,
    pub node_addr: String,
    pub offset: u64,
    pub size: u64,
    pub generation: u64,
}

/// Index of blocks available on remote nodes.
/// Populated by etcd watch events.
pub struct RemoteIndex {
    map: DashMap<[u8; 16], Vec<RemoteBlockInfo>>,
}

impl RemoteIndex {
    pub fn new() -> Self {
        Self {
            map: DashMap::new(),
        }
    }

    /// Look up remote locations for a prefix hash.
    pub fn lookup(&self, prefix_hash: &[u8; 16]) -> Option<Vec<RemoteBlockInfo>> {
        self.map.get(prefix_hash).map(|v| v.clone())
    }

    /// Insert or update a remote block entry.
    pub fn insert(&self, prefix_hash: [u8; 16], info: RemoteBlockInfo) {
        self.map
            .entry(prefix_hash)
            .and_modify(|entries| {
                // Replace existing entry from same node, or append.
                if let Some(existing) = entries.iter_mut().find(|e| e.node_id == info.node_id) {
                    *existing = info.clone();
                } else {
                    entries.push(info.clone());
                }
            })
            .or_insert_with(|| vec![info]);
    }

    /// Remove all entries for a given prefix hash.
    pub fn remove(&self, prefix_hash: &[u8; 16]) {
        self.map.remove(prefix_hash);
    }

    /// Remove all block entries from a specific node (e.g., when its lease expires).
    pub fn remove_node(&self, node_id: &str) {
        // Retain only entries not from this node.
        self.map.retain(|_, entries| {
            entries.retain(|e| e.node_id != node_id);
            !entries.is_empty()
        });
    }
}

impl Default for RemoteIndex {
    fn default() -> Self {
        Self::new()
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

    fn info(node: &str) -> RemoteBlockInfo {
        RemoteBlockInfo {
            node_id: node.to_string(),
            node_addr: format!("{}:50051", node),
            offset: 0,
            size: 4096,
            generation: 1,
        }
    }

    #[test]
    fn insert_and_lookup() {
        let idx = RemoteIndex::new();
        idx.insert(hash(1), info("node-a"));
        let entries = idx.lookup(&hash(1)).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].node_id, "node-a");
    }

    #[test]
    fn lookup_miss() {
        let idx = RemoteIndex::new();
        assert!(idx.lookup(&hash(99)).is_none());
    }

    #[test]
    fn remove_node_bulk() {
        let idx = RemoteIndex::new();
        idx.insert(hash(1), info("node-a"));
        idx.insert(hash(1), info("node-b"));
        idx.insert(hash(2), info("node-a"));

        idx.remove_node("node-a");

        let entries = idx.lookup(&hash(1)).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].node_id, "node-b");

        // hash(2) only had node-a, so it should be gone entirely
        assert!(idx.lookup(&hash(2)).is_none());
    }
}
