"""LMCache RemoteConnector implementation backed by cmx-rdma.

This connector implements LMCache's StorageBackendInterface, allowing
both vLLM and SGLang to use cmx-rdma for distributed KV cache sharing
via LMCache's engine abstraction.

Usage:
    # In LMCache config:
    storage_backend: "cmx://agent-host:50051"
"""

from __future__ import annotations

from typing import Optional, Protocol, runtime_checkable

from cmx_client import CmxClient
from cmx_client.hashing import hash_key


@runtime_checkable
class StorageBackendInterface(Protocol):
    """LMCache StorageBackendInterface protocol.

    Defined here as a Protocol so this module works without LMCache installed.
    When LMCache is available, this matches its actual interface.
    """

    def contains(self, key: str) -> bool: ...
    def put(self, key: str, kv_tensors: bytes) -> None: ...
    def get(self, key: str) -> Optional[bytes]: ...
    def close(self) -> None: ...


class CmxRemoteConnector:
    """LMCache-compatible storage backend that delegates to cmx-agent.

    Implements the StorageBackendInterface protocol so it can be used
    as a drop-in replacement for LMCache's built-in storage backends.

    Args:
        url: cmx-agent URL in the form cmx://host:port
        model_id: Model identifier for cache isolation
    """

    def __init__(self, url: str, model_id: str = ""):
        self._client = CmxClient.from_url(url)
        self._model_id = model_id

    def contains(self, key: str) -> bool:
        """Check if a key exists in the cache."""
        prefix_hash = hash_key(key)
        result = self._client.lookup(prefix_hash)
        return result.found

    def put(self, key: str, kv_tensors: bytes) -> None:
        """Store KV tensors under the given key."""
        prefix_hash = hash_key(key)
        success, _ = self._client.store(
            prefix_hash=prefix_hash,
            data=kv_tensors,
            model_id=self._model_id,
        )
        if not success:
            raise RuntimeError(f"Failed to store key: {key}")

    def get(self, key: str) -> Optional[bytes]:
        """Retrieve KV tensors for the given key."""
        prefix_hash = hash_key(key)
        result = self._client.lookup(prefix_hash)
        if not result.found:
            return None
        return self._client.get(prefix_hash)

    def close(self) -> None:
        """Close the connection to cmx-agent."""
        self._client.close()

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()
