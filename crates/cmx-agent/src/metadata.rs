//! etcd metadata client for node registration and block location publishing.

use std::sync::Arc;

use etcd_client::{Client, LeaseKeepAliveStream, LeaseKeeper, PutOptions};
use tokio::sync::RwLock;

use crate::placement::HashRing;
use crate::remote_index::{RemoteBlockInfo, RemoteIndex};

/// Information about a registered node.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NodeInfo {
    pub node_id: String,
    pub addr: String,
}

/// Metadata stored for a block location in etcd.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BlockLocationMeta {
    pub node_id: String,
    pub node_addr: String,
    pub offset: u64,
    pub size: u64,
    pub generation: u64,
}

/// etcd metadata client for distributed coordination.
pub struct MetadataClient {
    client: RwLock<Client>,
    lease_id: RwLock<Option<i64>>,
    node_id: String,
    node_addr: String,
    lease_ttl: i64,
}

impl MetadataClient {
    /// Connect to etcd endpoints.
    pub async fn connect(
        endpoints: &[String],
        node_id: String,
        node_addr: String,
        lease_ttl: i64,
    ) -> anyhow::Result<Self> {
        let client = Client::connect(endpoints, None).await?;
        tracing::info!(?endpoints, "connected to etcd");
        Ok(Self {
            client: RwLock::new(client),
            lease_id: RwLock::new(None),
            node_id,
            node_addr,
            lease_ttl,
        })
    }

    /// Register this node with an etcd lease.
    pub async fn register_node(&self) -> anyhow::Result<()> {
        let mut client = self.client.write().await;
        let lease = client.lease_grant(self.lease_ttl, None).await?;
        let id = lease.id();
        *self.lease_id.write().await = Some(id);

        let key = format!("/cmx/nodes/{}", self.node_id);
        let value = serde_json::to_string(&NodeInfo {
            node_id: self.node_id.clone(),
            addr: self.node_addr.clone(),
        })
        .unwrap_or_default();

        let opts = PutOptions::new().with_lease(id);
        client.put(key, value, Some(opts)).await?;
        tracing::info!(node_id = %self.node_id, lease_id = id, "registered node in etcd");
        Ok(())
    }

    /// Start a background keepalive task that renews the lease every TTL/3.
    /// Returns the join handle so the caller can abort on shutdown.
    pub async fn start_keepalive(&self) -> anyhow::Result<tokio::task::JoinHandle<()>> {
        let lease_id = self
            .lease_id
            .read()
            .await
            .ok_or_else(|| anyhow::anyhow!("no lease to keep alive"))?;

        let mut client = self.client.write().await;
        let (keeper, stream) = client.lease_keep_alive(lease_id).await?;
        drop(client);

        let interval = std::time::Duration::from_secs((self.lease_ttl / 3).max(1) as u64);
        let handle = tokio::spawn(keepalive_loop(keeper, stream, interval));
        tracing::info!(lease_id, ?interval, "lease keepalive started");
        Ok(handle)
    }

    /// List all registered nodes.
    pub async fn list_nodes(&self) -> anyhow::Result<Vec<NodeInfo>> {
        let mut client = self.client.write().await;
        let resp = client
            .get(
                "/cmx/nodes/",
                Some(etcd_client::GetOptions::new().with_prefix()),
            )
            .await?;

        let mut nodes = Vec::new();
        for kv in resp.kvs() {
            if let Ok(info) = serde_json::from_slice::<NodeInfo>(kv.value()) {
                nodes.push(info);
            }
        }
        Ok(nodes)
    }

    /// Publish a block's location to etcd.
    pub async fn publish_block(
        &self,
        prefix_hash: &[u8; 16],
        offset: u64,
        size: u64,
        generation: u64,
    ) -> anyhow::Result<()> {
        let key = format!("/cmx/blocks/{}", hex::encode(prefix_hash));
        let meta = BlockLocationMeta {
            node_id: self.node_id.clone(),
            node_addr: self.node_addr.clone(),
            offset,
            size,
            generation,
        };
        let value = serde_json::to_string(&meta)?;

        let lease_id = *self.lease_id.read().await;
        let opts = lease_id.map(|id| PutOptions::new().with_lease(id));

        let mut client = self.client.write().await;
        client.put(key, value, opts).await?;
        Ok(())
    }

    /// Lookup a block's location in etcd.
    pub async fn lookup_block(
        &self,
        prefix_hash: &[u8; 16],
    ) -> anyhow::Result<Option<RemoteBlockInfo>> {
        let key = format!("/cmx/blocks/{}", hex::encode(prefix_hash));
        let mut client = self.client.write().await;
        let resp = client.get(key, None).await?;

        if let Some(kv) = resp.kvs().first() {
            if let Ok(meta) = serde_json::from_slice::<BlockLocationMeta>(kv.value()) {
                return Ok(Some(RemoteBlockInfo {
                    node_id: meta.node_id,
                    node_addr: meta.node_addr,
                    offset: meta.offset,
                    size: meta.size,
                    generation: meta.generation,
                }));
            }
        }
        Ok(None)
    }

    /// Start watching for block and node changes. Updates the remote index
    /// and hash ring as events arrive.
    pub async fn start_block_watch(
        &self,
        remote_index: Arc<RemoteIndex>,
        hash_ring: Option<Arc<RwLock<HashRing>>>,
    ) -> anyhow::Result<tokio::task::JoinHandle<()>> {
        let mut client = self.client.write().await;

        // Watch /cmx/blocks/ prefix
        let (_, block_stream) = client
            .watch(
                "/cmx/blocks/",
                Some(etcd_client::WatchOptions::new().with_prefix()),
            )
            .await?;

        // Watch /cmx/nodes/ prefix for membership changes
        let (_, node_stream) = client
            .watch(
                "/cmx/nodes/",
                Some(etcd_client::WatchOptions::new().with_prefix()),
            )
            .await?;
        drop(client);

        let remote_index_clone = remote_index.clone();
        let hash_ring_clone = hash_ring.clone();

        let handle = tokio::spawn(async move {
            tokio::select! {
                _ = watch_blocks(block_stream, remote_index_clone) => {}
                _ = watch_nodes(node_stream, remote_index, hash_ring_clone) => {}
            }
        });

        tracing::info!("started etcd watches for blocks and nodes");
        Ok(handle)
    }

    /// Revoke the lease (for graceful shutdown).
    pub async fn revoke_lease(&self) -> anyhow::Result<()> {
        if let Some(id) = *self.lease_id.read().await {
            let mut client = self.client.write().await;
            client.lease_revoke(id).await?;
            tracing::info!(lease_id = id, "revoked etcd lease");
        }
        Ok(())
    }
}

async fn keepalive_loop(
    mut keeper: LeaseKeeper,
    mut stream: LeaseKeepAliveStream,
    interval: std::time::Duration,
) {
    loop {
        tokio::time::sleep(interval).await;
        if let Err(e) = keeper.keep_alive().await {
            tracing::error!(error = %e, "lease keepalive request failed");
            return;
        }
        match stream.message().await {
            Ok(Some(resp)) => {
                if resp.ttl() == 0 {
                    tracing::warn!("lease expired");
                    return;
                }
            }
            Ok(None) => {
                tracing::warn!("keepalive stream ended");
                return;
            }
            Err(e) => {
                tracing::error!(error = %e, "keepalive stream error");
                return;
            }
        }
    }
}

async fn watch_blocks(mut stream: etcd_client::WatchStream, remote_index: Arc<RemoteIndex>) {
    use etcd_client::EventType;

    loop {
        match stream.message().await {
            Ok(Some(resp)) => {
                for event in resp.events() {
                    if let Some(kv) = event.kv() {
                        let key_str = String::from_utf8_lossy(kv.key());
                        // Extract hex hash from key like "/cmx/blocks/<hex>"
                        let hex_part = key_str.trim_start_matches("/cmx/blocks/");
                        let hash_bytes = match hex::decode(hex_part) {
                            Ok(b) if b.len() == 16 => {
                                let mut arr = [0u8; 16];
                                arr.copy_from_slice(&b);
                                arr
                            }
                            _ => continue,
                        };

                        match event.event_type() {
                            EventType::Put => {
                                if let Ok(meta) =
                                    serde_json::from_slice::<BlockLocationMeta>(kv.value())
                                {
                                    let info = RemoteBlockInfo {
                                        node_id: meta.node_id,
                                        node_addr: meta.node_addr,
                                        offset: meta.offset,
                                        size: meta.size,
                                        generation: meta.generation,
                                    };
                                    remote_index.insert(hash_bytes, info);
                                }
                            }
                            EventType::Delete => {
                                remote_index.remove(&hash_bytes);
                            }
                        }
                    }
                }
            }
            Ok(None) => {
                tracing::warn!("block watch stream ended");
                return;
            }
            Err(e) => {
                tracing::error!(error = %e, "block watch error");
                return;
            }
        }
    }
}

async fn watch_nodes(
    mut stream: etcd_client::WatchStream,
    remote_index: Arc<RemoteIndex>,
    hash_ring: Option<Arc<RwLock<HashRing>>>,
) {
    use etcd_client::EventType;

    loop {
        match stream.message().await {
            Ok(Some(resp)) => {
                for event in resp.events() {
                    if let Some(kv) = event.kv() {
                        let key_str = String::from_utf8_lossy(kv.key());
                        let node_id = key_str.trim_start_matches("/cmx/nodes/").to_string();

                        match event.event_type() {
                            EventType::Put => {
                                if let Some(ring) = &hash_ring {
                                    let mut ring = ring.write().await;
                                    ring.add_node(node_id.clone());
                                }
                                tracing::info!(node_id = %node_id, "node joined");
                            }
                            EventType::Delete => {
                                remote_index.remove_node(&node_id);
                                if let Some(ring) = &hash_ring {
                                    let mut ring = ring.write().await;
                                    ring.remove_node(&node_id);
                                }
                                tracing::warn!(node_id = %node_id, "node left (lease expired)");
                            }
                        }
                    }
                }
            }
            Ok(None) => {
                tracing::warn!("node watch stream ended");
                return;
            }
            Err(e) => {
                tracing::error!(error = %e, "node watch error");
                return;
            }
        }
    }
}
