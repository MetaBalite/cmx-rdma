"""SGLang HiCache storage backend backed by cmx-rdma.

Implements SGLang's HiCache storage interface (exist/get/set) for
direct integration without LMCache.

Usage:
    python -m sglang.launch_server --model "model" \\
      --enable-hicache --hicache-storage-backend cmx \\
      --hicache-storage-url cmx://agent:50051
"""

from __future__ import annotations

from typing import Optional

from cmx_client import CmxClient
from cmx_client.hashing import hash_key


class CmxHiCacheStorage:
    """SGLang HiCache storage backend that delegates to cmx-agent.

    Args:
        url: cmx-agent URL in the form cmx://host:port
        model_id: Model identifier for cache isolation.
    """

    def __init__(self, url: str, model_id: str = ""):
        self._client = CmxClient.from_url(url)
        self._model_id = model_id

    def exist(self, key: str) -> bool:
        """Check if a key exists in the cache."""
        prefix_hash = hash_key(key)
        result = self._client.lookup(prefix_hash)
        return result.found

    def get(self, key: str) -> Optional[bytes]:
        """Retrieve cached data for the given key."""
        prefix_hash = hash_key(key)
        result = self._client.lookup(prefix_hash)
        if not result.found:
            return None
        return self._client.get(prefix_hash)

    def set(self, key: str, value: bytes) -> None:
        """Store data under the given key."""
        prefix_hash = hash_key(key)
        success, _ = self._client.store(
            prefix_hash=prefix_hash,
            data=value,
            model_id=self._model_id,
        )
        if not success:
            raise RuntimeError(f"Failed to store key: {key}")

    def close(self) -> None:
        """Close the connection to cmx-agent."""
        self._client.close()

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()
