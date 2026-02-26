#!/usr/bin/env python3
"""Analyze benchmark results and compute TTFT improvement.

Usage:
    python analyze_results.py results.json
"""

import json
import statistics
import sys


def analyze(results_file: str):
    """Analyze benchmark results from a JSON file."""
    with open(results_file) as f:
        data = json.load(f)

    print("=== cmx-rdma Benchmark Analysis ===\n")

    if "store_latencies" in data:
        store = data["store_latencies"]
        print(f"Store latencies (n={len(store)}):")
        print(f"  mean:   {statistics.mean(store):.3f} ms")
        print(f"  p99:    {sorted(store)[int(len(store) * 0.99)]:.3f} ms")

    if "hit_latencies" in data and "miss_latencies" in data:
        hits = data["hit_latencies"]
        misses = data["miss_latencies"]

        if hits and misses:
            hit_mean = statistics.mean(hits)
            miss_mean = statistics.mean(misses)
            improvement = ((miss_mean - hit_mean) / miss_mean) * 100

            print(f"\nLookup hit  (n={len(hits)}):  mean = {hit_mean:.3f} ms")
            print(f"Lookup miss (n={len(misses)}): mean = {miss_mean:.3f} ms")
            print(f"\nTTFT improvement: {improvement:.1f}%")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print(f"Usage: {sys.argv[0]} <results.json>")
        sys.exit(1)
    analyze(sys.argv[1])
