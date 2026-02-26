//! cmx-agent: per-node daemon for distributed KV cache sharing over RDMA.

use std::sync::atomic::AtomicU8;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use tonic::transport::Server;
use tracing_subscriber::EnvFilter;

use cmx_agent::config::AgentConfig;
use cmx_agent::grpc::CacheService;
use cmx_agent::metadata::MetadataClient;
use cmx_agent::metrics::install_metrics_recorder;
use cmx_agent::placement::HashRing;
use cmx_agent::pressure::start_pressure_monitor;
use cmx_agent::remote_index::RemoteIndex;
use cmx_agent::state::AgentState;
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
        AgentConfig::default_config()
    };

    // Initialize tracing.
    let log_format =
        std::env::var("CMX_LOG_FORMAT").unwrap_or_else(|_| config.agent.log_format.clone());

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&config.agent.log_level));

    if log_format == "json" {
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_target(true)
            .json()
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_target(true)
            .init();
    }

    config.resolve_node_id();
    tracing::info!(node_id = %config.agent.node_id, "starting cmx-agent");

    // Start Prometheus metrics exporter.
    let metrics_addr = config
        .metrics
        .listen_addr
        .parse()
        .context("invalid metrics listen address")?;
    install_metrics_recorder(metrics_addr)?;

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

    // Pressure monitor.
    let pressure_level = Arc::new(AtomicU8::new(0));
    let _pressure_handle = start_pressure_monitor(
        allocator.clone(),
        index.clone(),
        pressure_level.clone(),
        config.memory.pressure_warn,
        config.memory.pressure_critical,
        config.memory.pressure_reject,
    );

    // Connect to etcd (if endpoints configured and reachable).
    let listen_addr_str = config.agent.listen_addr.clone();
    let node_id = config.agent.node_id.clone();
    let metadata = match MetadataClient::connect(
        &config.metadata.etcd_endpoints,
        node_id.clone(),
        listen_addr_str.clone(),
        config.metadata.lease_ttl_seconds as i64,
    )
    .await
    {
        Ok(client) => {
            let client = Arc::new(client);
            if let Err(e) = client.register_node().await {
                tracing::warn!(error = %e, "failed to register node in etcd — running standalone");
            } else if let Err(e) = client.start_keepalive().await {
                tracing::warn!(error = %e, "failed to start lease keepalive");
            }
            Some(client)
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to connect to etcd — running standalone");
            None
        }
    };

    // Remote index and hash ring.
    let remote_index = Arc::new(RemoteIndex::new());
    let hash_ring = Arc::new(tokio::sync::RwLock::new(HashRing::new(
        config.placement.vnodes_per_node,
    )));

    // Start etcd watches if connected.
    if let Some(ref meta) = metadata {
        // Add self to hash ring.
        {
            let mut ring = hash_ring.write().await;
            ring.add_node(node_id.clone());
        }

        if let Err(e) = meta
            .start_block_watch(remote_index.clone(), Some(hash_ring.clone()))
            .await
        {
            tracing::warn!(error = %e, "failed to start etcd watches");
        }
    }

    let config = Arc::new(config);

    // Build agent state.
    let agent_state = Arc::new(AgentState {
        allocator,
        index,
        node_id: config.agent.node_id.clone(),
        config: config.clone(),
        metadata: metadata.clone(),
        pressure_level,
        remote_index: Some(remote_index),
        hash_ring: Some(hash_ring),
    });

    // Build gRPC service.
    let cache_service = CacheService::new(agent_state);

    let addr = config
        .agent
        .listen_addr
        .parse()
        .context("invalid listen address")?;

    tracing::info!(%addr, "gRPC server starting");

    // Handle graceful shutdown.
    let metadata_for_shutdown = metadata.clone();
    let shutdown = async move {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install CTRL+C handler");
        tracing::info!("received CTRL+C, shutting down");
        if let Some(meta) = metadata_for_shutdown {
            if let Err(e) = meta.revoke_lease().await {
                tracing::warn!(error = %e, "failed to revoke etcd lease on shutdown");
            }
        }
    };

    Server::builder()
        .add_service(CmxCacheServer::new(cache_service))
        .serve_with_shutdown(addr, shutdown)
        .await
        .context("gRPC server failed")?;

    Ok(())
}
