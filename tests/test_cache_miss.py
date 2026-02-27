"""Integration tests for cache miss flow."""

import hashlib

import grpc
import pytest


def make_hash(key: str) -> bytes:
    return hashlib.sha256(key.encode()).digest()[:16]


@pytest.mark.integration
class TestCacheMiss:
    """Tests for the cache miss path."""

    def test_lookup_nonexistent(self, client):
        """Lookup for a key that was never stored returns not found."""
        prefix = make_hash("nonexistent-key-12345")
        result = client.lookup(prefix)
        assert not result.found

    def test_get_nonexistent(self, client):
        """Get for a missing key returns None or raises NOT_FOUND."""
        prefix = make_hash("nonexistent-get-key")
        try:
            result = client.get(prefix)
            assert result is None
        except grpc.RpcError as e:
            assert e.code() == grpc.StatusCode.NOT_FOUND

    def test_delete_nonexistent(self, client):
        """Delete on missing key returns success=false."""
        prefix = make_hash("nonexistent-delete-key")
        success, blocks_freed = client.delete(prefix)
        assert not success
        assert blocks_freed == 0
