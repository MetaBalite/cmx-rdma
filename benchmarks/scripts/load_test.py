#!/usr/bin/env python3
"""Concurrent load test for cmx-rdma cache.

Usage:
    python load_test.py --agent localhost:50051 --clients 4 --duration 10
"""

import argparse
import hashlib
import json
import os
import statistics
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor


def make_hash(key: str) -> bytes:
    return hashlib.sha256(key.encode()).digest()[:16]


def worker(agent_addr: str, worker_id: int, duration: float, store_ratio: float,
           data_size: int, results: dict, stop_event: threading.Event):
    """Single worker thread running mixed store/lookup operations."""
    from cmx_client import CmxClient

    client = CmxClient(agent_addr)
    latencies = []
    errors = 0
    ops = 0
    stored_keys = []

    end_time = time.monotonic() + duration
    i = 0

    while time.monotonic() < end_time and not stop_event.is_set():
        try:
            # Decide operation based on ratio
            if (i % 100) < int(store_ratio * 100):
                # Store
                key = f"load-w{worker_id}-{i}"
                prefix = make_hash(key)
                data = os.urandom(data_size)
                start = time.perf_counter()
                client.store(prefix, data)
                elapsed_ms = (time.perf_counter() - start) * 1000
                stored_keys.append(prefix)
            else:
                # Lookup
                if stored_keys:
                    prefix = stored_keys[i % len(stored_keys)] if stored_keys else make_hash(f"miss-{i}")
                else:
                    prefix = make_hash(f"miss-{i}")
                start = time.perf_counter()
                client.lookup(prefix)
                elapsed_ms = (time.perf_counter() - start) * 1000

            latencies.append(elapsed_ms)
            ops += 1
        except Exception:
            errors += 1
        i += 1

    client.close()

    results[worker_id] = {
        "ops": ops,
        "errors": errors,
        "latencies": latencies,
    }


def main():
    parser = argparse.ArgumentParser(description="cmx-rdma concurrent load test")
    parser.add_argument("--agent", default="localhost:50051", help="Agent address")
    parser.add_argument("--clients", type=int, default=4, help="Number of concurrent clients")
    parser.add_argument("--duration", type=float, default=10.0, help="Test duration in seconds")
    parser.add_argument("--store-ratio", type=float, default=0.2, help="Fraction of ops that are stores (0.0-1.0)")
    parser.add_argument("--data-size", type=int, default=4096, help="Data size for store ops")
    parser.add_argument("--output", type=str, default=None, help="Output JSON file")
    args = parser.parse_args()

    print(f"Load test: {args.clients} clients, {args.duration}s, "
          f"{args.store_ratio:.0%} store / {1-args.store_ratio:.0%} lookup")

    results = {}
    stop_event = threading.Event()

    with ThreadPoolExecutor(max_workers=args.clients) as executor:
        futures = []
        for i in range(args.clients):
            f = executor.submit(worker, args.agent, i, args.duration,
                              args.store_ratio, args.data_size, results, stop_event)
            futures.append(f)

        for f in futures:
            f.result()

    # Aggregate results
    total_ops = sum(r["ops"] for r in results.values())
    total_errors = sum(r["errors"] for r in results.values())
    all_latencies = []
    for r in results.values():
        all_latencies.extend(r["latencies"])

    all_latencies.sort()

    elapsed = args.duration
    ops_per_sec = total_ops / elapsed if elapsed > 0 else 0

    summary = {
        "clients": args.clients,
        "duration_seconds": elapsed,
        "total_ops": total_ops,
        "total_errors": total_errors,
        "ops_per_second": round(ops_per_sec, 1),
        "error_rate": round(total_errors / max(total_ops + total_errors, 1) * 100, 2),
    }

    if all_latencies:
        summary["p50_ms"] = round(all_latencies[len(all_latencies) // 2], 3)
        summary["p95_ms"] = round(all_latencies[int(len(all_latencies) * 0.95)], 3)
        summary["p99_ms"] = round(all_latencies[int(len(all_latencies) * 0.99)], 3)
        summary["mean_ms"] = round(statistics.mean(all_latencies), 3)

    print(f"\n--- Results ---")
    for k, v in summary.items():
        print(f"  {k}: {v}")

    if args.output:
        with open(args.output, "w") as f:
            json.dump(summary, f, indent=2)
        print(f"\nResults written to {args.output}")


if __name__ == "__main__":
    main()
