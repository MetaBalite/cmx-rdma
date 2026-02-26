"""Shared test fixtures for cmx-rdma integration tests."""

import subprocess
import time
import pytest


@pytest.fixture(scope="session")
def agent_address():
    """Default cmx-agent address for tests."""
    return "localhost:50051"


@pytest.fixture(scope="session")
def prefix_hash():
    """A sample 16-byte prefix hash for testing."""
    import hashlib
    return hashlib.sha256(b"test-prefix-key").digest()[:16]
