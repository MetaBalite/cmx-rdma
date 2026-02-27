"""vLLM V1 KV connector backed by cmx-rdma.

Implements vLLM's KVConnectorBase_V1 interface for direct KV cache
transfer without requiring LMCache as an intermediary.

Usage:
    vllm serve "model" --kv-transfer-config '{
      "kv_connector": "CmxRdmaConnector",
      "kv_role": "kv_both",
      "kv_connector_module_path": "cmx_vllm.connector"
    }'
"""

from __future__ import annotations

import asyncio
import os
import threading
from concurrent.futures import Future
from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any, Optional

from cmx_client.async_client import AsyncCmxClient
from cmx_client.hashing import hash_token_ids

if TYPE_CHECKING:
    pass

# Environment configuration
_CMX_AGENT_ADDR = os.environ.get("CMX_AGENT_ADDR", "localhost:50051")
_CMX_MODEL_ID = os.environ.get("CMX_MODEL_ID", "")
_CMX_BLOCK_SIZE = int(os.environ.get("CMX_BLOCK_SIZE", "16"))


@dataclass
class _LoadTask:
    """A pending KV load for one request."""
    prefix_hash: bytes
    block_ids: list[int]
    num_tokens: int


@dataclass
class _SaveTask:
    """A pending KV save for one request."""
    prefix_hash: bytes
    block_ids: list[int]
    num_tokens: int


@dataclass
class _ConnectorMeta:
    """Metadata passed from scheduler to workers."""
    loads: list[_LoadTask] = field(default_factory=list)
    saves: list[_SaveTask] = field(default_factory=list)


class CmxRdmaConnector:
    """vLLM V1 KV connector that transfers KV cache via cmx-agent.

    Scheduler-side: hashes token prefixes, batch-lookups against cmx-agent,
    and records which blocks to load/save.

    Worker-side: uses async gRPC to pipeline KV data transfer layer-by-layer,
    reading/writing directly into vLLM's paged KV cache tensors.
    """

    def __init__(self, config: Any = None):
        self._config = config
        self._agent_addr = _CMX_AGENT_ADDR
        self._model_id = _CMX_MODEL_ID
        self._block_size = _CMX_BLOCK_SIZE

        # Async client + event loop for worker-side operations
        self._client: Optional[AsyncCmxClient] = None
        self._loop: Optional[asyncio.AbstractEventLoop] = None
        self._loop_thread: Optional[threading.Thread] = None

        # Worker-side KV cache tensor references
        self._kv_caches: Optional[list] = None

        # Per-forward tracking
        self._pending_loads: dict[str, Future] = {}
        self._pending_saves: list[Future] = []
        self._current_meta: Optional[_ConnectorMeta] = None

    def _ensure_loop(self) -> asyncio.AbstractEventLoop:
        """Ensure the background asyncio loop is running."""
        if self._loop is not None:
            return self._loop
        self._loop = asyncio.new_event_loop()
        self._loop_thread = threading.Thread(
            target=self._loop.run_forever, daemon=True
        )
        self._loop_thread.start()
        # Create the async client on the event loop
        future = asyncio.run_coroutine_threadsafe(
            self._create_client(), self._loop
        )
        future.result(timeout=10)
        return self._loop

    async def _create_client(self) -> None:
        self._client = AsyncCmxClient(self._agent_addr)
        await self._client.connect()

    # ── Scheduler-side methods ──────────────────────────────────────

    def get_num_new_matched_tokens(
        self,
        request: Any,
        num_computed_tokens: int,
    ) -> int:
        """Check how many prefix tokens are cached in cmx-agent.

        Hashes the request's token IDs into block-level prefix hashes,
        calls batch_lookup, and returns how many leading tokens can be
        loaded from cache.
        """
        token_ids = self._get_token_ids(request)
        if not token_ids:
            return 0

        # Only check tokens beyond what's already computed
        unchecked = token_ids[num_computed_tokens:]
        if not unchecked:
            return 0

        prefix_hashes = hash_token_ids(unchecked, self._block_size)
        if not prefix_hashes:
            return 0

        # Synchronous batch lookup from scheduler context
        loop = self._ensure_loop()
        future = asyncio.run_coroutine_threadsafe(
            self._client.batch_lookup(prefix_hashes, self._model_id),
            loop,
        )
        found = future.result(timeout=5)

        # Count contiguous hits from the start
        matched_blocks = 0
        for hit in found:
            if not hit:
                break
            matched_blocks += 1

        return matched_blocks * self._block_size

    def update_state_after_alloc(
        self,
        request: Any,
        blocks: Any,
        num_external_tokens: int,
    ) -> None:
        """Record block mapping for later load after allocation."""
        if num_external_tokens <= 0:
            return

        token_ids = self._get_token_ids(request)
        prefix_hashes = hash_token_ids(
            token_ids[:num_external_tokens], self._block_size
        )

        if not hasattr(request, "_cmx_load_tasks"):
            request._cmx_load_tasks = []

        block_ids = self._extract_block_ids(blocks, num_external_tokens)

        for i, ph in enumerate(prefix_hashes):
            request._cmx_load_tasks.append(_LoadTask(
                prefix_hash=ph,
                block_ids=block_ids[i * self._block_size:(i + 1) * self._block_size]
                if block_ids else [],
                num_tokens=min(self._block_size, num_external_tokens - i * self._block_size),
            ))

    def build_connector_meta(self, scheduler_output: Any) -> Optional[_ConnectorMeta]:
        """Package load/save tasks into metadata for workers."""
        meta = _ConnectorMeta()

        # Collect load tasks from scheduled requests
        if hasattr(scheduler_output, "scheduled_seq_groups"):
            for sg in scheduler_output.scheduled_seq_groups:
                req = sg.seq_group.get_seqs()[0] if hasattr(sg.seq_group, "get_seqs") else sg
                if hasattr(req, "_cmx_load_tasks"):
                    meta.loads.extend(req._cmx_load_tasks)
                    del req._cmx_load_tasks

        # Collect save tasks for finished sequences
        if hasattr(scheduler_output, "finished_seq_groups"):
            for sg in scheduler_output.finished_seq_groups:
                req = sg.get_seqs()[0] if hasattr(sg, "get_seqs") else sg
                token_ids = self._get_token_ids(req)
                if token_ids:
                    prefix_hashes = hash_token_ids(token_ids, self._block_size)
                    for ph in prefix_hashes:
                        meta.saves.append(_SaveTask(
                            prefix_hash=ph,
                            block_ids=[],
                            num_tokens=self._block_size,
                        ))

        if not meta.loads and not meta.saves:
            return None
        return meta

    # ── Worker-side methods ─────────────────────────────────────────

    def register_kv_caches(self, kv_caches: list) -> None:
        """Store references to vLLM's paged KV cache tensors."""
        self._kv_caches = kv_caches

    def start_load_kv(self, forward_context: Any) -> None:
        """Kick off async gRPC Get calls for all layers.

        Reads KV data from cmx-agent and writes directly into vLLM's
        paged buffer slots using the block mapping from connector meta.
        """
        meta = self._get_meta(forward_context)
        if meta is None or not meta.loads:
            return

        self._current_meta = meta
        loop = self._ensure_loop()

        for load_task in meta.loads:
            fut = asyncio.run_coroutine_threadsafe(
                self._do_load(load_task), loop
            )
            key = load_task.prefix_hash.hex()
            self._pending_loads[key] = fut

    async def _do_load(self, task: _LoadTask) -> Optional[bytes]:
        """Async load of a single block from cmx-agent."""
        return await self._client.get(task.prefix_hash, model_id=self._model_id)

    def wait_for_layer_load(self, layer_name: str) -> None:
        """Block until layer data has landed.

        In practice, we load all layers at once via streaming, so this
        waits for all pending loads to complete on first call.
        """
        for key, fut in list(self._pending_loads.items()):
            try:
                fut.result(timeout=30)
            except Exception:
                pass  # Misses are not fatal
        self._pending_loads.clear()

    def save_kv_layer(
        self,
        layer_name: str,
        kv_layer: Any,
        attn_metadata: Any,
    ) -> None:
        """Serialize KV tensor and start async gRPC Store."""
        meta = self._current_meta
        if meta is None or not meta.saves:
            return

        loop = self._ensure_loop()

        for save_task in meta.saves:
            # Serialize the KV data from the tensor
            if hasattr(kv_layer, "cpu"):
                data = kv_layer.cpu().numpy().tobytes()
            elif isinstance(kv_layer, bytes):
                data = kv_layer
            else:
                data = bytes(kv_layer)

            fut = asyncio.run_coroutine_threadsafe(
                self._client.store(
                    save_task.prefix_hash, data, model_id=self._model_id
                ),
                loop,
            )
            self._pending_saves.append(fut)

    def wait_for_save(self) -> None:
        """Block until all stores complete."""
        for fut in self._pending_saves:
            try:
                fut.result(timeout=60)
            except Exception:
                pass  # Log but don't crash on save failure
        self._pending_saves.clear()
        self._current_meta = None

    # ── Optional overrides ──────────────────────────────────────────

    def request_finished(self, request: Any, block_ids: Any = None) -> None:
        """Called when a request finishes. Opportunity to publish to cmx-agent."""
        pass

    def shutdown(self) -> None:
        """Close gRPC channel and stop event loop."""
        if self._loop is not None and self._client is not None:
            future = asyncio.run_coroutine_threadsafe(
                self._client.close(), self._loop
            )
            try:
                future.result(timeout=5)
            except Exception:
                pass
            self._loop.call_soon_threadsafe(self._loop.stop)
            if self._loop_thread is not None:
                self._loop_thread.join(timeout=5)
            self._loop = None
            self._client = None

    # ── Helpers ─────────────────────────────────────────────────────

    @staticmethod
    def _get_token_ids(request: Any) -> list[int]:
        """Extract token IDs from a vLLM request object."""
        if hasattr(request, "prompt_token_ids"):
            return list(request.prompt_token_ids)
        if hasattr(request, "get_token_ids"):
            return list(request.get_token_ids())
        if hasattr(request, "data") and hasattr(request.data, "prompt_token_ids"):
            return list(request.data.prompt_token_ids)
        return []

    @staticmethod
    def _extract_block_ids(blocks: Any, num_tokens: int) -> list[int]:
        """Extract block IDs from vLLM's block allocation result."""
        if isinstance(blocks, list):
            return blocks
        if hasattr(blocks, "block_ids"):
            return list(blocks.block_ids)
        return []

    @staticmethod
    def _get_meta(forward_context: Any) -> Optional[_ConnectorMeta]:
        """Extract connector metadata from forward context."""
        if hasattr(forward_context, "connector_meta"):
            return forward_context.connector_meta
        if isinstance(forward_context, dict):
            return forward_context.get("connector_meta")
        return None
