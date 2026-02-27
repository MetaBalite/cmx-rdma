"""Shared test fixtures for cmx-rdma integration tests."""

import hashlib
import os
import signal
import socket
import subprocess
import tempfile
import time

import grpc
import pytest


def _find_free_port() -> int:
    """Find a free TCP port on localhost."""
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def _find_agent_binary() -> str:
    """Locate the cmx-agent binary from cargo build output."""
    repo_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    for profile in ("debug", "release"):
        path = os.path.join(repo_root, "target", profile, "cmx-agent")
        if os.path.isfile(path) and os.access(path, os.X_OK):
            return path
    pytest.skip(
        "cmx-agent binary not found. Run 'cargo build' first."
    )


@pytest.fixture(scope="session")
def agent_address():
    """Default cmx-agent address for tests."""
    return "localhost:50051"


@pytest.fixture(scope="session")
def prefix_hash():
    """A sample 16-byte prefix hash for testing."""
    return hashlib.sha256(b"test-prefix-key").digest()[:16]


@pytest.fixture(scope="session")
def agent_process():
    """Start a cmx-agent subprocess with a small test config.

    Uses a 4 MiB pool with 4 KiB blocks, standalone mode (no etcd),
    and mock transport. Yields (process, grpc_address) and tears down
    on session end.
    """
    binary = _find_agent_binary()
    grpc_port = _find_free_port()
    metrics_port = _find_free_port()
    grpc_addr = f"127.0.0.1:{grpc_port}"
    metrics_addr = f"127.0.0.1:{metrics_port}"

    config_content = f"""\
[agent]
node_id = "test-node"
listen_addr = "{grpc_addr}"
log_level = "debug"

[memory]
total_size = 4194304
block_size = 4096

[metadata]
etcd_endpoints = []

[transport]
backend = "mock"

[metrics]
listen_addr = "{metrics_addr}"
"""

    # Write config to a temp file that persists for the session.
    config_file = tempfile.NamedTemporaryFile(
        mode="w", suffix=".toml", prefix="cmx-test-", delete=False
    )
    config_file.write(config_content)
    config_file.flush()
    config_file.close()

    proc = subprocess.Popen(
        [binary, "--config", config_file.name],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )

    # Wait for gRPC to become ready by polling with a simple channel check.
    deadline = time.monotonic() + 15.0
    ready = False
    while time.monotonic() < deadline:
        try:
            channel = grpc.insecure_channel(grpc_addr)
            # Use gRPC channel connectivity check.
            future = grpc.channel_ready_future(channel)
            future.result(timeout=1.0)
            channel.close()
            ready = True
            break
        except Exception:
            # Check if process died.
            if proc.poll() is not None:
                stdout = proc.stdout.read().decode() if proc.stdout else ""
                stderr = proc.stderr.read().decode() if proc.stderr else ""
                pytest.fail(
                    f"cmx-agent exited with code {proc.returncode}\n"
                    f"stdout: {stdout}\nstderr: {stderr}"
                )
            time.sleep(0.2)

    if not ready:
        proc.terminate()
        proc.wait(timeout=5)
        os.unlink(config_file.name)
        pytest.fail("cmx-agent did not become ready within 15 seconds")

    yield proc, grpc_addr

    # Teardown: send SIGTERM and wait.
    proc.send_signal(signal.SIGTERM)
    try:
        proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait(timeout=5)

    os.unlink(config_file.name)


@pytest.fixture(scope="session")
def client(agent_process):
    """A CmxClient connected to the test agent."""
    from cmx_client import CmxClient

    _, grpc_addr = agent_process
    c = CmxClient(grpc_addr, timeout=10.0)
    yield c
    c.close()
