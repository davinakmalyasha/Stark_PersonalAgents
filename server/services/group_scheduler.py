"""Group project supervisor service — the heartbeat (Phase 4).

``docs/group-projects/02-runtime.md`` §2 and ``05-integration-map.md`` §10.

One thread for every project, ticking on the same 15-second cadence the task
scheduler already uses. **One thread is the point**: the single-writer rule
(``02`` §4) is structural rather than a per-project lock, so parallelism lives
in member execution and never in the scheduler.

Boot does a **full rebuild** of every project's projections before the first
tick — ``recover_all_projects``. That is the strong guarantee (``02`` §5.1):
whatever a crash, a manual edit or a bug left behind does not survive a
restart. Tick phase 0 only replays the tail, which is cheap but narrow.

What is deliberately NOT here
-----------------------------
- **No LLM.** The supervisor never calls a model (RT-01). Members do, through
  ``runner.LocalRunner``.
- **No queue of its own.** It reads the registry every tick, so a project
  created while the server is running is picked up without a restart.
- **No per-project threads.** They leak, they contend, and they destroy the
  ordering guarantee.
"""
from __future__ import annotations

import threading
import time
from collections.abc import Callable
from typing import Any

from agents.group.runner import LocalRunner
from agents.group.supervisor import Supervisor

#: Same cadence as the wake scheduler. One number, two loops.
TICK_SECONDS = 15.0

#: Boot recovery is a full rebuild per project; log if it takes a while.
REBUILD_WARN_SECONDS = 5.0

_started = False
_started_lock = threading.Lock()
_stop = threading.Event()
_thread: threading.Thread | None = None
_supervisor: Supervisor | None = None


def build_supervisor(*, runner: Any = None,
                     context_for: Callable[[str], Any] | None = None,
                     store_for: Callable[[str], Any] | None = None) -> Supervisor:
    """Wire a supervisor against the group registry.

    Imports the registry lazily so this module stays importable (and testable)
    without a project root or a database on disk.
    """
    from server.store import group_registry

    return Supervisor(
        store=None,
        store_for=store_for or group_registry.open_store,
        runner=runner if runner is not None else LocalRunner(),
        context_for=context_for or group_registry.context_for,
        log=lambda msg: print(msg, flush=True),
    )


def supervisor() -> Supervisor:
    """The process-wide supervisor, built on first use."""
    global _supervisor
    with _started_lock:
        if _supervisor is None:
            _supervisor = build_supervisor()
        return _supervisor


# ── boot recovery ───────────────────────────────────────────────────────────


def recover_all() -> int:
    """Rebuild every project's projections from its event log.

    Never trusts the tables. Cheap in absolute terms (dozens to hundreds of
    events) and the only place the full-rebuild guarantee is bought
    (``02`` §5.1).
    """
    from agents.group import recover_all_projects
    from server.store import group_registry

    rebuilt = 0
    for project_id in group_registry.list_project_ids():
        started = time.time()
        try:
            store = group_registry.open_store(project_id)
            recover_all_projects(store, [project_id])
            rebuilt += 1
        except Exception as exc:  # noqa: BLE001 - one bad project must not stop boot
            print(f"[GROUP] recovery failed for {project_id}: {exc}", flush=True)
            continue
        elapsed = time.time() - started
        if elapsed > REBUILD_WARN_SECONDS:
            print(f"[GROUP] recovery for {project_id} took {elapsed:.1f}s", flush=True)
    return rebuilt


# ── the loop ────────────────────────────────────────────────────────────────


def tick_all() -> list[dict]:
    """Tick every known project once. Exposed for tests and manual driving."""
    from server.store import group_registry

    reports = []
    supervisor_ = supervisor()
    for project_id in group_registry.list_project_ids():
        try:
            reports.append(supervisor_.tick(project_id))
        except Exception as exc:  # noqa: BLE001 - RT-03, process-wide
            print(f"[GROUP] tick failed for {project_id}: {exc}", flush=True)
            reports.append({"project": project_id, "error": str(exc)})
    return reports


def start_group_scheduler(tick_seconds: float = TICK_SECONDS) -> bool:
    """Start the heartbeat. Idempotent. Returns True if this call started it."""
    global _started, _thread
    with _started_lock:
        if _started:
            return False
        _started = True
        _stop.clear()

    # Recovery BEFORE the first tick, so a project never ticks against a
    # projection that a crash left wrong.
    try:
        count = recover_all()
        if count:
            print(f"[GROUP] recovered {count} project(s)", flush=True)
    except Exception as exc:  # noqa: BLE001 - boot must not die here
        print(f"[GROUP] boot recovery failed: {exc}", flush=True)

    def loop() -> None:
        while not _stop.is_set():
            try:
                tick_all()
            except Exception as exc:  # noqa: BLE001 - the loop must never die
                print(f"[GROUP] supervisor tick error: {exc}", flush=True)
            _stop.wait(tick_seconds)

    _thread = threading.Thread(target=loop, name="group-supervisor", daemon=True)
    _thread.start()
    return True


def stop_group_scheduler(timeout: float = 5.0) -> None:
    """Stop the heartbeat and drop cached stores."""
    global _started, _thread
    _stop.set()
    thread = _thread
    if thread is not None and thread.is_alive():
        thread.join(timeout=timeout)
    with _started_lock:
        _started = False
        _thread = None
    try:
        from server.store import group_registry
        group_registry.invalidate()
    except Exception:  # noqa: BLE001, S110 - shutdown must not raise
        pass


def is_running() -> bool:
    return _started and _thread is not None and _thread.is_alive()
