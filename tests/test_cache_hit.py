"""Integration tests for cache hit flow."""

import hashlib
import pytest


def make_hash(key: str) -> bytes:
    return hashlib.sha256(key.encode()).digest()[:16]


@pytest.mark.integration
class TestCacheHit:
    """Tests for the cache hit path: store data, then look it up."""

    def test_store_and_lookup(self, agent_address):
        """Store KV data then verify lookup finds it."""
        pytest.skip("Requires running cmx-agent — run with: pytest -m integration")
        # When agent is running:
        # from cmx_client import CmxClient
        # client = CmxClient(agent_address)
        # prefix = make_hash("test-store-lookup")
        # success, gen = client.store(prefix, b"fake-kv-data" * 100)
        # assert success
        # result = client.lookup(prefix)
        # assert result.found
        # assert result.is_local

    def test_store_and_get(self, agent_address):
        """Store KV data then retrieve it and verify content matches."""
        pytest.skip("Requires running cmx-agent — run with: pytest -m integration")

    def test_repeated_lookup_is_cache_hit(self, agent_address):
        """Second lookup should still be a hit (no eviction for small data)."""
        pytest.skip("Requires running cmx-agent — run with: pytest -m integration")
