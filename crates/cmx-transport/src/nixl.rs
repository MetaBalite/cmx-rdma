//! NIXL FFI bindings (stub for Phase 1).
//!
//! When NIXL is available, this module will contain:
//! - bindgen-generated FFI bindings to libnixl
//! - Safe Rust wrappers implementing the Transport trait
//! - Auto-negotiation of IB/RoCE/TCP per peer pair
//!
//! For now, use MockTransport for development and testing.

/// Placeholder for future NIXL integration.
pub struct NixlTransport {
    _private: (),
}

impl NixlTransport {
    /// NIXL transport is not yet available. Use MockTransport for testing.
    pub fn new() -> Result<Self, &'static str> {
        Err("NIXL transport not yet implemented. Use MockTransport for development.")
    }
}
