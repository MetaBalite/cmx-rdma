"""Integration tests for cache hit flow."""

import hashlib
import os

import pytest


def make_hash(key: str) -> bytes:
    return hashlib.sha256(key.encode()).digest()[:16]


@pytest.mark.integration
class TestCacheHit:
    """Tests for the cache hit path: store data, then look it up."""

    def test_store_and_lookup(self, client):
        """Store KV data then verify lookup finds it."""
        prefix = make_hash("test-store-lookup")
        data = b"fake-kv-data" * 100
        success, gen = client.store(prefix, data)
        assert success
        result = client.lookup(prefix)
        assert result.found
        assert result.is_local

    def test_store_and_get(self, client):
        """Store KV data then retrieve it and verify content matches."""
        prefix = make_hash("test-store-get")
        data = os.urandom(2048)
        success, gen = client.store(prefix, data)
        assert success
        retrieved = client.get(prefix)
        assert retrieved == data

    def test_repeated_lookup_is_cache_hit(self, client):
        """Multiple lookups should all be hits."""
        prefix = make_hash("test-repeated-lookup")
        data = b"persistent-data" * 50
        client.store(prefix, data)
        for _ in range(5):
            result = client.lookup(prefix)
            assert result.found
