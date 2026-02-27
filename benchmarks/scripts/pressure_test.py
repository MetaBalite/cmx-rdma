#!/usr/bin/env python3
"""Memory pressure stress test for cmx-rdma cache.

Fills the cache to 100%+, triggers evictions and rejections,
and monitors stats throughout.

Usage:
    python pressure_test.py --agent localhost:50051
"""

import argparse
import hashlib
import json
import os
import sys
import time


def make_hash(key: str) -> bytes:
    return hashlib.sha256(key.encode()).digest()[:16]


def main():
    parser = argparse.ArgumentParser(description="cmx-rdma memory pressure stress test")
    parser.add_argument("--agent", default="localhost:50051", help="Agent address")
    parser.add_argument("--output", type=str, default=None, help="Output JSON file")
    parser.add_argument("--data-size", type=int, default=4096, help="Data size per store")
    args = parser.parse_args()

    try:
        from cmx_client import CmxClient
    except ImportError:
        print("ERROR: cmx_client not installed.")
        sys.exit(1)

    client = CmxClient(args.agent)

    # Get initial stats
    stats = client.stats()
    total_blocks = stats.total_blocks
    print(f"Pool: {total_blocks} blocks, "
          f"{stats.memory_total_bytes / 1024 / 1024:.1f} MiB total")

    # Target: fill to 120% of capacity to trigger evictions
    target_stores = int(total_blocks * 1.2)
    print(f"Will store {target_stores} entries (120% of {total_blocks} blocks)")

    snapshots = []
    errors = 0
    rejections = 0
    latencies = []

    for i in range(target_stores):
        prefix = make_hash(f"pressure-{i}")
        data = os.urandom(min(args.data_size, 3000))  # ensure fits in block

        start = time.perf_counter()
        try:
            success, gen = client.store(prefix, data)
            elapsed_ms = (time.perf_counter() - start) * 1000
            latencies.append(elapsed_ms)
            if not success:
                rejections += 1
        except Exception as e:
            elapsed_ms = (time.perf_counter() - start) * 1000
            latencies.append(elapsed_ms)
            if "resource_exhausted" in str(e).lower() or "pressure" in str(e).lower():
                rejections += 1
            else:
                errors += 1

        # Snapshot every 100 ops
        if (i + 1) % 100 == 0:
            try:
                s = client.stats()
                snapshot = {
                    "op": i + 1,
                    "used_blocks": s.used_blocks,
                    "total_blocks": s.total_blocks,
                    "evictions": s.evictions,
                    "cache_hits": s.cache_hits,
                    "cache_misses": s.cache_misses,
                    "utilization": round(s.used_blocks / max(s.total_blocks, 1) * 100, 1),
                }
                snapshots.append(snapshot)
                print(f"  [{i+1}/{target_stores}] "
                      f"used={s.used_blocks}/{s.total_blocks} "
                      f"evictions={s.evictions} "
                      f"util={snapshot['utilization']}%")
            except Exception:
                pass

    # Final stats
    final_stats = client.stats()
    print(f"\n--- Pressure test complete ---")
    print(f"  Total stores attempted: {target_stores}")
    print(f"  Rejections (pressure): {rejections}")
    print(f"  Errors: {errors}")
    print(f"  Final evictions: {final_stats.evictions}")
    print(f"  Final used blocks: {final_stats.used_blocks}/{final_stats.total_blocks}")

    if latencies:
        latencies.sort()
        print(f"  Latency p50: {latencies[len(latencies)//2]:.3f} ms")
        print(f"  Latency p99: {latencies[int(len(latencies)*0.99)]:.3f} ms")

    # Verify agent didn't crash
    try:
        health = client.health()
        print(f"  Agent healthy: {health.healthy}")
    except Exception as e:
        print(f"  WARNING: Agent health check failed: {e}")

    result = {
        "total_stores": target_stores,
        "rejections": rejections,
        "errors": errors,
        "final_evictions": final_stats.evictions,
        "final_used_blocks": final_stats.used_blocks,
        "total_blocks": final_stats.total_blocks,
        "latencies": latencies,
        "snapshots": snapshots,
    }

    if args.output:
        with open(args.output, "w") as f:
            json.dump(result, f, indent=2)
        print(f"\nResults written to {args.output}")

    client.close()


if __name__ == "__main__":
    main()
