"""tools._parallel — parallel execution of independent read-only tool calls.

The provider protocol is parallel: one model turn can carry N tool calls that
share no dependencies. Mainstream harnesses execute the independent subset
concurrently and return results in call order (the provider associates each
result with a call id, so order preservation is required).

Only tools whose execution is thread-safe (read-only, no shared mutable state,
no side effects that invalidate sibling reads) run concurrently. Everything
else falls back to the sequential path, byte-for-byte identical to today.
"""
from __future__ import annotations

import os

from concurrent.futures import Future, ThreadPoolExecutor, as_completed
from threading import RLock
from typing import Callable, Iterable

#: Read-only, side-effect-free tools that may run concurrently within one turn.
#: run_command was historically sequential because it cleared the file cache /
#: live log even for read-only shells; now only destructive shells clear the
#: cache (tools/MainTool/runCommand.py) and the native barrier
#: (llm/native_tools.py) marks run_command non-sequential, so read-only shells
#: can parallelize. edit_file is a writer. browser_act takes
#: screenshots + touches the vision channel — sequential.
#: Only names that exist in the native tool surface (tools.TOOL_MAP) matter;
#: entries for absent tools are inert.
PARALLEL_SAFE = frozenset({
    "read_file",
    "outline",
    "search_code",
    "read_pdf",
    "github_search_code",
    "github_search_repositories",
    "github_get_file_contents",
    "github_list_commits",
    "github_list_issues",
    "github_list_pull_requests",
    "github_view_issue",
    "github_view_pull_request",
    "github_view_workflow_run",
    "web_search",
    "update_plan",
    "delegation_status",
    "files_changed",
})

#: Same names, explicit "always sequential" grouping for the definition flag.
PARALLEL_SAFE_ALIASES = {"exec_command", "github_apply_gate", "github_init_project"}

def _parallel_max_workers() -> int:
    """Single source for tool-pool size — mirrors PAI_MAX_CONCURRENCY.

    Env PAI_MAX_CONCURRENCY defaults to 4 (llm/native_tools.py). Legacy pool
    was hard-coded 6, causing double-queuing (limiter 4 + pool 6). Now both
    share the same value so there is one queue.
    """
    try:
        return max(1, int(os.getenv("PAI_MAX_CONCURRENCY", "4")))
    except Exception:
        return 4

_PARALLEL_MAX_WORKERS = _parallel_max_workers()

_EXECUTOR = ThreadPoolExecutor(max_workers=_PARALLEL_MAX_WORKERS, thread_name_prefix="tools")
_EXECUTOR_LOCK = RLock()

#: Cache the future->(idx, item) mapping so results reindex in call order.
_ORDERED_RESULTS_KEY = "__results_by_idx__"


def is_parallel_safe(tool_name: str) -> bool:
    return tool_name in PARALLEL_SAFE


def parallel_executor() -> ThreadPoolExecutor:
    """Return the shared executor, lazily created on first use."""
    global _EXECUTOR
    if _EXECUTOR is None:
        with _EXECUTOR_LOCK:
            if _EXECUTOR is None:
                _EXECUTOR = ThreadPoolExecutor(
                    max_workers=_PARALLEL_MAX_WORKERS,
                    thread_name_prefix="tools",
                )
    return _EXECUTOR


def run_tool_calls(
    calls: Iterable[dict],
    execute_one: Callable[[dict], str],
    max_workers: int = _PARALLEL_MAX_WORKERS,
) -> list[str]:
    """Execute one turn's tool calls, running the PARALLEL_SAFE subset
    concurrently. Returns results in input order.

    ``calls``: list of ``{"tool": name, "args": {...}, "id": ...}``.
    ``execute_one``: fn(call) -> result string. Runs concurrently; must be
    thread-safe (this module guarantees only safe tools reach the pool).
    """
    calls = list(calls)
    if not calls:
        return []
    safe_idx = [i for i, c in enumerate(calls) if is_parallel_safe(c.get("tool", ""))]
    if not safe_idx:
        return [execute_one(c) for c in calls]

    results: dict[int, str] = {}
    pool = parallel_executor()
    futures: dict[Future, int] = {}
    with _EXECUTOR_LOCK:
        for idx in safe_idx:
            fut = pool.submit(execute_one, calls[idx])
            futures[fut] = idx
    for fut in as_completed(futures):
        idx = futures[fut]
        try:
            results[idx] = fut.result()
        except Exception as exc:
            results[idx] = f"Error executing {calls[idx].get('tool', '?')} in parallel: {exc}"

    # NOTE: do NOT use results.get(i, execute_one(c)) — Python evaluates the
    # default eagerly, which would run execute_one(c) for EVERY call even when
    # the parallel result exists (double execution). Check membership first.
    return [results[i] if i in results else execute_one(c) for i, c in enumerate(calls)]
