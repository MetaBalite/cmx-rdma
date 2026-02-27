use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use cmx_memory::MemoryPool;

fn pool_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_pool");

    let block_size = 4096;
    let num_blocks = 1024;
    let pool = MemoryPool::new(num_blocks * block_size, block_size).unwrap();

    group.bench_function("allocate", |b| {
        b.iter(|| {
            if let Some(handle) = pool.allocate() {
                pool.deallocate(handle); // return for next iteration
                black_box(handle);
            }
        });
    });

    // Pre-allocate a handle for deallocate-only bench
    group.bench_function("alloc_dealloc_cycle", |b| {
        b.iter(|| {
            let handle = pool.allocate().unwrap();
            black_box(&handle);
            pool.deallocate(handle);
        });
    });

    group.finish();

    // Contention benchmark with multiple threads
    let mut group = c.benchmark_group("memory_pool_contention");

    for threads in [1, 2, 4, 8] {
        group.bench_with_input(BenchmarkId::new("alloc_dealloc", threads), &threads, |b, &threads| {
            let pool = std::sync::Arc::new(MemoryPool::new(threads * 256 * block_size, block_size).unwrap());
            b.iter(|| {
                let handles: Vec<_> = (0..threads).map(|_| {
                    let pool = pool.clone();
                    std::thread::spawn(move || {
                        for _ in 0..100 {
                            if let Some(handle) = pool.allocate() {
                                black_box(&handle);
                                pool.deallocate(handle);
                            }
                        }
                    })
                }).collect();
                for h in handles {
                    h.join().unwrap();
                }
            });
        });
    }

    group.finish();
}

criterion_group!(benches, pool_benchmarks);
criterion_main!(benches);
