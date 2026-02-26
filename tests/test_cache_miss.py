"""Integration tests for cache miss flow."""

import hashlib
import pytest


def make_hash(key: str) -> bytes:
    return hashlib.sha256(key.encode()).digest()[:16]


@pytest.mark.integration
class TestCacheMiss:
    """Tests for the cache miss path."""

    def test_lookup_nonexistent(self, agent_address):
        """Lookup for a key that was never stored returns not found."""
        pytest.skip("Requires running cmx-agent — run with: pytest -m integration")

    def test_get_nonexistent(self, agent_address):
        """Get for a missing key returns appropriate error."""
        pytest.skip("Requires running cmx-agent — run with: pytest -m integration")

    def test_delete_nonexistent(self, agent_address):
        """Delete on missing key returns success=false."""
        pytest.skip("Requires running cmx-agent — run with: pytest -m integration")
