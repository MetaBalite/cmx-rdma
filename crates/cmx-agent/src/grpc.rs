//! gRPC service implementation for the CmxCache service.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};

use cmx_block_store::{BlockAllocator, BlockIndex};
use cmx_proto::cmx_cache_server::CmxCache;
use cmx_proto::{
    DeleteRequest, DeleteResponse, GetHeader, GetRequest, GetResponse, HealthRequest,
    HealthResponse, LookupRequest, LookupResponse, StatsRequest, StatsResponse, StoreRequest,
    StoreResponse,
};

/// Shared state for the gRPC service.
pub struct CacheService {
    pub allocator: Arc<BlockAllocator>,
    pub index: Arc<BlockIndex>,
    pub node_id: String,
    pub start_time: Instant,
    pub cache_hits: AtomicU64,
    pub cache_misses: AtomicU64,
    pub evictions: AtomicU64,
}

impl CacheService {
    pub fn new(allocator: Arc<BlockAllocator>, index: Arc<BlockIndex>, node_id: String) -> Self {
        Self {
            allocator,
            index,
            node_id,
            start_time: Instant::now(),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        }
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

#[tonic::async_trait]
impl CmxCache for CacheService {
    async fn lookup(
        &self,
        request: Request<LookupRequest>,
    ) -> Result<Response<LookupResponse>, Status> {
        let req = request.into_inner();
        let prefix_hash = parse_prefix_hash(&req.prefix_hash)?;

        match self.index.lookup(&prefix_hash) {
            Some(entry) => {
                self.cache_hits.fetch_add(1, Ordering::Relaxed);
                let blocks = entry
                    .blocks
                    .iter()
                    .map(|loc| cmx_proto::BlockLocation {
                        node_id: self.node_id.clone(),
                        offset: loc.offset,
                        size: loc.block_size,
                        generation_id: loc.generation,
                    })
                    .collect();

                Ok(Response::new(LookupResponse {
                    found: true,
                    blocks,
                    is_local: true,
                }))
            }
            None => {
                self.cache_misses.fetch_add(1, Ordering::Relaxed);
                Ok(Response::new(LookupResponse {
                    found: false,
                    blocks: vec![],
                    is_local: false,
                }))
            }
        }
    }

    async fn store(
        &self,
        request: Request<Streaming<StoreRequest>>,
    ) -> Result<Response<StoreResponse>, Status> {
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
            .allocator
            .allocate_and_write(&data)
            .ok_or_else(|| Status::resource_exhausted("memory pool exhausted"))?;

        let generation = loc.generation;

        // Update the index. Evicted entries' hashes are returned (blocks freed by LRU).
        let evicted = self
            .index
            .insert(prefix_hash, vec![loc], Self::monotonic_now());
        self.evictions
            .fetch_add(evicted.len() as u64, Ordering::Relaxed);

        tracing::debug!(
            prefix_hash = ?prefix_hash,
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

    async fn get(
        &self,
        request: Request<GetRequest>,
    ) -> Result<Response<Self::GetStream>, Status> {
        let req = request.into_inner();
        let prefix_hash = parse_prefix_hash(&req.prefix_hash)?;

        let entry = self.index.lookup(&prefix_hash).ok_or_else(|| {
            self.cache_misses.fetch_add(1, Ordering::Relaxed);
            Status::not_found("prefix hash not found in cache")
        })?;

        self.cache_hits.fetch_add(1, Ordering::Relaxed);

        let (tx, rx) = mpsc::channel(16);
        let allocator = self.allocator.clone();

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

    async fn delete(
        &self,
        request: Request<DeleteRequest>,
    ) -> Result<Response<DeleteResponse>, Status> {
        let req = request.into_inner();
        let prefix_hash = parse_prefix_hash(&req.prefix_hash)?;

        match self.index.remove(&prefix_hash) {
            Some(entry) => {
                let blocks_freed = entry.blocks.len() as u64;
                for block_loc in &entry.blocks {
                    let handle = cmx_memory::BlockHandle {
                        pool_id: 0,
                        block_index: block_loc.block_index,
                        offset: block_loc.offset as usize,
                        size: block_loc.block_size as usize,
                    };
                    self.allocator.deallocate(handle);
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

    async fn stats(
        &self,
        _request: Request<StatsRequest>,
    ) -> Result<Response<StatsResponse>, Status> {
        let pool_stats = self.allocator.stats();
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

    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        Ok(Response::new(HealthResponse {
            healthy: true,
            node_id: self.node_id.clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_seconds: self.start_time.elapsed().as_secs(),
        }))
    }
}
