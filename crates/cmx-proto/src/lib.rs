pub mod cmx {
    tonic::include_proto!("cmx");
}

pub use cmx::*;
pub use cmx::cmx_cache_server::{CmxCache, CmxCacheServer};
pub use cmx::cmx_cache_client::CmxCacheClient;
