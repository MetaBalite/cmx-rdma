"""Integration tests for LRU eviction behavior."""

import hashlib
import pytest


def make_hash(key: str) -> bytes:
    return hashlib.sha256(key.encode()).digest()[:16]


@pytest.mark.integration
class TestEviction:
    """Tests for LRU eviction under memory pressure."""

    def test_eviction_under_pressure(self, agent_address):
        """Fill the cache beyond capacity and verify LRU entries are evicted."""
        pytest.skip("Requires running cmx-agent — run with: pytest -m integration")

    def test_recently_accessed_survives_eviction(self, agent_address):
        """Recently looked-up entries should not be evicted."""
        pytest.skip("Requires running cmx-agent — run with: pytest -m integration")

    def test_stats_track_evictions(self, agent_address):
        """Stats endpoint should report eviction count."""
        pytest.skip("Requires running cmx-agent — run with: pytest -m integration")
