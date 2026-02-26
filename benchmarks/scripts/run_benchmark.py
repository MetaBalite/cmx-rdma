#!/usr/bin/env python3
"""Benchmark harness for cmx-rdma cache performance.

Measures TTFT improvement from cache hits vs cache misses.

Usage:
    python run_benchmark.py --agent localhost:50051 --num-requests 100
"""

import argparse
import hashlib
import os
import statistics
import sys
import time


def make_hash(key: str) -> bytes:
    """Create a 16-byte prefix hash from a string key."""
    return hashlib.sha256(key.encode()).digest()[:16]


def generate_kv_data(size_bytes: int) -> bytes:
    """Generate fake KV cache data of the specified size."""
    return os.urandom(size_bytes)


def run_store_benchmark(client, num_requests: int, data_size: int) -> list[float]:
    """Benchmark store operations. Returns list of latencies in ms."""
    latencies = []
    for i in range(num_requests):
        prefix = make_hash(f"bench-store-{i}")
        data = generate_kv_data(data_size)
        start = time.perf_counter()
        client.store(prefix, data)
        elapsed_ms = (time.perf_counter() - start) * 1000
        latencies.append(elapsed_ms)
    return latencies


def run_lookup_benchmark(client, num_requests: int) -> tuple[list[float], list[float]]:
    """Benchmark lookup operations. Returns (hit_latencies, miss_latencies) in ms."""
    hit_latencies = []
    miss_latencies = []

    for i in range(num_requests):
        # Miss: lookup a key that doesn't exist
        miss_prefix = make_hash(f"bench-miss-{i}")
        start = time.perf_counter()
        result = client.lookup(miss_prefix)
        elapsed_ms = (time.perf_counter() - start) * 1000
        miss_latencies.append(elapsed_ms)

        # Hit: lookup a key that was stored
        hit_prefix = make_hash(f"bench-store-{i}")
        start = time.perf_counter()
        result = client.lookup(hit_prefix)
        elapsed_ms = (time.perf_counter() - start) * 1000
        if result.found:
            hit_latencies.append(elapsed_ms)

    return hit_latencies, miss_latencies


def print_stats(name: str, latencies: list[float]):
    """Print latency statistics."""
    if not latencies:
        print(f"  {name}: no data")
        return
    latencies.sort()
    print(f"  {name}:")
    print(f"    count:  {len(latencies)}")
    print(f"    mean:   {statistics.mean(latencies):.3f} ms")
    print(f"    median: {statistics.median(latencies):.3f} ms")
    print(f"    p95:    {latencies[int(len(latencies) * 0.95)]:.3f} ms")
    print(f"    p99:    {latencies[int(len(latencies) * 0.99)]:.3f} ms")
    print(f"    min:    {min(latencies):.3f} ms")
    print(f"    max:    {max(latencies):.3f} ms")


def main():
    parser = argparse.ArgumentParser(description="cmx-rdma benchmark harness")
    parser.add_argument("--agent", default="localhost:50051", help="Agent address")
    parser.add_argument("--num-requests", type=int, default=100, help="Number of requests")
    parser.add_argument(
        "--data-size", type=int, default=65536, help="KV data size in bytes per request"
    )
    args = parser.parse_args()

    try:
        from cmx_client import CmxClient
    except ImportError:
        print("ERROR: cmx_client not installed. Run: pip install -e python/cmx-client")
        sys.exit(1)

    print(f"Connecting to cmx-agent at {args.agent}...")
    client = CmxClient(args.agent)

    # Check health
    try:
        health = client.health()
        print(f"Agent healthy: {health.healthy}, node: {health.node_id}")
    except Exception as e:
        print(f"ERROR: Cannot connect to agent: {e}")
        sys.exit(1)

    # Run benchmarks
    print(f"\n--- Store benchmark ({args.num_requests} requests, {args.data_size} bytes each) ---")
    store_latencies = run_store_benchmark(client, args.num_requests, args.data_size)
    print_stats("store", store_latencies)

    print(f"\n--- Lookup benchmark ({args.num_requests} requests) ---")
    hit_latencies, miss_latencies = run_lookup_benchmark(client, args.num_requests)
    print_stats("lookup_hit", hit_latencies)
    print_stats("lookup_miss", miss_latencies)

    # Print stats
    stats = client.stats()
    print(f"\n--- Agent stats ---")
    print(f"  total_blocks:  {stats.total_blocks}")
    print(f"  used_blocks:   {stats.used_blocks}")
    print(f"  cache_hits:    {stats.cache_hits}")
    print(f"  cache_misses:  {stats.cache_misses}")
    print(f"  evictions:     {stats.evictions}")
    print(f"  memory_used:   {stats.memory_used_bytes / 1024 / 1024:.1f} MiB")
    print(f"  memory_total:  {stats.memory_total_bytes / 1024 / 1024:.1f} MiB")

    client.close()


if __name__ == "__main__":
    main()
