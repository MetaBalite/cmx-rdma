# cmx-rdma: Architecture & Implementation Plan

## Context

MetaBalite is building **cmx-rdma** — an open-source alternative to NVIDIA's proprietary CMX, providing high-performance distributed KV cache sharing over RDMA. It enables LLM inference nodes to share "context memory" to slash Time-to-First-Token (TTFT).

**Deployment model**: Offered as a service on MetaBalite's own cluster. Customers consume it via APIs. This means raw performance and API design are the highest priorities.

**Transport-agnostic**: Must work everywhere — InfiniBand, RoCE v2, and TCP. No hardware lock-in. NIXL auto-negotiates the best available transport per node pair.

**Engine support**: Both vLLM and SGLang from day one.

**MVP scope**: Full cluster deployment (not a 2-node demo). Every node participates from Phase 1.

**Key differentiator**: Hardware-agnostic (works without BlueField-4), open-source core, and performance-first design.

---

## 1. System Architecture

### 1.1 Component Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                     CLIENT LAYER (Python SDKs)                      │
│  ┌──────────────┐  ┌──────────────┐  ┌───────────────────────────┐ │
│  │ vLLM         │  │ SGLang       │  │ LMCache RemoteConnector   │ │
│  │ KV Connector │  │ Cache Backend│  │ (StorageBackendInterface) │ │
│  └──────┬───────┘  └──────┬───────┘  └────────────┬──────────────┘ │
│         │ gRPC             │ gRPC                  │ gRPC           │
└─────────┼──────────────────┼───────────────────────┼────────────────┘
          │                  │                       │
          ▼                  ▼                       ▼
┌─────────────────────────────────────────────────────────────────────┐
│                   cmx-agent (Rust, per-node daemon)                  │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │                    gRPC Server (tonic)                        │   │
│  │  Lookup | Store | Get | Delete | Stats | HealthCheck          │   │
│  └──────────────────────┬───────────────────────────────────────┘   │
│                          │                                           │
│  ┌───────────────┐  ┌───┴──────────┐  ┌─────────────────────────┐  │
│  │ Block Store   │  │ Metadata     │  │ Transport Engine        │  │
│  │               │  │ Client       │  │                         │  │
│  │ • Allocator   │  │              │  │ • NIXL backend          │  │
│  │ • Index       │  │ • etcd       │  │ • Mooncake TE backend   │  │
│  │   (prefix →   │  │   client     │  │ • TCP fallback          │  │
│  │    block loc) │  │ • Local      │  │                         │  │
│  │ • LRU evict   │  │   cache +    │  │ Wraps one-sided         │  │
│  │ • CRC32       │  │   watch      │  │ RDMA_READ / RDMA_WRITE  │  │
│  │   integrity   │  │   invalidate │  │                         │  │
│  └───────┬───────┘  └──────────────┘  └────────────┬────────────┘  │
│          │                                          │               │
│  ┌───────┴──────────────────────────────────────────┴────────────┐  │
│  │                    Memory Manager                              │  │
│  │  • Pinned DRAM pool (pre-allocated, no runtime alloc)         │  │
│  │  • GPU VRAM registration (GPUDirect RDMA)                     │  │
│  │  • NUMA-aware allocation (bind to NIC's NUMA node)            │  │
│  │  • Zero-copy paths (NIC ↔ GPU, NIC ↔ pinned DRAM)            │  │
│  └───────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────┘
          │                                          │
          ▼                                          ▼
┌──────────────────┐                    ┌─────────────────────────┐
│      etcd        │                    │    Network Fabric         │
│  (metadata       │                    │  InfiniBand / RoCE v2 /  │
│   registry)      │                    │  TCP (auto-negotiated    │
│                  │                    │   via NIXL agents)       │
└──────────────────┘                    └──────────────────────────┘
```

### 1.2 Data Flow: Cache Hit (hot path — must be <10ms)

```
1. Engine connector → gRPC Lookup(prefix_hash) → cmx-agent
2. cmx-agent checks local block store index         [< 1μs, lock-free hashmap]
3. LOCAL HIT → return block handles directly         [done, < 100μs total]
4. LOCAL MISS → check in-memory metadata cache       [< 1μs]
5. METADATA HIT → {remote_node, offset, gen_id}
6. cmx-agent issues RDMA_READ via NIXL to remote     [one-sided, zero-CPU on remote]
7. Data lands in local pinned DRAM or GPU VRAM        [< 8ms for 32k ctx on 200Gb/s]
8. Store locally (opportunistic caching) + return     [< 10ms total]
```

### 1.3 Data Flow: Cache Miss

```
1. Engine connector → gRPC Lookup(prefix_hash) → NOT FOUND
2. Engine does full prefill (expensive, but unavoidable)
3. Engine connector → gRPC Store(prefix_hash, kv_blocks)
4. cmx-agent allocates blocks from pinned pool        [pre-allocated, O(1)]
5. cmx-agent copies KV data into pinned region        [zero-copy if GPU→NIC path]
6. cmx-agent writes to local block store index
7. cmx-agent publishes metadata to etcd               [async, non-blocking]
8. Future identical prefixes → cache hit
```

### 1.4 Why This Architecture

- **cmx-agent is 100% Rust** — no Python in the hot path. gRPC server (tonic), block store, memory manager, transport wrapper — all Rust. Python only exists in the client SDKs that run inside inference engines.
- **Control plane is embedded in the agent**, not a separate service. Metadata lookups go directly from agent → etcd (or local cache). No extra network hop. The "control plane" is a library inside the agent, not a microservice.
- **One-sided RDMA** means the remote node's CPU is never involved in serving cache reads. The NIC handles it via pre-registered memory regions. This is key to scaling.
- **Pre-allocated memory pools** eliminate runtime allocation jitter. The agent allocates its full memory budget at startup and carves blocks from it.

---

## 2. Technology Choices

### 2.1 Language: Rust Throughout the Service

| Component | Language | Rationale |
|---|---|---|
| cmx-agent (entire daemon) | **Rust** | Zero-cost abstractions, no GC pauses, deterministic latency. Memory safety critical for RDMA buffer management (use-after-free = remote memory corruption). `tokio` for async I/O, `tonic` for gRPC, `pyo3` for optional Python bindings. |
| Client SDKs (connectors) | **Python** | vLLM/SGLang are Python. Connectors are thin gRPC stubs — performance-critical work happens in the Rust agent, not the connector. |
| CLI tool (`cmxctl`) | **Rust** | Single binary, consistent with the agent codebase. Uses `clap` for arg parsing. |
| Kubernetes Operator (Phase 3) | **Go** | Only because K8s controller-runtime is Go-native. Deferred to Phase 3. |

### 2.2 Build vs Integrate

| Capability | Decision | Reasoning |
|---|---|---|
| RDMA transport | **Integrate NIXL** | Abstracts UCX/RDMA/TCP behind a unified API. Handles multi-NIC, topology awareness. 6+ months saved. |
| KV block management | **Build** (core IP) | Block allocator, prefix-hash index, LRU eviction, generation tracking. This IS the product. |
| Metadata registry | **Integrate etcd** | Battle-tested, watch/lease/TTL built-in, already in every K8s cluster. |
| Engine connectors | **Build** (thin) | ~500 lines each. Implement LMCache's `StorageBackendInterface` / vLLM's KV Connector. |
| gRPC framework | **Integrate tonic** | Best-in-class Rust gRPC. HTTP/2 multiplexing, streaming, codegen from proto. |
| Observability | **Integrate prometheus + OTel** | `metrics` crate → Prometheus exporter. `tracing` crate → OTel spans. Standard ecosystem. |
| Memory management | **Build** | NUMA-aware pinned pools with GPU registration. Too performance-critical to delegate. |

---

## 3. Repository Structure

```
cmx-rdma/
├── Cargo.toml                    # Rust workspace root
├── pyproject.toml                # Python workspace (uv)
├── LICENSE                       # Apache 2.0
├── CLAUDE.md
├── CMX_RDMA_SRD.md
├── ARCHITECTURE.md
│
├── crates/                       # Rust (the service)
│   ├── cmx-agent/                # Main daemon
│   ├── cmx-block-store/          # KV block storage engine (core IP)
│   ├── cmx-memory/               # Memory management
│   ├── cmx-transport/            # Transport abstraction
│   └── cmx-proto/                # Protobuf definitions
│
├── python/                       # Python (client SDKs only)
│   ├── cmx-client/               # Core Python client (sync + async)
│   ├── cmx-vllm/                 # Direct vLLM KV connector
│   ├── cmx-sglang/               # Direct SGLang HiCache backend
│   └── cmx-lmcache/              # LMCache connector (optional)
│
├── deploy/                       # Deployment configs
│   ├── docker-compose.yaml
│   ├── Dockerfile.agent
│   ├── k8s/
│   │   └── daemonset.yaml        # K8s DaemonSet + ConfigMap
│   ├── slurm/
│   │   ├── cmx-agent.service     # systemd unit for Slurm nodes
│   │   └── launch_agents.sh      # srun wrapper
│   └── config/
│       └── cmx-agent.toml
│
├── benchmarks/                   # Performance testing
│   ├── scripts/
│   └── workloads/
│
├── tests/                        # Integration tests
└── .github/workflows/            # CI/CD
```

---

## 4. Integration

### 4.1 vLLM (Direct Connector)

```bash
pip install cmx-vllm

vllm serve "meta-llama/Llama-3-8B" --kv-transfer-config '{
  "kv_connector": "CmxRdmaConnector",
  "kv_role": "kv_both",
  "kv_connector_module_path": "cmx_vllm.connector"
}'
```

Set `CMX_AGENT_ADDR` and `CMX_MODEL_ID` environment variables to configure.

### 4.2 SGLang (HiCache Backend)

```bash
pip install cmx-sglang

python -m sglang.launch_server --model "meta-llama/Llama-3-8B" \
  --enable-hicache --hicache-storage-backend cmx \
  --hicache-storage-url cmx://agent:50051
```

### 4.3 LMCache (Indirect, via StorageBackendInterface)

```python
from cmx_lmcache import CmxRemoteConnector

connector = CmxRemoteConnector("cmx://agent:50051", model_id="llama-3-8b")
connector.put("prefix:abc", kv_tensor_bytes)
data = connector.get("prefix:abc")
```

### 4.4 Kubernetes Deployment

```bash
kubectl create namespace cmx
kubectl apply -f deploy/k8s/daemonset.yaml
```

This deploys a DaemonSet (one cmx-agent per node) with host networking,
a ConfigMap for configuration, and gRPC health probes.

### 4.5 Slurm Deployment

```bash
# As a systemd service on each node:
sudo cp deploy/slurm/cmx-agent.service /etc/systemd/system/
sudo systemctl enable --now cmx-agent

# Or via srun across allocated nodes:
salloc -N 4 srun bash deploy/slurm/launch_agents.sh
```

---

## 5. Implementation Phases

### Phase 1: Foundation MVP (Weeks 1-8)
Full cluster of inference nodes (vLLM + SGLang) sharing KV cache over any available transport, with measurable TTFT reduction.

### Phase 2: Production Hardening (Weeks 9-16)
Fault tolerance, cache placement intelligence, NUMA/GPUDirect optimization, observability.

### Phase 3: Enterprise & Multi-Tenant (Weeks 17-28)
Multi-tenancy, mTLS security, replication, Kubernetes operator.

### Phase 4: Scale & Optimization (Weeks 29-40)
Hierarchical storage tiers, speculative prefetch, compression, multi-NIC aggregation.

### Phase 5: Ecosystem (Weeks 41-52)
TensorRT-LLM connector, llm-d integration, management console, documentation.

---

## 6. Risk Mitigation

| Risk | Impact | Mitigation |
|---|---|---|
| NIXL API breaks | High | Wrap behind `Transport` trait in `cmx-transport/traits.rs`. Swap to direct UCX or Mooncake TE without changing upstream code. |
| LMCache interface changes | Medium | Connector is ~500 lines. Pin version. Nightly CI against LMCache main. |
| RDMA driver incompatibility | High | Test against MLNX_OFED 5.x + 23.x. TCP fallback ensures dev/CI works without RDMA. |
| etcd scalability (>200 nodes) | Medium | In-agent cache + watch invalidation reduces reads to ~zero. Replace in Phase 4 if needed. |
| Rust FFI safety with NIXL | High | All `unsafe` isolated in `cmx-transport/nixl.rs`. `miri` in CI. Property-based tests for memory manager. |
