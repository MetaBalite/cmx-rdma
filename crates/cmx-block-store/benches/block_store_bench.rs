use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use cmx_block_store::{BlockIndex, BlockLocation, BlockAllocator};
use cmx_memory::MemoryPool;

fn hash(n: u32) -> [u8; 16] {
    let mut h = [0u8; 16];
    h[..4].copy_from_slice(&n.to_le_bytes());
    h
}

fn dummy_location(gen: u64) -> BlockLocation {
    BlockLocation {
        node_id: 0,
        block_index: 0,
        offset: 0,
        block_size: 4096,
        generation: gen,
    }
}

fn index_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("block_index");

    // Pre-populate 10K-entry index for read benchmarks
    let index = BlockIndex::new(20_000);
    for i in 0..10_000u32 {
        index.insert(hash(i), vec![dummy_location(i as u64)], i as u64);
    }

    group.bench_function("lookup_hit", |b| {
        let mut i = 0u32;
        b.iter(|| {
            i = (i + 1) % 10_000;
            black_box(index.lookup(&hash(i)));
        });
    });

    group.bench_function("lookup_miss", |b| {
        let mut i = 20_000u32;
        b.iter(|| {
            i += 1;
            black_box(index.lookup(&hash(i)));
        });
    });

    group.bench_function("contains", |b| {
        let mut i = 0u32;
        b.iter(|| {
            i = (i + 1) % 10_000;
            black_box(index.contains(&hash(i)));
        });
    });

    // Insert at 50% capacity (no eviction) - use a fresh index each iteration group
    group.bench_function("insert_no_eviction", |b| {
        let idx = BlockIndex::new(100_000);
        let mut counter = 0u32;
        b.iter(|| {
            counter += 1;
            black_box(idx.insert(hash(counter), vec![dummy_location(counter as u64)], counter as u64));
        });
    });

    // Insert with eviction - index at capacity
    group.bench_function("insert_with_eviction", |b| {
        let capacity = 1_000;
        let idx = BlockIndex::new(capacity);
        for i in 0..capacity as u32 {
            idx.insert(hash(i), vec![dummy_location(i as u64)], i as u64);
        }
        let mut counter = capacity as u32;
        b.iter(|| {
            counter += 1;
            black_box(idx.insert(hash(counter), vec![dummy_location(counter as u64)], counter as u64));
        });
    });

    // Batch lookup - parameterized
    for n in [1, 10, 50, 100, 500] {
        group.bench_with_input(BenchmarkId::new("batch_lookup", n), &n, |b, &n| {
            b.iter(|| {
                for i in 0..n {
                    black_box(index.lookup(&hash(i as u32)));
                }
            });
        });
    }

    group.finish();
}

fn allocator_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("block_allocator");

    for size in [1024usize, 65536, 262144] {
        let label = match size {
            1024 => "1KiB",
            65536 => "64KiB",
            262144 => "256KiB",
            _ => unreachable!(),
        };

        // Block size must be larger than data + header (24 bytes)
        let block_size = size + 1024; // extra space for header
        let num_blocks = 64;
        let pool = MemoryPool::new(num_blocks * block_size, block_size).unwrap();
        let alloc = BlockAllocator::new(pool, 0);
        let data = vec![0xABu8; size];

        group.bench_with_input(BenchmarkId::new("allocate_and_write", label), &data, |b, data| {
            b.iter(|| {
                if let Some((_, handle)) = alloc.allocate_and_write(black_box(data)) {
                    alloc.deallocate(handle); // return block for next iteration
                }
            });
        });
    }

    // read_verified benchmark
    {
        let block_size = 65536 + 1024;
        let pool = MemoryPool::new(4 * block_size, block_size).unwrap();
        let alloc = BlockAllocator::new(pool, 0);
        let data = vec![0xCDu8; 65536];
        let (_, handle) = alloc.allocate_and_write(&data).unwrap();

        group.bench_function("read_verified_64KiB", |b| {
            b.iter(|| {
                black_box(alloc.read_verified(&handle));
            });
        });
    }

    group.finish();
}

criterion_group!(benches, index_benchmarks, allocator_benchmarks);
criterion_main!(benches);
