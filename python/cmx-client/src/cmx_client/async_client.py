"""Async CMX cache client using grpc.aio for non-blocking operations.

Designed for vLLM's layer-by-layer pipelining where KV transfer must
overlap with model computation.
"""

from __future__ import annotations

import asyncio
from typing import Optional
from urllib.parse import urlparse

import grpc
import grpc.aio


class AsyncCmxClient:
    """Async gRPC client for cmx-agent.

    Thread-safe: can be called from worker threads via
    ``asyncio.run_coroutine_threadsafe``.

    Usage::

        client = AsyncCmxClient("localhost:50051")
        await client.connect()
        found = await client.batch_lookup(hashes, model_id="llama")
        await client.close()
    """

    def __init__(self, target: str, timeout: float = 10.0):
        self._target = target
        self._timeout = timeout
        self._channel: Optional[grpc.aio.Channel] = None
        self._stub = None
        self._pb2 = None

    @classmethod
    def from_url(cls, url: str, **kwargs) -> "AsyncCmxClient":
        """Create a client from a cmx:// URL."""
        parsed = urlparse(url)
        if parsed.scheme != "cmx":
            raise ValueError(f"Expected cmx:// URL scheme, got {parsed.scheme}://")
        host = parsed.hostname or "localhost"
        port = parsed.port or 50051
        return cls(f"{host}:{port}", **kwargs)

    async def connect(self) -> None:
        """Open the async gRPC channel."""
        if self._channel is not None:
            return
        self._channel = grpc.aio.insecure_channel(self._target)
        from cmx_client.generated import cmx_pb2, cmx_pb2_grpc
        self._pb2 = cmx_pb2
        self._stub = cmx_pb2_grpc.CmxCacheStub(self._channel)

    def _ensure_connected(self) -> None:
        if self._stub is None:
            raise RuntimeError("Not connected. Call connect() first.")

    async def batch_lookup(
        self, prefix_hashes: list[bytes], model_id: str = ""
    ) -> list[bool]:
        """Check N prefix hashes in one call.

        Returns a list of booleans — ``found[i]`` is True if
        ``prefix_hashes[i]`` exists in the cache.
        """
        self._ensure_connected()
        request = self._pb2.BatchLookupRequest(
            prefix_hashes=prefix_hashes,
            model_id=model_id,
        )
        response = await self._stub.BatchLookup(request, timeout=self._timeout)
        return list(response.found)

    async def store(
        self,
        prefix_hash: bytes,
        data: bytes,
        model_id: str = "",
        num_blocks: int = 1,
    ) -> tuple[bool, int]:
        """Store KV cache data. Returns (success, generation_id)."""
        self._ensure_connected()
        chunk_size = 1024 * 1024

        async def request_iter():
            yield self._pb2.StoreRequest(
                header=self._pb2.StoreHeader(
                    prefix_hash=prefix_hash,
                    num_blocks=num_blocks,
                    total_size=len(data),
                    model_id=model_id,
                )
            )
            for i in range(0, len(data), chunk_size):
                yield self._pb2.StoreRequest(data=data[i : i + chunk_size])

        response = await self._stub.Store(request_iter(), timeout=self._timeout)
        return response.success, response.generation_id

    async def get(self, prefix_hash: bytes, model_id: str = "") -> Optional[bytes]:
        """Get KV cache data. Returns raw bytes or None."""
        self._ensure_connected()
        request = self._pb2.GetRequest(
            prefix_hash=prefix_hash,
            model_id=model_id,
        )
        chunks = []
        try:
            async for response in self._stub.Get(request, timeout=self._timeout):
                payload = response.WhichOneof("payload")
                if payload == "data":
                    chunks.append(response.data)
        except grpc.aio.AioRpcError as e:
            if e.code() == grpc.StatusCode.NOT_FOUND:
                return None
            raise
        if not chunks:
            return None
        return b"".join(chunks)

    async def lookup(
        self, prefix_hash: bytes, model_id: str = ""
    ) -> tuple[bool, bool]:
        """Single-key lookup. Returns (found, is_local)."""
        self._ensure_connected()
        request = self._pb2.LookupRequest(
            prefix_hash=prefix_hash,
            model_id=model_id,
        )
        response = await self._stub.Lookup(request, timeout=self._timeout)
        return response.found, response.is_local

    async def close(self) -> None:
        """Close the async channel."""
        if self._channel is not None:
            await self._channel.close()
            self._channel = None
            self._stub = None

    async def __aenter__(self):
        await self.connect()
        return self

    async def __aexit__(self, *args):
        await self.close()
