/// Identifies a remote peer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PeerId(pub String);

/// A registered memory region that can be used for RDMA operations.
#[derive(Debug, Clone)]
pub struct RemoteMemoryRegion {
    pub peer: PeerId,
    pub addr: u64,
    pub size: u64,
    pub rkey: u32,
}

/// Local buffer for RDMA operations.
#[derive(Debug)]
pub struct LocalBuffer {
    pub ptr: *mut u8,
    pub size: usize,
    pub lkey: u32,
}

// Safety: LocalBuffer is Send/Sync if the underlying memory is thread-safe (pinned pool).
unsafe impl Send for LocalBuffer {}
unsafe impl Sync for LocalBuffer {}

/// Transport error types.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("connection failed to peer {0}: {1}")]
    ConnectionFailed(String, String),
    #[error("transfer failed: {0}")]
    TransferFailed(String),
    #[error("peer not found: {0}")]
    PeerNotFound(String),
    #[error("timeout after {0}ms")]
    Timeout(u64),
    #[error("transport not available: {0}")]
    NotAvailable(String),
}

/// What transport was negotiated (for observability).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportType {
    InfiniBand,
    RoCEv2,
    Tcp,
    Mock,
}

impl std::fmt::Display for TransportType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InfiniBand => write!(f, "InfiniBand"),
            Self::RoCEv2 => write!(f, "RoCE v2"),
            Self::Tcp => write!(f, "TCP"),
            Self::Mock => write!(f, "Mock"),
        }
    }
}

/// The core transport trait. Implementations wrap NIXL, TCP, etc.
pub trait Transport: Send + Sync + 'static {
    /// Connect to a remote peer. Returns negotiated transport type.
    fn connect(
        &self,
        peer: &PeerId,
        addr: &str,
    ) -> impl std::future::Future<Output = Result<TransportType, TransportError>> + Send;

    /// Disconnect from a peer.
    fn disconnect(
        &self,
        peer: &PeerId,
    ) -> impl std::future::Future<Output = Result<(), TransportError>> + Send;

    /// One-sided RDMA READ: read from remote memory into local buffer.
    /// The remote CPU is NOT involved (NIC handles it).
    fn read(
        &self,
        remote: &RemoteMemoryRegion,
        local: &LocalBuffer,
    ) -> impl std::future::Future<Output = Result<usize, TransportError>> + Send;

    /// One-sided RDMA WRITE: write local buffer to remote memory.
    fn write(
        &self,
        local: &LocalBuffer,
        remote: &RemoteMemoryRegion,
    ) -> impl std::future::Future<Output = Result<usize, TransportError>> + Send;

    /// Register a local memory region for RDMA access by remote peers.
    fn register_memory(&self, ptr: *mut u8, size: usize) -> Result<u32, TransportError>;

    /// Deregister a memory region.
    fn deregister_memory(&self, lkey: u32) -> Result<(), TransportError>;

    /// Get the negotiated transport type for a peer.
    fn transport_type(&self, peer: &PeerId) -> Option<TransportType>;
}
