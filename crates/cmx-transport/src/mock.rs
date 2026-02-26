//! Mock transport for unit tests.
//!
//! Implements the Transport trait with in-process memory copies. No actual network.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

use tokio::sync::RwLock;

use crate::traits::{
    LocalBuffer, PeerId, RemoteMemoryRegion, Transport, TransportError, TransportType,
};

/// Mock transport that simulates RDMA via in-process memory copies.
pub struct MockTransport {
    /// Connected peers.
    connections: RwLock<HashMap<PeerId, ()>>,
    /// Simulated remote memory: (PeerId, addr) -> bytes.
    remote_memory: RwLock<HashMap<(PeerId, u64), Vec<u8>>>,
    /// Incrementing counter for local memory keys.
    next_lkey: AtomicU32,
}

impl MockTransport {
    pub fn new() -> Self {
        Self {
            connections: RwLock::new(HashMap::new()),
            remote_memory: RwLock::new(HashMap::new()),
            next_lkey: AtomicU32::new(1),
        }
    }

    /// Pre-populate simulated remote memory for testing reads.
    pub async fn insert_remote_memory(&self, peer: PeerId, addr: u64, data: Vec<u8>) {
        let mut mem = self.remote_memory.write().await;
        mem.insert((peer, addr), data);
    }
}

impl Default for MockTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for MockTransport {
    async fn connect(&self, peer: &PeerId, _addr: &str) -> Result<TransportType, TransportError> {
        let mut conns = self.connections.write().await;
        conns.insert(peer.clone(), ());
        tracing::debug!(peer = %peer.0, "mock: connected");
        Ok(TransportType::Mock)
    }

    async fn disconnect(&self, peer: &PeerId) -> Result<(), TransportError> {
        let mut conns = self.connections.write().await;
        if conns.remove(peer).is_none() {
            return Err(TransportError::PeerNotFound(peer.0.clone()));
        }
        tracing::debug!(peer = %peer.0, "mock: disconnected");
        Ok(())
    }

    async fn read(
        &self,
        remote: &RemoteMemoryRegion,
        local: &LocalBuffer,
    ) -> Result<usize, TransportError> {
        let conns = self.connections.read().await;
        if !conns.contains_key(&remote.peer) {
            return Err(TransportError::PeerNotFound(remote.peer.0.clone()));
        }
        drop(conns);

        let mem = self.remote_memory.read().await;
        let key = (remote.peer.clone(), remote.addr);
        let data = mem.get(&key).ok_or_else(|| {
            TransportError::TransferFailed(format!(
                "no remote memory at addr {:#x} for peer {}",
                remote.addr, remote.peer.0
            ))
        })?;

        let bytes_to_copy = local.size.min(data.len()).min(remote.size as usize);
        // Safety: LocalBuffer guarantees ptr is valid for `size` bytes and we copy
        // at most `bytes_to_copy` which is <= local.size.
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), local.ptr, bytes_to_copy);
        }
        Ok(bytes_to_copy)
    }

    async fn write(
        &self,
        local: &LocalBuffer,
        remote: &RemoteMemoryRegion,
    ) -> Result<usize, TransportError> {
        let conns = self.connections.read().await;
        if !conns.contains_key(&remote.peer) {
            return Err(TransportError::PeerNotFound(remote.peer.0.clone()));
        }
        drop(conns);

        let bytes_to_copy = local.size.min(remote.size as usize);
        // Safety: LocalBuffer guarantees ptr is valid for `size` bytes.
        let data = unsafe { std::slice::from_raw_parts(local.ptr, bytes_to_copy) };

        let mut mem = self.remote_memory.write().await;
        let key = (remote.peer.clone(), remote.addr);
        mem.insert(key, data.to_vec());

        Ok(bytes_to_copy)
    }

    fn register_memory(&self, _ptr: *mut u8, _size: usize) -> Result<u32, TransportError> {
        let lkey = self.next_lkey.fetch_add(1, Ordering::Relaxed);
        Ok(lkey)
    }

    fn deregister_memory(&self, _lkey: u32) -> Result<(), TransportError> {
        // No-op for mock.
        Ok(())
    }

    fn transport_type(&self, peer: &PeerId) -> Option<TransportType> {
        // We can't call async from a sync fn, so we use try_read.
        let conns = self.connections.try_read().ok()?;
        if conns.contains_key(peer) {
            Some(TransportType::Mock)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connect_disconnect_lifecycle() {
        let transport = MockTransport::new();
        let peer = PeerId("node-1".to_string());

        // Connect
        let tt = transport.connect(&peer, "127.0.0.1:9000").await.unwrap();
        assert_eq!(tt, TransportType::Mock);
        assert_eq!(transport.transport_type(&peer), Some(TransportType::Mock));

        // Disconnect
        transport.disconnect(&peer).await.unwrap();
        assert_eq!(transport.transport_type(&peer), None);

        // Disconnect again should fail
        let err = transport.disconnect(&peer).await.unwrap_err();
        assert!(matches!(err, TransportError::PeerNotFound(_)));
    }

    #[tokio::test]
    async fn write_then_read_roundtrip() {
        let transport = MockTransport::new();
        let peer = PeerId("node-2".to_string());
        transport.connect(&peer, "127.0.0.1:9001").await.unwrap();

        let lkey = transport
            .register_memory(std::ptr::null_mut(), 0)
            .unwrap();

        // Write data to remote
        let src_data: Vec<u8> = vec![0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE];
        let mut src_buf = src_data.clone();
        let local_write = LocalBuffer {
            ptr: src_buf.as_mut_ptr(),
            size: src_buf.len(),
            lkey,
        };
        let remote = RemoteMemoryRegion {
            peer: peer.clone(),
            addr: 0x1000,
            size: src_buf.len() as u64,
            rkey: 42,
        };
        let written = transport.write(&local_write, &remote).await.unwrap();
        assert_eq!(written, src_data.len());

        // Read data back
        let mut dst_buf = vec![0u8; src_data.len()];
        let local_read = LocalBuffer {
            ptr: dst_buf.as_mut_ptr(),
            size: dst_buf.len(),
            lkey,
        };
        let read = transport.read(&remote, &local_read).await.unwrap();
        assert_eq!(read, src_data.len());
        assert_eq!(dst_buf, src_data);
    }

    #[tokio::test]
    async fn read_from_nonexistent_peer_errors() {
        let transport = MockTransport::new();
        let peer = PeerId("ghost".to_string());

        let mut buf = vec![0u8; 64];
        let local = LocalBuffer {
            ptr: buf.as_mut_ptr(),
            size: buf.len(),
            lkey: 0,
        };
        let remote = RemoteMemoryRegion {
            peer,
            addr: 0x0,
            size: 64,
            rkey: 0,
        };

        let err = transport.read(&remote, &local).await.unwrap_err();
        assert!(matches!(err, TransportError::PeerNotFound(_)));
    }
}
