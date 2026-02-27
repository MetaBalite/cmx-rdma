"""Cross-engine hash utilities — single source of truth.

All connectors (vLLM, SGLang, LMCache) import from here so that
a cache stored via one engine is retrievable via another.
"""

from __future__ import annotations

import hashlib
import struct


def hash_token_ids(token_ids: list[int], block_size: int = 16) -> list[bytes]:
    """Hash a token sequence into per-block 16-byte prefix hashes.

    Splits ``token_ids`` into blocks of ``block_size`` tokens, SHA-256
    hashes each block (encoding each token as a little-endian int32),
    and truncates to 16 bytes.

    Args:
        token_ids: Sequence of token IDs.
        block_size: Number of tokens per block.

    Returns:
        List of 16-byte prefix hashes, one per block.
    """
    hashes: list[bytes] = []
    for start in range(0, len(token_ids), block_size):
        block = token_ids[start : start + block_size]
        raw = struct.pack(f"<{len(block)}i", *block)
        digest = hashlib.sha256(raw).digest()[:16]
        hashes.append(digest)
    return hashes


def hash_key(key: str) -> bytes:
    """Hash a string key to a 16-byte prefix hash.

    Used by LMCache and SGLang connectors which operate on string keys.

    Args:
        key: Arbitrary string key.

    Returns:
        16-byte SHA-256 truncated hash.
    """
    return hashlib.sha256(key.encode()).digest()[:16]
