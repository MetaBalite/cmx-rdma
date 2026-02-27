use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use std::sync::atomic::AtomicU8;
use std::sync::Arc;

use cmx_agent::config::AgentConfig;
use cmx_agent::grpc::{composite_key, CacheService};
use cmx_agent::state::AgentState;
use cmx_block_store::{BlockAllocator, BlockIndex};
use cmx_memory::MemoryPool;
use cmx_proto::cmx_cache_server::CmxCacheServer;

fn make_test_state() -> Arc<AgentState> {
    // Use a large pool so store benchmarks don't exhaust it during warmup/sampling.
    // Store bench does ~100K+ iterations; each allocates a block without freeing.
    let num_blocks = 256 * 1024; // 256K blocks = 1 GiB at 4 KiB
    let pool = MemoryPool::new(num_blocks * 4096, 4096).unwrap();
    let allocator = Arc::new(BlockAllocator::new(pool, 0));
    let index = Arc::new(BlockIndex::new(num_blocks));
    let config = Arc::new(AgentConfig::default_config());
    let pressure_level = Arc::new(AtomicU8::new(0));

    Arc::new(AgentState {
        allocator,
        index,
        node_id: "bench-node".to_string(),
        config,
        metadata: None,
        pressure_level,
        remote_index: None,
        hash_ring: None,
    })
}

fn composite_key_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("composite_key");

    let prefix_hash: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];

    group.bench_function("with_model_id", |b| {
        b.iter(|| {
            black_box(composite_key(black_box("llama-3-70b"), black_box(&prefix_hash)));
        });
    });

    group.bench_function("empty_model_id", |b| {
        b.iter(|| {
            black_box(composite_key(black_box(""), black_box(&prefix_hash)));
        });
    });

    group.finish();
}

fn grpc_benchmarks(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Start a test server
    let state = make_test_state();
    let service = CacheService::new(state.clone());

    // Bind to random port
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let server_addr = addr;
    rt.spawn(async move {
        tonic::transport::Server::builder()
            .add_service(CmxCacheServer::new(service))
            .serve(server_addr)
            .await
            .unwrap();
    });

    // Wait for server to be ready
    rt.block_on(async {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    });

    // Connect client
    let channel = rt.block_on(async {
        tonic::transport::Channel::from_shared(format!("http://{}", addr))
            .unwrap()
            .connect()
            .await
            .unwrap()
    });
    let mut client = cmx_proto::cmx_cache_client::CmxCacheClient::new(channel);

    let mut group = c.benchmark_group("grpc");

    // Health RPC
    group.bench_function("health", |b| {
        b.to_async(tokio::runtime::Runtime::new().unwrap()).iter(|| {
            let mut client = client.clone();
            async move {
                let resp = client.health(cmx_proto::HealthRequest {}).await.unwrap();
                black_box(resp);
            }
        });
    });

    // Store some data for lookup benchmarks
    rt.block_on(async {
        for i in 0..50u32 {
            let mut prefix_hash = [0u8; 16];
            prefix_hash[..4].copy_from_slice(&i.to_le_bytes());

            let data = vec![0xABu8; 1024];
            let header = cmx_proto::StoreHeader {
                prefix_hash: prefix_hash.to_vec(),
                num_blocks: 1,
                total_size: data.len() as u64,
                model_id: String::new(),
            };
            let requests = vec![
                cmx_proto::StoreRequest {
                    payload: Some(cmx_proto::store_request::Payload::Header(header)),
                },
                cmx_proto::StoreRequest {
                    payload: Some(cmx_proto::store_request::Payload::Data(data)),
                },
            ];
            client.store(tokio_stream::iter(requests)).await.unwrap();
        }
    });

    // Lookup hit
    group.bench_function("lookup_hit", |b| {
        b.to_async(tokio::runtime::Runtime::new().unwrap()).iter(|| {
            let mut client = client.clone();
            async move {
                let prefix_hash = vec![0u8; 16]; // key 0 exists
                let resp = client.lookup(cmx_proto::LookupRequest {
                    prefix_hash,
                    num_blocks: 0,
                    model_id: String::new(),
                }).await.unwrap();
                black_box(resp);
            }
        });
    });

    // Lookup miss
    group.bench_function("lookup_miss", |b| {
        b.to_async(tokio::runtime::Runtime::new().unwrap()).iter(|| {
            let mut client = client.clone();
            async move {
                let mut prefix_hash = vec![0u8; 16];
                prefix_hash[0] = 0xFF; // doesn't exist
                prefix_hash[1] = 0xFF;
                let resp = client.lookup(cmx_proto::LookupRequest {
                    prefix_hash,
                    num_blocks: 0,
                    model_id: String::new(),
                }).await.unwrap();
                black_box(resp);
            }
        });
    });

    // Batch lookup - parameterized
    for n in [1, 10, 50] {
        group.bench_with_input(BenchmarkId::new("batch_lookup", n), &n, |b, &n| {
            b.to_async(tokio::runtime::Runtime::new().unwrap()).iter(|| {
                let mut client = client.clone();
                async move {
                    let hashes: Vec<Vec<u8>> = (0..n).map(|i| {
                        let mut h = vec![0u8; 16];
                        h[..4].copy_from_slice(&(i as u32).to_le_bytes());
                        h
                    }).collect();
                    let resp = client.batch_lookup(cmx_proto::BatchLookupRequest {
                        prefix_hashes: hashes,
                        model_id: String::new(),
                    }).await.unwrap();
                    black_box(resp);
                }
            });
        });
    }

    // Store small (1 KiB)
    group.bench_function("store_1KiB", |b| {
        let data = vec![0xCDu8; 1024];
        b.to_async(tokio::runtime::Runtime::new().unwrap()).iter(|| {
            let mut client = client.clone();
            let data = data.clone();
            async move {
                // Use unique key each time (with dedup via model_id rotation)
                let prefix_hash = vec![0xEE; 16]; // will overwrite same key
                let header = cmx_proto::StoreHeader {
                    prefix_hash: prefix_hash.clone(),
                    num_blocks: 1,
                    total_size: data.len() as u64,
                    model_id: String::new(),
                };
                let requests = vec![
                    cmx_proto::StoreRequest {
                        payload: Some(cmx_proto::store_request::Payload::Header(header)),
                    },
                    cmx_proto::StoreRequest {
                        payload: Some(cmx_proto::store_request::Payload::Data(data)),
                    },
                ];
                let resp = client.store(tokio_stream::iter(requests)).await.unwrap();
                black_box(resp);
            }
        });
    });

    group.finish();
}

criterion_group!(benches, composite_key_benchmarks, grpc_benchmarks);
criterion_main!(benches);
