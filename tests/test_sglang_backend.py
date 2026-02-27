"""Unit tests for CmxHiCacheStorage with mocked gRPC.

Tests the SGLang HiCache storage backend without requiring a running cmx-agent.
"""

from __future__ import annotations

from types import SimpleNamespace
from unittest.mock import MagicMock, patch

import pytest

from cmx_client.hashing import hash_key


class TestCmxHiCacheStorage:
    """Tests for the SGLang HiCache storage backend."""

    def _make_storage(self):
        """Create a CmxHiCacheStorage with a mocked CmxClient."""
        from cmx_sglang.backend import CmxHiCacheStorage

        storage = CmxHiCacheStorage.__new__(CmxHiCacheStorage)
        storage._client = MagicMock()
        storage._model_id = "test-model"
        return storage

    def test_exist_returns_true_on_hit(self):
        """exist() returns True when lookup finds the key."""
        storage = self._make_storage()
        storage._client.lookup.return_value = SimpleNamespace(found=True)
        assert storage.exist("my-key") is True
        storage._client.lookup.assert_called_once_with(hash_key("my-key"))

    def test_exist_returns_false_on_miss(self):
        """exist() returns False when lookup doesn't find the key."""
        storage = self._make_storage()
        storage._client.lookup.return_value = SimpleNamespace(found=False)
        assert storage.exist("missing-key") is False

    def test_get_returns_data_on_hit(self):
        """get() returns bytes when lookup finds the key."""
        storage = self._make_storage()
        storage._client.lookup.return_value = SimpleNamespace(found=True)
        storage._client.get.return_value = b"cached-data"

        result = storage.get("my-key")
        assert result == b"cached-data"
        storage._client.get.assert_called_once_with(hash_key("my-key"))

    def test_get_returns_none_on_miss(self):
        """get() returns None when lookup doesn't find the key."""
        storage = self._make_storage()
        storage._client.lookup.return_value = SimpleNamespace(found=False)

        result = storage.get("missing-key")
        assert result is None
        storage._client.get.assert_not_called()

    def test_set_stores_data(self):
        """set() calls store with correct prefix hash and data."""
        storage = self._make_storage()
        storage._client.store.return_value = (True, 1)

        storage.set("my-key", b"tensor-data")
        storage._client.store.assert_called_once_with(
            prefix_hash=hash_key("my-key"),
            data=b"tensor-data",
            model_id="test-model",
        )

    def test_set_raises_on_failure(self):
        """set() raises RuntimeError when store fails."""
        storage = self._make_storage()
        storage._client.store.return_value = (False, 0)

        with pytest.raises(RuntimeError, match="Failed to store key"):
            storage.set("my-key", b"data")

    def test_close_closes_client(self):
        """close() delegates to the underlying client."""
        storage = self._make_storage()
        storage.close()
        storage._client.close.assert_called_once()

    def test_context_manager(self):
        """CmxHiCacheStorage works as a context manager."""
        storage = self._make_storage()
        with storage as s:
            assert s is storage
        storage._client.close.assert_called_once()

    def test_uses_shared_hash_key(self):
        """Verify that the storage uses cmx_client.hashing.hash_key."""
        storage = self._make_storage()
        storage._client.lookup.return_value = SimpleNamespace(found=False)

        key = "test-key-123"
        storage.exist(key)

        # Verify it was called with the output of hash_key
        expected_hash = hash_key(key)
        storage._client.lookup.assert_called_once_with(expected_hash)

    def test_different_keys_produce_different_hashes(self):
        """Different keys should call lookup with different hashes."""
        storage = self._make_storage()
        storage._client.lookup.return_value = SimpleNamespace(found=False)

        storage.exist("key-a")
        call1_hash = storage._client.lookup.call_args_list[0][0][0]

        storage.exist("key-b")
        call2_hash = storage._client.lookup.call_args_list[1][0][0]

        assert call1_hash != call2_hash
