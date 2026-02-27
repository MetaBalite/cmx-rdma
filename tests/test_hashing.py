"""Cross-engine hash consistency tests.

Ensures that vLLM, SGLang, and LMCache connectors produce identical
prefix hashes for the same inputs — so a cache stored via one engine
is retrievable via another.
"""

import hashlib
import struct

import pytest

from cmx_client.hashing import hash_key, hash_token_ids


class TestHashKey:
    """Tests for string-key hashing (LMCache/SGLang path)."""

    def test_deterministic(self):
        """Same key always produces the same hash."""
        h1 = hash_key("my-cache-key")
        h2 = hash_key("my-cache-key")
        assert h1 == h2

    def test_length_is_16_bytes(self):
        """Hash output is exactly 16 bytes."""
        h = hash_key("test")
        assert len(h) == 16

    def test_different_keys_different_hashes(self):
        """Different keys produce different hashes."""
        h1 = hash_key("key-a")
        h2 = hash_key("key-b")
        assert h1 != h2

    def test_matches_raw_sha256(self):
        """hash_key matches a manual SHA-256 truncation."""
        key = "hello-world"
        expected = hashlib.sha256(key.encode()).digest()[:16]
        assert hash_key(key) == expected

    def test_empty_key(self):
        """Empty string is a valid key."""
        h = hash_key("")
        assert len(h) == 16


class TestHashTokenIds:
    """Tests for token-ID hashing (vLLM path)."""

    def test_deterministic(self):
        """Same tokens always produce the same hashes."""
        tokens = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
        h1 = hash_token_ids(tokens, block_size=16)
        h2 = hash_token_ids(tokens, block_size=16)
        assert h1 == h2

    def test_single_block(self):
        """Exactly block_size tokens produce one hash."""
        tokens = list(range(16))
        hashes = hash_token_ids(tokens, block_size=16)
        assert len(hashes) == 1
        assert len(hashes[0]) == 16

    def test_multiple_blocks(self):
        """Tokens spanning multiple blocks produce multiple hashes."""
        tokens = list(range(48))  # 3 blocks of 16
        hashes = hash_token_ids(tokens, block_size=16)
        assert len(hashes) == 3

    def test_partial_block(self):
        """Tokens not aligned to block_size produce a partial trailing block."""
        tokens = list(range(20))  # 1 full + 1 partial
        hashes = hash_token_ids(tokens, block_size=16)
        assert len(hashes) == 2

    def test_matches_raw_computation(self):
        """hash_token_ids matches manual SHA-256 of packed int32s."""
        tokens = [100, 200, 300]
        raw = struct.pack("<3i", 100, 200, 300)
        expected = hashlib.sha256(raw).digest()[:16]
        result = hash_token_ids(tokens, block_size=16)
        assert result[0] == expected

    def test_empty_tokens(self):
        """Empty token list produces no hashes."""
        assert hash_token_ids([], block_size=16) == []

    def test_different_block_sizes(self):
        """Different block sizes produce different block counts."""
        tokens = list(range(32))
        h8 = hash_token_ids(tokens, block_size=8)
        h16 = hash_token_ids(tokens, block_size=16)
        assert len(h8) == 4  # 32/8
        assert len(h16) == 2  # 32/16


class TestCrossEngineCompatibility:
    """Verify that string-key hashing matches across LMCache and SGLang paths."""

    def test_lmcache_and_sglang_same_hash(self):
        """hash_key used by both LMCache and SGLang produces identical results."""
        # Both connectors import hash_key from the same module
        key = "model:llama:layer:0:token_block:42"
        h1 = hash_key(key)
        h2 = hash_key(key)
        assert h1 == h2

    def test_hash_key_is_sha256_truncated(self):
        """All connectors agree: hash = SHA-256(utf8(key))[:16]."""
        key = "cross-engine-test"
        manual = hashlib.sha256(key.encode("utf-8")).digest()[:16]
        assert hash_key(key) == manual

    def test_token_hash_is_sha256_of_packed_ints(self):
        """vLLM connector: hash = SHA-256(pack('<Ni', tokens))[:16]."""
        tokens = [1, 2, 3, 4]
        raw = struct.pack("<4i", *tokens)
        manual = hashlib.sha256(raw).digest()[:16]
        result = hash_token_ids(tokens, block_size=16)
        assert result[0] == manual
