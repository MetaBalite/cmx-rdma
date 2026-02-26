Software Requirements Document: RDMA-Based CMX (Context Memory Storage)
1. Objective
To implement a distributed, disaggregated KV Cache sharing layer that replicates the functionality of NVIDIA CMX using standard RDMA (Remote Direct Memory Access) protocols. This system enables multiple inference nodes to share "context memory" to reduce Time-to-First-Token (TTFT) for long-context LLM tasks.
2. Hardware & Infrastructure Requirements
The software layer relies on a high-performance network fabric to bypass CPU overhead.
Network Protocol: Must utilize InfiniBand or RoCE v2 (RDMA over Converged Ethernet) to provide the lossless transport required for large KV tensor movement. Use the NVIDIA RoCE Configuration Guide for switch-level setup.
Hardware Interface: Minimum ConnectX-6 or ConnectX-7 SmartNICs. Performance targets require 200Gb/s to 400Gb/s per node.
Memory Access: Support for GPUDirect RDMA is mandatory to allow the NIC to read/write directly to GPU VRAM. Verify compatibility via the NVIDIA GPUDirect RDMA Documentation.
3. Software Component Requirements
3.1 Kernel & Driver Layer
NVIDIA PeerMem: The nvidia-peermem kernel module must be active to facilitate direct peer-to-peer communication between the NIC and the GPU.
OFED Drivers: Installation of the NVIDIA MLNX_OFED stack is required for the ibverbs user-space libraries.
3.2 Metadata & Control Plane
Distributed Registry: Use a high-speed key-value store like Redis or the Ray Global Control Store to map prompt prefix hashes to specific node IP addresses and memory offsets.
Cache Logic: Implement a block-level management system. The vLLM PagedAttention mechanism is the recommended standard for segmenting KV caches into transferable "pages."
3.3 Data Plane (Transfer Logic)
Asynchronous Jobs: Use the NVIDIA DOCA RDMA API to initiate one-sided RDMA_READ or RDMA_WRITE operations. This ensures the inference engine is not blocked during context retrieval.
Memory Pinning: All KV cache buffers must be pinned in physical memory to prevent the OS from swapping them, which would break the RDMA connection.
4. Integration & Reference Libraries
Rather than a "from-scratch" build, the following frameworks provide the necessary RDMA abstractions:
LMCache: An open-source project specifically designed for multi-tier KV caching across RDMA and NVMe.
SGLang: An inference runtime that supports RadixAttention, which is highly compatible with the prefix-sharing goals of CMX.
NIXL (NVIDIA Inference Exchange Library): A specialized library for optimizing tensor movement across complex network topologies.
5. Performance Targets
Latency: Remote KV retrieval must stay below 10ms for a 32k context window.
Throughput: Must sustain >80% of line-rate bandwidth during burst context-loading phases.

6. References

Network & Hardware Infrastructure
NVIDIA RoCE Configuration Guide: docs.nvidia.com
GPUDirect RDMA Overview: docs.nvidia.com
NVIDIA MLNX_OFED Drivers: network.nvidia.com
BlueField-4 CMX Launch Details: nvidianews.nvidia.com
Inference Engines & Caching Logic
vLLM Engine (PagedAttention): docs.vllm.ai
LMCache GitHub Repository: github.com
SGLang Inference Runtime: github.com
TensorRT-LLM Documentation: nvidia.github.io
Developer APIs & Libraries
DOCA RDMA SDK Guide: docs.nvidia.com
NIXL (Inference Exchange Library): docs.nvidia.com
Ray Global Control Store (Metadata): docs.ray.io
NVIDIA Dynamo Optimization: developer.nvidia.com
Architectural Partners (Storage Offload)
VAST Data CMX Integration: www.vastdata.com
Weka Data Management for BF4: www.weka.io
