"""Multi-node integration tests.

Requires Docker Compose with agent-1 and agent-2 running.
"""

import hashlib

import pytest

try:
    from cmx_client import CmxClient
except ImportError:
    CmxClient = None  # type: ignore[misc,assignment]


def make_prefix_hash(text: str) -> bytes:
    """Create a 16-byte prefix hash from text."""
    return hashlib.sha256(text.encode()).digest()[:16]


@pytest.fixture
def agent1_address() -> str:
    return "localhost:50051"


@pytest.fixture
def agent2_address() -> str:
    return "localhost:50052"


@pytest.mark.integration
@pytest.mark.skipif(CmxClient is None, reason="cmx_client not installed")
class TestMultiNode:
    def test_store_on_agent1_lookup_on_agent2(
        self, agent1_address: str, agent2_address: str
    ) -> None:
        """Store data on agent-1, verify lookup on agent-2 finds it via etcd."""
        pytest.skip("Requires Docker Compose environment with etcd")

        client1 = CmxClient(agent1_address)
        client2 = CmxClient(agent2_address)

        prefix_hash = make_prefix_hash("multi-node-test")
        data = b"cross-node KV cache data" * 100

        # Store on agent-1
        result = client1.store(prefix_hash, data)
        assert result.success

        # Lookup on agent-2 should find it (via etcd remote index)
        import time
        time.sleep(2)  # Allow etcd propagation

        lookup = client2.lookup(prefix_hash)
        assert lookup.found
        assert not lookup.is_local  # Should be found remotely

    def test_health_both_agents(
        self, agent1_address: str, agent2_address: str
    ) -> None:
        """Verify both agents are healthy."""
        pytest.skip("Requires Docker Compose environment")

        client1 = CmxClient(agent1_address)
        client2 = CmxClient(agent2_address)

        health1 = client1.health()
        health2 = client2.health()

        assert health1.healthy
        assert health2.healthy
        assert health1.node_id != health2.node_id

    def test_metrics_endpoint(self) -> None:
        """Verify Prometheus metrics are served."""
        pytest.skip("Requires Docker Compose environment")

        import urllib.request

        resp = urllib.request.urlopen("http://localhost:9091/metrics")
        body = resp.read().decode()

        assert "cmx_cache_hits_total" in body
        assert "cmx_cache_misses_total" in body
        assert "cmx_pool_used_blocks" in body
