//! Consolidated agent state shared across gRPC handlers and background tasks.

use std::sync::atomic::AtomicU8;
use std::sync::Arc;

use tokio::sync::RwLock;

use cmx_block_store::{BlockAllocator, BlockIndex};

use crate::config::AgentConfig;
use crate::metadata::MetadataClient;
use crate::placement::HashRing;
use crate::remote_index::RemoteIndex;

/// Shared state for the cmx-agent process.
pub struct AgentState {
    pub allocator: Arc<BlockAllocator>,
    pub index: Arc<BlockIndex>,
    pub node_id: String,
    pub config: Arc<AgentConfig>,
    pub metadata: Option<Arc<MetadataClient>>,
    /// 0=normal, 1=warn, 2=critical, 3=reject
    pub pressure_level: Arc<AtomicU8>,
    pub remote_index: Option<Arc<RemoteIndex>>,
    pub hash_ring: Option<Arc<RwLock<HashRing>>>,
}
