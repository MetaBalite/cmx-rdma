"""Unit tests for CmxRdmaConnector with mocked gRPC.

Tests scheduler-side and worker-side methods of the vLLM V1 connector
without requiring a running cmx-agent.
"""

from __future__ import annotations

import asyncio
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from cmx_client.hashing import hash_token_ids


class TestCmxRdmaConnectorScheduler:
    """Tests for scheduler-side methods."""

    def _make_connector(self):
        """Create a connector with a mocked async client."""
        from cmx_vllm.connector import CmxRdmaConnector

        connector = CmxRdmaConnector.__new__(CmxRdmaConnector)
        connector._config = None
        connector._agent_addr = "localhost:50051"
        connector._model_id = "test-model"
        connector._block_size = 16
        connector._kv_caches = None
        connector._pending_loads = {}
        connector._pending_saves = []
        connector._current_meta = None

        # Set up a real event loop for testing
        connector._loop = asyncio.new_event_loop()
        connector._loop_thread = None

        # Mock the async client
        connector._client = AsyncMock()

        return connector

    def test_get_num_new_matched_tokens_all_hit(self):
        """All prefix blocks are cached -> return full token count."""
        connector = self._make_connector()
        connector._client.batch_lookup = AsyncMock(return_value=[True, True])

        # Start the loop in a thread
        import threading
        t = threading.Thread(target=connector._loop.run_forever, daemon=True)
        t.start()

        try:
            request = SimpleNamespace(prompt_token_ids=list(range(32)))
            result = connector.get_num_new_matched_tokens(request, num_computed_tokens=0)
            assert result == 32  # 2 blocks * 16 tokens
        finally:
            connector._loop.call_soon_threadsafe(connector._loop.stop)
            t.join(timeout=5)

    def test_get_num_new_matched_tokens_partial_hit(self):
        """First block hit, second miss -> return one block of tokens."""
        connector = self._make_connector()
        connector._client.batch_lookup = AsyncMock(return_value=[True, False])

        import threading
        t = threading.Thread(target=connector._loop.run_forever, daemon=True)
        t.start()

        try:
            request = SimpleNamespace(prompt_token_ids=list(range(32)))
            result = connector.get_num_new_matched_tokens(request, num_computed_tokens=0)
            assert result == 16  # Only first block
        finally:
            connector._loop.call_soon_threadsafe(connector._loop.stop)
            t.join(timeout=5)

    def test_get_num_new_matched_tokens_all_miss(self):
        """No blocks cached -> return 0."""
        connector = self._make_connector()
        connector._client.batch_lookup = AsyncMock(return_value=[False, False])

        import threading
        t = threading.Thread(target=connector._loop.run_forever, daemon=True)
        t.start()

        try:
            request = SimpleNamespace(prompt_token_ids=list(range(32)))
            result = connector.get_num_new_matched_tokens(request, num_computed_tokens=0)
            assert result == 0
        finally:
            connector._loop.call_soon_threadsafe(connector._loop.stop)
            t.join(timeout=5)

    def test_get_num_new_matched_tokens_empty_request(self):
        """Empty token list -> return 0."""
        connector = self._make_connector()

        import threading
        t = threading.Thread(target=connector._loop.run_forever, daemon=True)
        t.start()

        try:
            request = SimpleNamespace(prompt_token_ids=[])
            result = connector.get_num_new_matched_tokens(request, num_computed_tokens=0)
            assert result == 0
        finally:
            connector._loop.call_soon_threadsafe(connector._loop.stop)
            t.join(timeout=5)

    def test_update_state_after_alloc(self):
        """Records load tasks on the request object."""
        connector = self._make_connector()

        request = SimpleNamespace(prompt_token_ids=list(range(16)))
        connector.update_state_after_alloc(request, blocks=[0, 1], num_external_tokens=16)
        assert hasattr(request, "_cmx_load_tasks")
        assert len(request._cmx_load_tasks) == 1

    def test_build_connector_meta_empty(self):
        """No load/save tasks -> returns None."""
        connector = self._make_connector()
        scheduler_output = SimpleNamespace()
        result = connector.build_connector_meta(scheduler_output)
        assert result is None


class TestCmxRdmaConnectorWorker:
    """Tests for worker-side methods."""

    def _make_connector(self):
        from cmx_vllm.connector import CmxRdmaConnector

        connector = CmxRdmaConnector.__new__(CmxRdmaConnector)
        connector._config = None
        connector._agent_addr = "localhost:50051"
        connector._model_id = "test-model"
        connector._block_size = 16
        connector._kv_caches = None
        connector._pending_loads = {}
        connector._pending_saves = []
        connector._current_meta = None
        connector._loop = asyncio.new_event_loop()
        connector._loop_thread = None
        connector._client = AsyncMock()
        return connector

    def test_register_kv_caches(self):
        """register_kv_caches stores tensor references."""
        connector = self._make_connector()
        mock_caches = [MagicMock(), MagicMock()]
        connector.register_kv_caches(mock_caches)
        assert connector._kv_caches is mock_caches

    def test_start_load_kv_no_meta(self):
        """start_load_kv with no meta is a no-op."""
        connector = self._make_connector()
        context = SimpleNamespace()  # No connector_meta
        connector.start_load_kv(context)
        assert len(connector._pending_loads) == 0

    def test_wait_for_save_clears_pending(self):
        """wait_for_save clears the pending saves list."""
        connector = self._make_connector()
        connector._pending_saves = []
        connector._current_meta = None
        connector.wait_for_save()
        assert connector._pending_saves == []

    def test_shutdown(self):
        """shutdown closes the client and cleans up."""
        connector = self._make_connector()
        mock_client = connector._client

        import threading
        t = threading.Thread(target=connector._loop.run_forever, daemon=True)
        t.start()

        connector.shutdown()
        t.join(timeout=5)

        # After shutdown, client and loop are cleared
        mock_client.close.assert_awaited()
        assert connector._client is None
        assert connector._loop is None

    def test_request_finished_is_noop(self):
        """request_finished does not raise."""
        connector = self._make_connector()
        connector.request_finished(SimpleNamespace(), block_ids=[1, 2])


class TestTokenIdExtraction:
    """Test the _get_token_ids helper."""

    def test_prompt_token_ids_attr(self):
        from cmx_vllm.connector import CmxRdmaConnector

        request = SimpleNamespace(prompt_token_ids=[1, 2, 3])
        assert CmxRdmaConnector._get_token_ids(request) == [1, 2, 3]

    def test_get_token_ids_method(self):
        from cmx_vllm.connector import CmxRdmaConnector

        request = SimpleNamespace(get_token_ids=lambda: [4, 5, 6])
        assert CmxRdmaConnector._get_token_ids(request) == [4, 5, 6]

    def test_nested_data(self):
        from cmx_vllm.connector import CmxRdmaConnector

        request = SimpleNamespace(data=SimpleNamespace(prompt_token_ids=[7, 8, 9]))
        assert CmxRdmaConnector._get_token_ids(request) == [7, 8, 9]

    def test_no_tokens(self):
        from cmx_vllm.connector import CmxRdmaConnector

        request = SimpleNamespace()
        assert CmxRdmaConnector._get_token_ids(request) == []
