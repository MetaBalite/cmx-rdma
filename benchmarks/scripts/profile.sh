#!/usr/bin/env bash
# Generate flamegraphs from criterion benchmarks.
#
# Prerequisites:
#   cargo install flamegraph
#   (On Linux: install perf via 'sudo apt install linux-tools-generic')
#
# Usage:
#   ./benchmarks/scripts/profile.sh [benchmark_filter]
#
# Examples:
#   ./benchmarks/scripts/profile.sh                    # all benchmarks
#   ./benchmarks/scripts/profile.sh block_index        # only block_index benchmarks

set -euo pipefail

FILTER="${1:-}"
OUTPUT_DIR="target/flamegraphs"
mkdir -p "$OUTPUT_DIR"

# Check for cargo-flamegraph
if ! command -v cargo-flamegraph &> /dev/null; then
    echo "Installing cargo-flamegraph..."
    cargo install flamegraph
fi

# Check for perf
if ! command -v perf &> /dev/null; then
    echo "ERROR: perf not found. Install with: sudo apt install linux-tools-generic"
    exit 1
fi

echo "=== Generating flamegraphs ==="

# Build benchmarks first
cargo bench --all --no-run

# Find benchmark binaries
for bench_bin in target/release/deps/*_bench-*; do
    # Skip .d files
    [[ "$bench_bin" == *.d ]] && continue
    # Skip non-executables
    [[ ! -x "$bench_bin" ]] && continue

    bench_name=$(basename "$bench_bin" | sed 's/-[a-f0-9]*$//')

    if [[ -n "$FILTER" && "$bench_name" != *"$FILTER"* ]]; then
        continue
    fi

    echo "Profiling: $bench_name"

    # Run under perf and generate flamegraph
    cargo flamegraph \
        --bin "$bench_bin" \
        --output "$OUTPUT_DIR/${bench_name}.svg" \
        -- --bench ${FILTER:+"$FILTER"} 2>/dev/null || true
done

echo ""
echo "Flamegraphs saved to $OUTPUT_DIR/"
ls -la "$OUTPUT_DIR/"*.svg 2>/dev/null || echo "No flamegraphs generated."
