"""Phase 0 gate assertions 2 and 4 — durability and cache-is-a-cache.

Assertion 2 (``07-phase-0-implementation.md`` section 8): a job survives a
process restart, and one left ``running`` reads back as
``failed(process_restart)``.

Assertion 4: ``_DELEGATIONS`` is a *cache*. Clearing it — which is what a
restart does to it — must not break the read paths; ``status_table()``,
``job_detail()`` and ``get_entry()`` must still answer from the store.

Import note: ``agents/__init__.py`` pulls the whole agent stack, which needs
the real ``tools`` package. Other suites in this directory replace ``tools``
with a stub via ``tests/_util.py::tools_stub()`` and never restore it, so
these tests install a path-only ``agents`` package shim that loads
``agents.delegation`` and ``agents.jobs.store`` without executing
``agents/__init__.py``. It is restored on the way out.
"""
from __future__ import annotations

import contextlib
import importlib
import os
import sys
import tempfile
import types

HERE = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.dirname(HERE)
if REPO_ROOT not in sys.path:
    sys.path.insert(0, REPO_ROOT)

_SUBMODULES = (
    "agents.delegation",
    "agents.jobs",
    "agents.jobs.store",
    "agents.workspace_registry",
)


@contextlib.contextmanager
def _isolated_agents(db_path: str):
    """Load agents.delegation + agents.jobs.store against a private jobs.db."""
    saved_pkg = sys.modules.get("agents")
    saved_sub = {name: sys.modules.get(name) for name in _SUBMODULES}

    pkg = types.ModuleType("agents")
    pkg.__path__ = [os.path.join(REPO_ROOT, "agents")]
    sys.modules["agents"] = pkg

    jobs_store = importlib.import_module("agents.jobs.store")
    delegation = importlib.import_module("agents.delegation")

    old_singleton = jobs_store._STORE
    old_failed = delegation._STORE_FAILED
    store = jobs_store.JobStore(db_path)
    jobs_store._STORE = store
    delegation._STORE_FAILED = False
    try:
        yield store, delegation
    finally:
        delegation._DELEGATIONS.clear()
        delegation._STORE_FAILED = old_failed
        jobs_store._STORE = old_singleton
        store.close()
        for name, module in saved_sub.items():
            if module is None:
                sys.modules.pop(name, None)
            else:
                sys.modules[name] = module
        if saved_pkg is None:
            sys.modules.pop("agents", None)
        else:
            sys.modules["agents"] = saved_pkg


def _entry(job_id: str, status: str, parent: str = "chat-A") -> dict:
    return {
        "id": job_id,
        "parent": parent,
        "agent_key": "developer",
        "agent_display": "Dev",
        "title": f"job {job_id}",
        "task": "do the thing",
        "status": status,
        "workspace": "",
        "chat_id": f"child-{job_id}",
        "started": "2026-10-01T00:00:00",
        "last_event_at": "2026-10-01T00:00:00",
        "result": "",
        "error": "",
        "artifact": "",
        "usage": {},
        "delivered": False,
    }


def test_jobs_survive_a_restart():
    with tempfile.TemporaryDirectory() as tmp:
        db = os.path.join(tmp, "jobs.db")

        with _isolated_agents(db) as (store, delegation):
            entry = _entry("del-restart", "running")
            delegation._DELEGATIONS["del-restart"] = entry
            delegation._persist(entry)
            assert store.get("del-restart") is not None, "row was never written"

        # A second JobStore on the same file is what a process restart looks
        # like to the store: __init__ reconciles.
        with _isolated_agents(db) as (store2, _delegation):
            row = store2.get("del-restart")
            assert row is not None, "job row did not survive the restart"
            assert row["status"] == "failed", (
                f"a job left running must reconcile to failed, got {row['status']!r}"
            )
            assert row["error"] == "process_restart", (
                f"expected error='process_restart', got {row['error']!r}"
            )


def test_cache_is_a_cache():
    with tempfile.TemporaryDirectory() as tmp:
        db = os.path.join(tmp, "jobs.db")
        with _isolated_agents(db) as (store, delegation):
            for job_id, status in (("del-a", "done"), ("del-b", "running")):
                entry = _entry(job_id, status)
                delegation._DELEGATIONS[job_id] = entry
                delegation._persist(entry)
            assert len(store.all_ids()) == 2

            # Clearing the dict is exactly what a restart does to it.
            delegation._DELEGATIONS.clear()

            table = delegation.status_table()
            assert "del-a" in table, f"status_table lost del-a after cache clear:\n{table}"
            assert "del-b" in table, f"status_table lost del-b after cache clear:\n{table}"

            detail = delegation.job_detail("del-a")
            assert "del-a" in detail, f"job_detail lost del-a after cache clear: {detail!r}"

            got = delegation.get_entry("del-b")
            assert got is not None, "get_entry returned None after a cache clear"
            assert got["status"] == "running", got


def test_running_and_undelivered_counts_read_through():
    with tempfile.TemporaryDirectory() as tmp:
        db = os.path.join(tmp, "jobs.db")
        with _isolated_agents(db) as (_store, delegation):
            for job_id, status in (("del-q", "queued"), ("del-r", "running"),
                                   ("del-d", "done"), ("del-o", "done")):
                entry = _entry(job_id, status, parent="chat-X")
                if job_id == "del-o":
                    entry["delivered"] = True
                delegation._DELEGATIONS[job_id] = entry
                delegation._persist(entry)
            delegation._DELEGATIONS.clear()

            assert delegation.running_count("chat-X") == 2, "queued+running should be 2"
            assert delegation.running_count("other-chat") == 0
            assert delegation.undelivered_count("chat-X") == 1, "one settled+undelivered"
