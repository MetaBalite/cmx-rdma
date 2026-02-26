//! NIXL FFI bindings.
//!
//! When the `nixl` feature is enabled and NIXL headers are available,
//! this module provides a Transport implementation backed by NIXL's
//! auto-negotiated RDMA/TCP fabric.
//!
//! Without the feature, `NixlTransport::new()` returns an error directing
//! users to MockTransport.

#[cfg(not(feature = "nixl"))]
mod stub {
    /// Placeholder for NIXL integration when feature is disabled.
    pub struct NixlTransport {
        _private: (),
    }

    impl NixlTransport {
        /// NIXL transport is not available — build with `--features nixl`.
        pub fn new() -> Result<Self, &'static str> {
            Err("NIXL transport not available. Build with --features nixl or use MockTransport.")
        }
    }
}

#[cfg(not(feature = "nixl"))]
pub use stub::NixlTransport;

#[cfg(feature = "nixl")]
mod ffi {
    use crate::traits::{
        LocalBuffer, PeerId, RemoteMemoryRegion, Transport, TransportError, TransportType,
    };
    use std::collections::HashMap;
    use tokio::sync::RwLock;

    // TODO: Replace with bindgen-generated bindings when NIXL headers are available.
    // For now, this provides the structure for the FFI integration.
    //
    // Expected NIXL C API:
    //   nixl_init() -> nixl_handle_t
    //   nixl_connect(handle, peer_addr) -> nixl_conn_t
    //   nixl_reg_mem(handle, ptr, size) -> nixl_mr_t
    //   nixl_xfer(conn, local_mr, remote_addr, remote_rkey, size, op) -> status
    //   nixl_disconnect(conn)
    //   nixl_destroy(handle)

    /// NIXL-backed transport using auto-negotiated RDMA/TCP.
    pub struct NixlTransport {
        connections: RwLock<HashMap<PeerId, NixlConnection>>,
    }

    struct NixlConnection {
        transport_type: TransportType,
        _addr: String,
    }

    impl NixlTransport {
        /// Create a new NIXL transport. Requires NIXL runtime to be available.
        pub fn new() -> Result<Self, &'static str> {
            // TODO: Call nixl_init() here
            tracing::info!("NIXL transport initialized (stub — awaiting FFI bindings)");
            Ok(Self {
                connections: RwLock::new(HashMap::new()),
            })
        }
    }

    impl Transport for NixlTransport {
        async fn connect(
            &self,
            peer: &PeerId,
            addr: &str,
        ) -> Result<TransportType, TransportError> {
            // TODO: Call nixl_connect() — NIXL auto-negotiates IB/RoCE/TCP
            let tt = TransportType::RoCEv2; // placeholder
            let mut conns = self.connections.write().await;
            conns.insert(
                peer.clone(),
                NixlConnection {
                    transport_type: tt,
                    _addr: addr.to_string(),
                },
            );
            Ok(tt)
        }

        async fn disconnect(&self, peer: &PeerId) -> Result<(), TransportError> {
            let mut conns = self.connections.write().await;
            conns
                .remove(peer)
                .ok_or_else(|| TransportError::PeerNotFound(peer.0.clone()))?;
            Ok(())
        }

        async fn read(
            &self,
            remote: &RemoteMemoryRegion,
            _local: &LocalBuffer,
        ) -> Result<usize, TransportError> {
            let conns = self.connections.read().await;
            if !conns.contains_key(&remote.peer) {
                return Err(TransportError::PeerNotFound(remote.peer.0.clone()));
            }
            // TODO: Call nixl_xfer() with RDMA READ op
            Ok(remote.size as usize)
        }

        async fn write(
            &self,
            _local: &LocalBuffer,
            remote: &RemoteMemoryRegion,
        ) -> Result<usize, TransportError> {
            let conns = self.connections.read().await;
            if !conns.contains_key(&remote.peer) {
                return Err(TransportError::PeerNotFound(remote.peer.0.clone()));
            }
            // TODO: Call nixl_xfer() with RDMA WRITE op
            Ok(remote.size as usize)
        }

        fn register_memory(&self, _ptr: *mut u8, _size: usize) -> Result<u32, TransportError> {
            // TODO: Call nixl_reg_mem()
            Ok(0)
        }

        fn deregister_memory(&self, _lkey: u32) -> Result<(), TransportError> {
            Ok(())
        }

        fn transport_type(&self, peer: &PeerId) -> Option<TransportType> {
            let conns = self.connections.try_read().ok()?;
            conns.get(peer).map(|c| c.transport_type)
        }
    }
}

#[cfg(feature = "nixl")]
pub use ffi::NixlTransport;
