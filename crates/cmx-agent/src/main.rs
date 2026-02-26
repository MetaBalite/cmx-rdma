//! cmx-agent: per-node daemon for distributed KV cache sharing over RDMA.

use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use tonic::transport::Server;
use tracing_subscriber::EnvFilter;

use cmx_agent::config::AgentConfig;
use cmx_agent::grpc::CacheService;
use cmx_block_store::{BlockAllocator, BlockIndex};
use cmx_memory::MemoryPool;
use cmx_proto::cmx_cache_server::CmxCacheServer;

#[derive(Parser)]
#[command(name = "cmx-agent", version, about = "CMX RDMA cache agent")]
struct Cli {
    /// Path to configuration file.
    #[arg(short, long, default_value = "/etc/cmx/cmx-agent.toml")]
    config: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Load config (fall back to defaults if file doesn't exist).
    let mut config = if std::path::Path::new(&cli.config).exists() {
        AgentConfig::load(&cli.config)
            .with_context(|| format!("failed to load config from {}", cli.config))?
    } else {
        tracing::info!(path = %cli.config, "config file not found, using defaults");
        AgentConfig::default_config()
    };

    // Initialize tracing.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(&config.agent.log_level)),
        )
        .with_target(true)
        .init();

    config.resolve_node_id();
    tracing::info!(node_id = %config.agent.node_id, "starting cmx-agent");

    // Initialize memory pool.
    let pool = MemoryPool::new(config.memory.total_size, config.memory.block_size)
        .context("failed to create memory pool")?;
    tracing::info!(
        total_size = config.memory.total_size,
        block_size = config.memory.block_size,
        total_blocks = pool.stats().total_blocks,
        "memory pool initialized"
    );

    // Initialize block allocator and index.
    let allocator = Arc::new(BlockAllocator::new(pool, 0));
    let max_entries = config.memory.total_size / config.memory.block_size;
    let index = Arc::new(BlockIndex::new(max_entries));

    // Build gRPC service.
    let cache_service = CacheService::new(
        allocator.clone(),
        index.clone(),
        config.agent.node_id.clone(),
    );

    let addr = config
        .agent
        .listen_addr
        .parse()
        .context("invalid listen address")?;

    tracing::info!(%addr, "gRPC server starting");

    Server::builder()
        .add_service(CmxCacheServer::new(cache_service))
        .serve(addr)
        .await
        .context("gRPC server failed")?;

    Ok(())
}
