//! Connection pool for reusing transport connections to remote peers.

use std::sync::Arc;

use dashmap::DashMap;

use crate::traits::{PeerId, Transport, TransportError, TransportType};

/// State of a connection to a remote peer.
#[derive(Debug, Clone)]
pub struct ConnectionState {
    pub transport_type: TransportType,
    pub addr: String,
    pub connected_at: std::time::Instant,
}

/// Pool of transport connections, keyed by PeerId.
/// Lazily connects on first use and caches the connection.
pub struct ConnectionPool<T: Transport> {
    transport: Arc<T>,
    connections: DashMap<PeerId, ConnectionState>,
}

impl<T: Transport> ConnectionPool<T> {
    pub fn new(transport: Arc<T>) -> Self {
        Self {
            transport,
            connections: DashMap::new(),
        }
    }

    /// Get an existing connection or create a new one.
    pub async fn get_or_connect(
        &self,
        peer: &PeerId,
        addr: &str,
    ) -> Result<TransportType, TransportError> {
        // Check cache first.
        if let Some(entry) = self.connections.get(peer) {
            return Ok(entry.transport_type);
        }

        // Connect and cache.
        let tt = self.transport.connect(peer, addr).await?;
        self.connections.insert(
            peer.clone(),
            ConnectionState {
                transport_type: tt,
                addr: addr.to_string(),
                connected_at: std::time::Instant::now(),
            },
        );
        Ok(tt)
    }

    /// Disconnect a peer and remove from cache.
    pub async fn disconnect(&self, peer: &PeerId) -> Result<(), TransportError> {
        self.connections.remove(peer);
        self.transport.disconnect(peer).await
    }

    /// List connected peers.
    pub fn connected_peers(&self) -> Vec<PeerId> {
        self.connections.iter().map(|e| e.key().clone()).collect()
    }

    /// Start a background health check that removes stale connections.
    pub fn start_health_check(self: &Arc<Self>, interval_secs: u64) -> tokio::task::JoinHandle<()>
    where
        T: 'static,
    {
        let pool = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
            loop {
                interval.tick().await;

                let peers: Vec<PeerId> = pool.connections.iter().map(|e| e.key().clone()).collect();

                for peer in peers {
                    if pool.transport.transport_type(&peer).is_none() {
                        // Connection is stale — remove from cache.
                        pool.connections.remove(&peer);
                        tracing::debug!(peer = %peer.0, "removed stale connection");
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockTransport;

    #[tokio::test]
    async fn get_or_connect_reuses_connection() {
        let transport = Arc::new(MockTransport::new());
        let pool = ConnectionPool::new(transport);

        let peer = PeerId("node-1".to_string());
        let tt1 = pool.get_or_connect(&peer, "127.0.0.1:9000").await.unwrap();
        let tt2 = pool.get_or_connect(&peer, "127.0.0.1:9000").await.unwrap();

        assert_eq!(tt1, TransportType::Mock);
        assert_eq!(tt2, TransportType::Mock);
        assert_eq!(pool.connected_peers().len(), 1);
    }

    #[tokio::test]
    async fn disconnect_removes_from_pool() {
        let transport = Arc::new(MockTransport::new());
        let pool = ConnectionPool::new(transport);

        let peer = PeerId("node-1".to_string());
        pool.get_or_connect(&peer, "127.0.0.1:9000").await.unwrap();
        assert_eq!(pool.connected_peers().len(), 1);

        pool.disconnect(&peer).await.unwrap();
        assert_eq!(pool.connected_peers().len(), 0);
    }

    #[tokio::test]
    async fn multiple_peers() {
        let transport = Arc::new(MockTransport::new());
        let pool = ConnectionPool::new(transport);

        for i in 0..3 {
            let peer = PeerId(format!("node-{i}"));
            pool.get_or_connect(&peer, &format!("127.0.0.1:900{i}"))
                .await
                .unwrap();
        }

        assert_eq!(pool.connected_peers().len(), 3);
    }
}
