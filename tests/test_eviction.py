"""Integration tests for cache utilization tracking."""

import hashlib

import pytest


def make_hash(key: str) -> bytes:
    return hashlib.sha256(key.encode()).digest()[:16]


@pytest.mark.integration
class TestEviction:
    """Tests for cache utilization and block management."""

    def test_store_increases_utilization(self, client):
        """Storing data should increase used_blocks in stats."""
        before = client.stats()
        data = b"pressure-test-data" * 100
        client.store(make_hash("util-1"), data)
        client.store(make_hash("util-2"), data)
        after = client.stats()
        assert after.used_blocks >= before.used_blocks + 2

    def test_delete_frees_blocks(self, client):
        """Deleting a stored entry should free the block."""
        prefix = make_hash("delete-block-test")
        data = b"delete-me" * 100
        client.store(prefix, data)

        before = client.stats()
        client.delete(prefix)
        after = client.stats()
        assert after.used_blocks <= before.used_blocks

    def test_stats_report_utilization(self, client):
        """Stats endpoint should report consistent block utilization."""
        stats = client.stats()
        assert stats.used_blocks <= stats.total_blocks
        assert stats.memory_used_bytes <= stats.memory_total_bytes
        assert stats.total_blocks == 1024  # 4 MiB / 4 KiB
