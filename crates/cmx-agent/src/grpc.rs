//! gRPC service implementation for the CmxCache service.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};

use sha2::{Digest, Sha256};

use cmx_block_store::{BlockAllocator, BlockIndex};
use cmx_proto::cmx_cache_server::CmxCache;
use cmx_proto::{
    BatchLookupRequest, BatchLookupResponse, DeleteRequest, DeleteResponse, GetHeader, GetRequest,
    GetResponse, HealthRequest, HealthResponse, LookupRequest, LookupResponse, StatsRequest,
    StatsResponse, StoreRequest, StoreResponse,
};

use crate::pressure::PRESSURE_REJECT;
use crate::state::AgentState;

/// Shared state for the gRPC service.
pub struct CacheService {
    pub state: Arc<AgentState>,
    pub start_time: Instant,
    pub cache_hits: AtomicU64,
    pub cache_misses: AtomicU64,
    pub evictions: AtomicU64,
}

impl CacheService {
    pub fn new(state: Arc<AgentState>) -> Self {
        Self {
            state,
            start_time: Instant::now(),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        }
    }

    fn allocator(&self) -> &Arc<BlockAllocator> {
        &self.state.allocator
    }

    fn index(&self) -> &Arc<BlockIndex> {
        &self.state.index
    }

    fn node_id(&self) -> &str {
        &self.state.node_id
    }

    fn monotonic_now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
}

#[allow(clippy::result_large_err)] // Status is tonic's error type — we must return it
fn parse_prefix_hash(bytes: &[u8]) -> Result<[u8; 16], Status> {
    bytes
        .try_into()
        .map_err(|_| Status::invalid_argument("prefix_hash must be exactly 16 bytes"))
}

/// Build a composite index key from model_id + prefix_hash.
/// Empty model_id = backwards-compatible (returns prefix_hash as-is).
fn composite_key(model_id: &str, prefix_hash: &[u8; 16]) -> [u8; 16] {
    if model_id.is_empty() {
        return *prefix_hash;
    }
    let mut hasher = Sha256::new();
    hasher.update(model_id.as_bytes());
    hasher.update(prefix_hash);
    let digest = hasher.finalize();
    let mut key = [0u8; 16];
    key.copy_from_slice(&digest[..16]);
    key
}

#[tonic::async_trait]
impl CmxCache for CacheService {
    #[tracing::instrument(skip(self, request), fields(method = "lookup"))]
    async fn lookup(
        &self,
        request: Request<LookupRequest>,
    ) -> Result<Response<LookupResponse>, Status> {
        let timer = Instant::now();
        let req = request.into_inner();
        let prefix_hash = parse_prefix_hash(&req.prefix_hash)?;
        let index_key = composite_key(&req.model_id, &prefix_hash);

        match self.index().lookup(&index_key) {
            Some(entry) => {
                self.cache_hits.fetch_add(1, Ordering::Relaxed);
                metrics::counter!("cmx_cache_hits_total").increment(1);
                let blocks = entry
                    .blocks
                    .iter()
                    .map(|loc| cmx_proto::BlockLocation {
                        node_id: self.node_id().to_string(),
                        offset: loc.offset,
                        size: loc.block_size,
                        generation_id: loc.generation,
                    })
                    .collect();

                metrics::histogram!("cmx_get_duration_seconds")
                    .record(timer.elapsed().as_secs_f64());

                Ok(Response::new(LookupResponse {
                    found: true,
                    blocks,
                    is_local: true,
                }))
            }
            None => {
                // Check remote index if available.
                if let Some(remote_index) = &self.state.remote_index {
                    if let Some(entries) = remote_index.lookup(&index_key) {
                        if let Some(first) = entries.first() {
                            self.cache_hits.fetch_add(1, Ordering::Relaxed);
                            metrics::counter!("cmx_cache_hits_total").increment(1);
                            let blocks = vec![cmx_proto::BlockLocation {
                                node_id: first.node_id.clone(),
                                offset: first.offset,
                                size: first.size,
                                generation_id: first.generation,
                            }];

                            metrics::histogram!("cmx_get_duration_seconds")
                                .record(timer.elapsed().as_secs_f64());

                            return Ok(Response::new(LookupResponse {
                                found: true,
                                blocks,
                                is_local: false,
                            }));
                        }
                    }
                }

                // Check hash ring for preferred node suggestion.
                if let Some(ring) = &self.state.hash_ring {
                    let ring = ring.read().await;
                    if let Some(preferred) = ring.preferred_node(&index_key) {
                        if preferred != self.node_id() {
                            tracing::debug!(preferred_node = %preferred, "hash ring suggests remote node");
                        }
                    }
                }

                self.cache_misses.fetch_add(1, Ordering::Relaxed);
                metrics::counter!("cmx_cache_misses_total").increment(1);
                metrics::histogram!("cmx_get_duration_seconds")
                    .record(timer.elapsed().as_secs_f64());

                Ok(Response::new(LookupResponse {
                    found: false,
                    blocks: vec![],
                    is_local: false,
                }))
            }
        }
    }

    #[tracing::instrument(skip(self, request), fields(method = "batch_lookup"))]
    async fn batch_lookup(
        &self,
        request: Request<BatchLookupRequest>,
    ) -> Result<Response<BatchLookupResponse>, Status> {
        let timer = Instant::now();
        let req = request.into_inner();

        let mut found = Vec::with_capacity(req.prefix_hashes.len());
        let mut blocks = Vec::new();

        for raw_hash in &req.prefix_hashes {
            let prefix_hash = parse_prefix_hash(raw_hash)?;
            let index_key = composite_key(&req.model_id, &prefix_hash);

            match self.index().lookup(&index_key) {
                Some(entry) => {
                    self.cache_hits.fetch_add(1, Ordering::Relaxed);
                    metrics::counter!("cmx_cache_hits_total").increment(1);
                    found.push(true);
                    for loc in &entry.blocks {
                        blocks.push(cmx_proto::BlockLocation {
                            node_id: self.node_id().to_string(),
                            offset: loc.offset,
                            size: loc.block_size,
                            generation_id: loc.generation,
                        });
                    }
                }
                None => {
                    self.cache_misses.fetch_add(1, Ordering::Relaxed);
                    metrics::counter!("cmx_cache_misses_total").increment(1);
                    found.push(false);
                }
            }
        }

        metrics::histogram!("cmx_batch_lookup_duration_seconds")
            .record(timer.elapsed().as_secs_f64());

        Ok(Response::new(BatchLookupResponse { found, blocks }))
    }

    #[tracing::instrument(skip(self, request), fields(method = "store"))]
    async fn store(
        &self,
        request: Request<Streaming<StoreRequest>>,
    ) -> Result<Response<StoreResponse>, Status> {
        // Check memory pressure before accepting store.
        let pressure = self
            .state
            .pressure_level
            .load(std::sync::atomic::Ordering::Relaxed);
        if pressure >= PRESSURE_REJECT {
            return Err(Status::resource_exhausted(
                "memory pressure too high — store rejected",
            ));
        }

        let timer = Instant::now();
        let mut stream = request.into_inner();

        // First message must be the header.
        let header = match stream.message().await? {
            Some(msg) => match msg.payload {
                Some(cmx_proto::store_request::Payload::Header(h)) => h,
                _ => return Err(Status::invalid_argument("first message must be a header")),
            },
            None => return Err(Status::invalid_argument("empty stream")),
        };

        let prefix_hash = parse_prefix_hash(&header.prefix_hash)?;
        let index_key = composite_key(&header.model_id, &prefix_hash);

        // Collect all data chunks.
        let mut data = Vec::with_capacity(header.total_size as usize);
        while let Some(msg) = stream.message().await? {
            match msg.payload {
                Some(cmx_proto::store_request::Payload::Data(chunk)) => {
                    data.extend_from_slice(&chunk);
                }
                Some(cmx_proto::store_request::Payload::Header(_)) => {
                    return Err(Status::invalid_argument("unexpected header in stream"));
                }
                None => {}
            }
        }

        // Allocate a block and store data.
        let (loc, _handle) = self
            .allocator()
            .allocate_and_write(&data)
            .ok_or_else(|| Status::resource_exhausted("memory pool exhausted"))?;

        let generation = loc.generation;

        // Update the index. Evicted entries' hashes are returned (blocks freed by LRU).
        let evicted = self
            .index()
            .insert(index_key, vec![loc], Self::monotonic_now());
        let eviction_count = evicted.len() as u64;
        self.evictions.fetch_add(eviction_count, Ordering::Relaxed);
        metrics::counter!("cmx_evictions_total").increment(eviction_count);

        // Update pool metrics.
        let pool_stats = self.allocator().stats();
        metrics::gauge!("cmx_pool_used_blocks").set(pool_stats.used_blocks as f64);
        metrics::gauge!("cmx_pool_total_bytes").set(pool_stats.total_bytes as f64);
        metrics::gauge!("cmx_pool_used_bytes").set(pool_stats.used_bytes as f64);

        // Publish block metadata to etcd (async, non-blocking).
        if let Some(metadata) = &self.state.metadata {
            let metadata = metadata.clone();
            let ph = index_key;
            let offset = loc.offset;
            let size = loc.block_size;
            tokio::spawn(async move {
                if let Err(e) = metadata.publish_block(&ph, offset, size, generation).await {
                    tracing::warn!(error = %e, "failed to publish block to etcd");
                }
            });
        }

        metrics::histogram!("cmx_store_duration_seconds").record(timer.elapsed().as_secs_f64());

        tracing::debug!(
            index_key = ?index_key,
            total_size = data.len(),
            generation,
            "stored KV cache block"
        );

        Ok(Response::new(StoreResponse {
            success: true,
            error_message: String::new(),
            generation_id: generation,
        }))
    }

    type GetStream = ReceiverStream<Result<GetResponse, Status>>;

    #[tracing::instrument(skip(self, request), fields(method = "get"))]
    async fn get(&self, request: Request<GetRequest>) -> Result<Response<Self::GetStream>, Status> {
        let req = request.into_inner();
        let prefix_hash = parse_prefix_hash(&req.prefix_hash)?;
        let index_key = composite_key(&req.model_id, &prefix_hash);

        let entry = self.index().lookup(&index_key).ok_or_else(|| {
            self.cache_misses.fetch_add(1, Ordering::Relaxed);
            metrics::counter!("cmx_cache_misses_total").increment(1);
            Status::not_found("prefix hash not found in cache")
        })?;

        self.cache_hits.fetch_add(1, Ordering::Relaxed);
        metrics::counter!("cmx_cache_hits_total").increment(1);

        let (tx, rx) = mpsc::channel(16);
        let allocator = self.allocator().clone();

        tokio::spawn(async move {
            // Send header first.
            let total_size: u64 = entry.blocks.iter().map(|b| b.block_size).sum();
            let header_msg = GetResponse {
                payload: Some(cmx_proto::get_response::Payload::Header(GetHeader {
                    total_size,
                    num_blocks: entry.blocks.len() as u32,
                    generation_id: entry.blocks.first().map(|b| b.generation).unwrap_or(0),
                })),
            };
            if tx.send(Ok(header_msg)).await.is_err() {
                return;
            }

            // Send data blocks.
            for block_loc in &entry.blocks {
                let handle = cmx_memory::BlockHandle {
                    pool_id: 0, // Local pool — Phase 1 only has one pool
                    block_index: block_loc.block_index,
                    offset: block_loc.offset as usize,
                    size: block_loc.block_size as usize,
                };
                match allocator.read_verified(&handle) {
                    Some(data) => {
                        let msg = GetResponse {
                            payload: Some(cmx_proto::get_response::Payload::Data(data)),
                        };
                        if tx.send(Ok(msg)).await.is_err() {
                            return;
                        }
                    }
                    None => {
                        let _ = tx
                            .send(Err(Status::data_loss("block CRC32C verification failed")))
                            .await;
                        return;
                    }
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    #[tracing::instrument(skip(self, request), fields(method = "delete"))]
    async fn delete(
        &self,
        request: Request<DeleteRequest>,
    ) -> Result<Response<DeleteResponse>, Status> {
        let req = request.into_inner();
        let prefix_hash = parse_prefix_hash(&req.prefix_hash)?;

        match self.index().remove(&prefix_hash) {
            Some(entry) => {
                let blocks_freed = entry.blocks.len() as u64;
                for block_loc in &entry.blocks {
                    let handle = cmx_memory::BlockHandle {
                        pool_id: 0,
                        block_index: block_loc.block_index,
                        offset: block_loc.offset as usize,
                        size: block_loc.block_size as usize,
                    };
                    self.allocator().deallocate(handle);
                }
                Ok(Response::new(DeleteResponse {
                    success: true,
                    blocks_freed,
                }))
            }
            None => Ok(Response::new(DeleteResponse {
                success: false,
                blocks_freed: 0,
            })),
        }
    }

    #[tracing::instrument(skip(self, _request), fields(method = "stats"))]
    async fn stats(
        &self,
        _request: Request<StatsRequest>,
    ) -> Result<Response<StatsResponse>, Status> {
        let pool_stats = self.allocator().stats();
        Ok(Response::new(StatsResponse {
            total_blocks: pool_stats.total_blocks as u64,
            used_blocks: pool_stats.used_blocks as u64,
            cache_hits: self.cache_hits.load(Ordering::Relaxed),
            cache_misses: self.cache_misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
            memory_used_bytes: pool_stats.used_bytes as u64,
            memory_total_bytes: pool_stats.total_bytes as u64,
        }))
    }

    #[tracing::instrument(skip(self, _request), fields(method = "health"))]
    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        Ok(Response::new(HealthResponse {
            healthy: true,
            node_id: self.node_id().to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_seconds: self.start_time.elapsed().as_secs(),
        }))
    }
}
