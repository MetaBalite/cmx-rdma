"""CMX cache client -- thin gRPC wrapper around the cmx-agent."""

from __future__ import annotations

import grpc
from dataclasses import dataclass
from typing import Optional
from urllib.parse import urlparse


@dataclass
class BlockLocation:
    """Location of a cached block."""
    node_id: str
    offset: int
    size: int
    generation_id: int


@dataclass
class LookupResult:
    """Result of a cache lookup."""
    found: bool
    blocks: list[BlockLocation]
    is_local: bool


@dataclass
class CacheStats:
    """Cache statistics from the agent."""
    total_blocks: int
    used_blocks: int
    cache_hits: int
    cache_misses: int
    evictions: int
    memory_used_bytes: int
    memory_total_bytes: int


@dataclass
class HealthStatus:
    """Health check response."""
    healthy: bool
    node_id: str
    version: str
    uptime_seconds: int


# Chunk size for streaming RPCs (1MB)
_CHUNK_SIZE = 1024 * 1024


class CmxClient:
    """Client for communicating with a cmx-agent daemon via gRPC.

    Usage:
        client = CmxClient("localhost:50051")
        result = client.lookup(prefix_hash)

        # Or from URL:
        client = CmxClient.from_url("cmx://agent-host:50051")
    """

    def __init__(
        self,
        target: str,
        timeout: float = 10.0,
        tls_cert: str = None,
        tls_key: str = None,
        tls_ca: str = None,
    ):
        """Connect to a cmx-agent.

        Args:
            target: gRPC target (host:port)
            timeout: Default timeout for RPCs in seconds
            tls_cert: Path to PEM client certificate file (for mTLS)
            tls_key: Path to PEM client private key file (for mTLS)
            tls_ca: Path to CA certificate for server verification
        """
        self._target = target
        self._timeout = timeout
        if tls_cert and tls_key:
            with open(tls_cert, 'rb') as f:
                cert = f.read()
            with open(tls_key, 'rb') as f:
                key = f.read()
            ca = None
            if tls_ca:
                with open(tls_ca, 'rb') as f:
                    ca = f.read()
            credentials = grpc.ssl_channel_credentials(
                root_certificates=ca,
                private_key=key,
                certificate_chain=cert,
            )
            self._channel = grpc.secure_channel(target, credentials)
        else:
            self._channel = grpc.insecure_channel(target)

        # Import generated stubs
        try:
            from cmx_client.generated import cmx_pb2, cmx_pb2_grpc
            self._pb2 = cmx_pb2
            self._stub = cmx_pb2_grpc.CmxCacheStub(self._channel)
        except ImportError:
            raise ImportError(
                "Generated protobuf stubs not found. Run:\n"
                "  python -m grpc_tools.protoc -I../../crates/cmx-proto/proto "
                "--python_out=src/cmx_client/generated "
                "--grpc_python_out=src/cmx_client/generated "
                "../../crates/cmx-proto/proto/cmx.proto"
            )

    @classmethod
    def from_url(cls, url: str, **kwargs) -> "CmxClient":
        """Create a client from a cmx:// URL.

        Args:
            url: URL in the form cmx://host:port
        """
        parsed = urlparse(url)
        if parsed.scheme != "cmx":
            raise ValueError(f"Expected cmx:// URL scheme, got {parsed.scheme}://")
        host = parsed.hostname or "localhost"
        port = parsed.port or 50051
        return cls(f"{host}:{port}", **kwargs)

    def lookup(self, prefix_hash: bytes, num_blocks: int = 0) -> LookupResult:
        """Look up cached blocks by prefix hash.

        Args:
            prefix_hash: SHA-256 truncated to 16 bytes
            num_blocks: Expected number of blocks (0 = any)
        """
        request = self._pb2.LookupRequest(
            prefix_hash=prefix_hash,
            num_blocks=num_blocks,
        )
        response = self._stub.Lookup(request, timeout=self._timeout)
        return LookupResult(
            found=response.found,
            blocks=[
                BlockLocation(
                    node_id=b.node_id,
                    offset=b.offset,
                    size=b.size,
                    generation_id=b.generation_id,
                )
                for b in response.blocks
            ],
            is_local=response.is_local,
        )

    def batch_lookup(
        self,
        prefix_hashes: list[bytes],
        model_id: str = "",
    ) -> list[bool]:
        """Check N prefix hashes in one call.

        Returns a list of booleans — found[i] is True if
        prefix_hashes[i] exists in the cache.
        """
        request = self._pb2.BatchLookupRequest(
            prefix_hashes=prefix_hashes,
            model_id=model_id,
        )
        response = self._stub.BatchLookup(request, timeout=self._timeout)
        return list(response.found)

    def store(
        self,
        prefix_hash: bytes,
        data: bytes,
        model_id: str = "",
        num_blocks: int = 1,
    ) -> tuple[bool, int]:
        """Store KV cache data.

        Args:
            prefix_hash: SHA-256 truncated to 16 bytes
            data: Raw KV tensor data
            model_id: Model identifier for cache isolation
            num_blocks: Number of blocks

        Returns:
            Tuple of (success, generation_id)
        """
        def request_iterator():
            # First message: header
            yield self._pb2.StoreRequest(
                header=self._pb2.StoreHeader(
                    prefix_hash=prefix_hash,
                    num_blocks=num_blocks,
                    total_size=len(data),
                    model_id=model_id,
                )
            )
            # Subsequent messages: data chunks
            for i in range(0, len(data), _CHUNK_SIZE):
                yield self._pb2.StoreRequest(data=data[i : i + _CHUNK_SIZE])

        response = self._stub.Store(request_iterator(), timeout=self._timeout)
        return response.success, response.generation_id

    def get(
        self,
        prefix_hash: bytes,
        num_blocks: int = 0,
        source_node_id: str = "",
    ) -> Optional[bytes]:
        """Get KV cache data by prefix hash.

        Returns the raw KV tensor data, or None if not found.
        """
        request = self._pb2.GetRequest(
            prefix_hash=prefix_hash,
            num_blocks=num_blocks,
            source_node_id=source_node_id,
        )
        chunks = []
        for response in self._stub.Get(request, timeout=self._timeout):
            payload = response.WhichOneof("payload")
            if payload == "data":
                chunks.append(response.data)
        if not chunks:
            return None
        return b"".join(chunks)

    def delete(self, prefix_hash: bytes) -> tuple[bool, int]:
        """Delete cached blocks by prefix hash.

        Returns:
            Tuple of (success, blocks_freed)
        """
        request = self._pb2.DeleteRequest(prefix_hash=prefix_hash)
        response = self._stub.Delete(request, timeout=self._timeout)
        return response.success, response.blocks_freed

    def stats(self) -> CacheStats:
        """Get cache statistics."""
        response = self._stub.Stats(
            self._pb2.StatsRequest(), timeout=self._timeout
        )
        return CacheStats(
            total_blocks=response.total_blocks,
            used_blocks=response.used_blocks,
            cache_hits=response.cache_hits,
            cache_misses=response.cache_misses,
            evictions=response.evictions,
            memory_used_bytes=response.memory_used_bytes,
            memory_total_bytes=response.memory_total_bytes,
        )

    def health(self) -> HealthStatus:
        """Check agent health."""
        response = self._stub.Health(
            self._pb2.HealthRequest(), timeout=self._timeout
        )
        return HealthStatus(
            healthy=response.healthy,
            node_id=response.node_id,
            version=response.version,
            uptime_seconds=response.uptime_seconds,
        )

    def close(self):
        """Close the gRPC channel."""
        self._channel.close()

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()
