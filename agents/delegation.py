"""
Delegation runtime — parallel fan-out from the Orchestrator to workspace agents.

One ``delegate(...)`` tool call fans out N items through the pydantic-graph
map+join path (parallel). Every item routes through the CANONICAL node runner
(developer_node / general_node / house_node) — never the degraded generic
subagent loop.

Isolation per item:
  * own visible chat (created here, or continued when ``chat_id`` is given)
  * own StreamBus target via ``llm.llm.stream_chat_override`` so child deltas
    never interleave into the parent's SSE stream
  * final reply mirrored into the child chat (user task + agent answer)

Completion delivery: each finished item posts a compact ``[DELEGATION RESULT]``
block into the PARENT's follow-up inbox. The orchestrator parks on that inbox
instead of exiting while children are still running.

Thread-safe. Never raises past the tool boundary (error strings instead).
"""

from __future__ import annotations

import json
import os
import threading
import uuid
from datetime import datetime

from .workspace_registry import delegatable_specs as _delegatable_specs

#: Canonical delegatable agents (workspace tier), derived from the agent
#: registry (agents/workspace_registry.py). Specialists (UI/DevOps) stay
#: reachable through Dev's call_subagent, not through this runtime.
_AGENT_SPECS = {(s.delegation_key or s.key): s for s in _delegatable_specs()}

DELEGATABLE_AGENTS = tuple(_AGENT_SPECS)

_AGENT_DISPLAY = {k: s.display for k, s in _AGENT_SPECS.items()}

_LOCK = threading.RLock()

#: del_id -> entry dict. Entries survive until process exit (small).
_DELEGATIONS: dict[str, dict] = {}

# ── Concurrency slots (bounded fan-out) ──────────────────────────────────────
#: A flood guard: at most ORCH_MAX_PARALLEL jobs execute at once (default 6);
#: the rest wait as status="queued" and start as slots free. Mirrors Anthropic's
#: "spawning 50 subagents for a simple query" failure mode.
_SLOT_COND = threading.Condition(_LOCK)
_ACTIVE_SLOTS = 0


def _max_parallel() -> int:
    try:
        return max(1, int(os.environ.get("ORCH_MAX_PARALLEL", "6")))
    except Exception:
        return 6


def _job_timeout_default() -> float:
    """Default per-job deadline in seconds (ORCH_JOB_TIMEOUT; 0 disables)."""
    try:
        return max(0.0, float(os.environ.get("ORCH_JOB_TIMEOUT", "3600")))
    except Exception:
        return 3600.0


def _acquire_slot() -> None:
    global _ACTIVE_SLOTS
    with _SLOT_COND:
        while _ACTIVE_SLOTS >= _max_parallel():
            _SLOT_COND.wait(timeout=1.0)
        _ACTIVE_SLOTS += 1


def _release_slot() -> None:
    global _ACTIVE_SLOTS
    with _SLOT_COND:
        _ACTIVE_SLOTS = max(0, _ACTIVE_SLOTS - 1)
        _SLOT_COND.notify_all()


# ── Job artifacts (full report on disk; summary in the wake block) ───────────


def _jobs_dir() -> str:
    """Where full job reports live (created on demand)."""
    harness = os.environ.get("HARNESS_ROOT") or os.path.dirname(
        os.path.dirname(os.path.abspath(__file__))
    )
    d = os.path.join(harness, ".agent-memory", "jobs")
    try:
        os.makedirs(d, exist_ok=True)
    except Exception:
        pass
    return d


#: Wake-block summary cap (the artifact keeps the full report).
_WAKE_SUMMARY_CAP = 1200
#: Failed/cancelled error text cap in the wake block.
_WAKE_ERROR_CAP = 2000


def _summarize_result(text: str, cap: int = _WAKE_SUMMARY_CAP) -> str:
    """Bounded head summary for the wake block (full text stays in the artifact)."""
    t = str(text or "").strip()
    if len(t) <= cap:
        return t
    return t[:cap].rstrip() + (
        f"\n[... {len(t) - cap:,} more chars — full report in the artifact]"
    )


def _write_job_artifact(snapshot: dict) -> str:
    """Persist the full job report under .agent-memory/jobs/<del_id>.md."""
    del_id = str(snapshot.get("id") or "job")
    path = os.path.join(_jobs_dir(), f"{del_id}.md")
    try:
        lines = [
            f"# Job {del_id} — {snapshot.get('title', '')}",
            "",
            f"- agent: {snapshot.get('agent_display', '')}",
            f"- status: {snapshot.get('status', '')}",
            f"- chat: {snapshot.get('workspace', '')}/{snapshot.get('chat_id', '')}",
            f"- started: {snapshot.get('started', '')}",
            f"- duration_s: {snapshot.get('duration_s', '')}",
        ]
        if snapshot.get("mode") == "resume":
            lines.append(f"- project: {snapshot.get('project', '')}")
        usage = snapshot.get("usage") or {}
        if usage:
            lines.append(
                f"- tokens: in={usage.get('total_input_tokens', 0)} "
                f"out={usage.get('total_output_tokens', 0)} "
                f"cost={usage.get('total_cost', 0)}"
            )
        if snapshot.get("outcome"):
            lines += ["", "## Outcome (from Sir's brief)", str(snapshot["outcome"])]
        if snapshot.get("acceptance"):
            lines += ["", "## Acceptance (from Sir's brief)", str(snapshot["acceptance"])]
        lines += ["", "## Task", str(snapshot.get("task") or "(none)")]
        result = str(snapshot.get("result") or "")
        error = str(snapshot.get("error") or "")
        lines += ["", "## Report", result or "(no output)"]
        if error and error not in result:
            lines += ["", "## Error", error]
        with open(path, "w", encoding="utf-8") as f:
            f.write("\n".join(lines) + "\n")
        return path
    except Exception as e:
        _log(f"[DELEGATE] Artifact write skipped for {del_id}: {e}")
        return ""


# ── Helpers ──────────────────────────────────────────────────────────────────


def _norm_agent(name: str) -> str:
    return str(name or "").strip().lower()


def _log(msg: str) -> None:
    try:
        print(msg, flush=True)
    except Exception:
        pass
    try:
        from sync.state_sync import safe_append_live_log
        safe_append_live_log(msg)
    except Exception:
        pass


def _default_workspace(agent_key: str, fallback_ws: str) -> str:
    """Where this agent's chats live when the item doesn't specify one."""
    spec = _AGENT_SPECS.get(agent_key)
    if spec is not None and spec.delegatable_chat != "_general":
        return fallback_ws or ""
    # General / House conversations belong to the General workspace.
    try:
        import workspace_manager as wm
        return wm.GENERAL_WORKSPACE_ID
    except Exception:
        return fallback_ws or ""


def _resolve_workspace(item: dict, agent_key: str, fallback_ws: str) -> str:
    ws = str(item.get("workspace") or "").strip()
    if ws:
        return ws
    return _default_workspace(agent_key, fallback_ws)


def _ensure_workspace_dir(ws: str) -> str:
    """Ensure the delegation workspace folder exists (create-before-delegate).

    If ``ws`` is a real path (e.g. ``D:\\MyProject\\TodoList``) that doesn't
    exist yet, create it so the child Dev chat has a real home instead of a
    phantom path. Returns the (possibly created) workspace path.
    """
    if not ws:
        return ws
    try:
        if os.path.isabs(ws) and not os.path.isdir(ws):
            os.makedirs(ws, exist_ok=True)
            _log(f"[DELEGATE] Created missing workspace folder: {ws}")
    except Exception as e:
        _log(f"[DELEGATE] Could not create workspace '{ws}': {e}")
    return ws


# ── Auto job tracker (orchestrator plan built from real jobs) ────────────────
#
# The orchestrator does NOT write its own plan: the delegation runtime rebuilds
# the orchestrator-scope plan from the live job entries on every status change.
# Items look like "del-a1b2 · Dev · Build a snake game — running" and are
# re-injected into the orchestrator at run start and after compaction.

_PLAN_SYNC_LOCK = threading.Lock()


def _elapsed_short(secs) -> str:
    try:
        secs = int(secs)
    except Exception:
        return "?"
    if secs < 60:
        return f"{secs}s"
    mins, rem = divmod(secs, 60)
    if mins < 60:
        return f"{mins}m{rem:02d}s"
    hrs, rem = divmod(mins, 60)
    return f"{hrs}h{rem:02d}m"


def _job_step_text(e: dict) -> str:
    """One plan line: id · agent · title — status (duration/reason)."""
    status = str(e.get("status") or "?")
    text = f"{e.get('id', '')} · {e.get('agent_display', '')} · {e.get('title', '')} — {status}"
    if status in ("failed", "cancelled"):
        reason = str(e.get("error") or "").strip().replace("\n", " ")[:100]
        if reason:
            text += f": {reason}"
    if status in ("done", "failed", "cancelled") and e.get("duration_s") is not None:
        text += f" ({_elapsed_short(e.get('duration_s'))})"
    return text


def _sync_orchestrator_job_plan(parent_chat_id: str) -> None:
    """Rebuild the orchestrator-scope plan from live jobs (best-effort)."""
    chat = str(parent_chat_id or "")
    if not chat:
        return
    try:
        with _LOCK:
            entries = sorted(
                (dict(e) for e in _DELEGATIONS.values() if e.get("parent") == chat),
                key=lambda e: str(e.get("started") or ""),
            )
        if not entries:
            return
        items: list[dict] = []
        for e in entries:
            est = str(e.get("status") or "")
            status = "pending" if est == "queued" else (
                "in_progress" if est == "running" else "completed"
            )
            items.append({"step": _job_step_text(e)[:200], "status": status})
        from sync.sync_helpers import plan_state, save_plan
        from sync.state_sync import safe_update_state

        with _PLAN_SYNC_LOCK:
            save_plan(items, "", chat, "orchestrator")
            try:
                safe_update_state({"plan": plan_state("", chat)})
            except Exception:
                pass
    except Exception as exc:
        _log(f"[DELEGATE] Job-plan sync skipped: {exc}")


def _result_block(entry: dict) -> str:
    status = entry.get("status", "")
    lines = [
        "[DELEGATION RESULT]",
        f"id: {entry['id']}",
        f"agent: {entry['agent_display']}",
        f"title: {entry['title']}",
        f"status: {'FAILED' if status == 'failed' else 'CANCELLED' if status == 'cancelled' else 'DONE'}",
        f"chat: {entry['workspace']}/{entry['chat_id']}",
    ]
    if entry.get("mode") == "resume":
        lines.append(f"project: {entry.get('project', '')} (durable session continued)")
    if entry.get("acceptance"):
        lines.append(f"acceptance: {str(entry.get('acceptance'))[:300]}")
    if entry.get("duration_s") is not None:
        lines.append(f"duration: {entry.get('duration_s')}s")
    if entry.get("artifact"):
        lines.append(f"artifact: {entry.get('artifact')}")
    reply = str(entry.get("result") or "").strip()
    if status in ("failed", "cancelled"):
        reply = (reply or entry.get("error") or "Agent crashed.")[:_WAKE_ERROR_CAP]
    else:
        reply = _summarize_result(reply)
    lines.append("reply:")
    lines.append(reply or "(no output)")
    if entry.get("artifact"):
        lines.append(f"(full report: delegation_status(id=\"{entry['id']}\"))")
    return "\n".join(lines)


# ── Per-item runner ──────────────────────────────────────────────────────────


def _node_state(agent_key: str, task_text: str, workspace: str, chat_id: str,
                images: list | None = None) -> dict:
    base = {
        "client_request": task_text,
        "project_path": workspace,
        "chat_id": chat_id,
        "error_count": 0,
        "test_report": "",
    }
    spec = _AGENT_SPECS.get(agent_key)
    is_dev = spec is not None and spec.state_builder == "dev"
    if images:
        if is_dev:
            # Dev drains images from the GLOBAL vision stash (race-prone under
            # parallelism), so embed the paths in the task text instead — the
            # Dev can open them with its own tools, deterministically.
            listed = "\n".join(f"- {p}" for p in images)
            base["client_request"] = f"{task_text}\n\nAttached image(s) to inspect:\n{listed}"
        else:
            # General/House accept images natively through their node state.
            base["images"] = list(images)
    if is_dev:
        try:
            from sync.state_sync import safe_get_state
            outs = safe_get_state().get("outputs", {}) or {}
            base["tech_spec"] = outs.get("tech_spec", "")
            base["requirements"] = outs.get("requirements", "")
        except Exception:
            pass
    return base


def _call_node(agent_key: str, state: dict) -> dict:
    """Run this delegation's node via the registry `runtime` entrypoint."""
    spec = _AGENT_SPECS.get(agent_key)
    if spec is None or not spec.runtime:
        raise RuntimeError(f"agent '{agent_key}' has no runtime entrypoint")
    import importlib

    mod_name, fn_name = spec.runtime.split(":", 1)
    return getattr(importlib.import_module(mod_name), fn_name)(state)


def _emit_completion(snapshot: dict, parent_chat_id: str) -> None:
    """Artifact + ledger + chat mirror + parent wake + log for a settled entry."""
    del_id = snapshot.get("id", "")
    # Full report to disk first — the wake block carries only a summary + path.
    # Idempotent: _finalize_entry may already have written it.
    try:
        if not snapshot.get("artifact"):
            artifact = _write_job_artifact(snapshot)
            if artifact:
                snapshot["artifact"] = artifact
                with _LOCK:
                    cur = _DELEGATIONS.get(del_id)
                    if cur is not None:
                        cur["artifact"] = artifact
    except Exception as e:
        _log(f"[DELEGATE] Artifact skipped for {del_id}: {e}")
    # Durable job ledger — one JSON line per completion.
    try:
        _append_job_ledger(snapshot)
    except Exception as e:
        _log(f"[DELEGATE] Ledger append skipped for {del_id}: {e}")
    # Auto job tracker: refresh the orchestrator plan from real jobs.
    _sync_orchestrator_job_plan(snapshot.get("parent") or parent_chat_id)

    # Mirror the final reply into the delegation's own chat.
    try:
        import workspace_manager as wm
        body = snapshot.get("result") or snapshot.get("error") or "No output."
        wm.add_message(snapshot.get("workspace", ""), snapshot.get("chat_id", ""), "agent", body, {
            "delegated": True,
            "del_id": del_id,
            "status": snapshot.get("status", ""),
        })
    except Exception as e:
        _log(f"[DELEGATE] Chat mirror skipped for {del_id}: {e}")

    # Deliver the completion event to the parked orchestrator (C1: enqueue if active).
    if parent_chat_id:
        try:
            from server.realtime.enqueue_bridge import enqueue_or_follow_up
            enqueue_or_follow_up(parent_chat_id, _result_block(snapshot))
        except Exception:
            try:
                from inbox import follow_up
                follow_up(parent_chat_id, _result_block(snapshot))
            except Exception as e:
                _log(f"[DELEGATE] Completion delivery skipped for {del_id}: {e}")
    # Mark delivered so the orchestrator can distinguish "settled but the wake
    # is still in flight" from "settled and already in the parent's inbox".
    with _LOCK:
        cur = _DELEGATIONS.get(del_id)
        if cur is not None:
            cur["delivered"] = True

    icon = "[OK]" if snapshot.get("status") == "done" else "[XX]"
    _log(f"[DELEGATE] {icon} {snapshot.get('agent_display', '')} finished: {snapshot.get('title', '')}")


def _finalize_entry(del_id: str, parent_chat_id: str, status: str, result: str,
                    error: str, expect=("queued", "running")) -> dict:
    """Mark an entry terminal and emit its completion.

    Guarded: when the entry already settled (e.g. cancelled via job_cancel
    while a watcher was polling), the call is a no-op returning the current
    snapshot — completions are emitted exactly once.
    """
    if isinstance(expect, str):
        expect = (expect,)
    with _LOCK:
        entry = _DELEGATIONS.get(del_id)
        if entry is None:
            return {}
        if entry.get("status") not in tuple(expect):
            return dict(entry)
        # Publish the terminal status before waking the parent.  The parent can
        # resume immediately when the inbox event arrives, so it must not observe
        # a stale "running" entry while processing a completed reply.
        entry["status"] = status
        entry["result"] = result
        entry["error"] = error
        entry["last_event_at"] = datetime.now().isoformat(timespec="seconds")
        try:
            started = datetime.fromisoformat(str(entry.get("started") or ""))
            entry["duration_s"] = max(0, int((datetime.now() - started).total_seconds()))
        except Exception:
            entry["duration_s"] = None
        # Write the artifact inside the same lock block (with the final result)
        # so any observer that sees "done" can already read the full report.
        if not entry.get("artifact"):
            try:
                _art = _write_job_artifact(entry)
                if _art:
                    entry["artifact"] = _art
            except Exception as e:
                _log(f"[DELEGATE] Artifact skipped for {del_id}: {e}")
        snapshot = dict(entry)
    _emit_completion(snapshot, parent_chat_id)
    return snapshot


#: Default cap for one resume turn (seconds).
_RESUME_TIMEOUT_DEFAULT = 1800.0
#: Queued-resume watch: poll chunk and total bound (seconds).
_RESUME_WATCH_CHUNK = 120.0
_RESUME_WATCH_TOTAL = 1800.0


def _watch_queued_resume(del_id: str, parent_chat_id: str, child_id: str) -> None:
    """Track a queued resume turn until it lands (bounded), then finalize.

    Runs on the entry's runner thread — the tool call itself returned long ago.
    """
    import time

    try:
        from agents.subagent_manager import child_status as _cs
    except Exception as e:
        _finalize_entry(del_id, parent_chat_id, "failed", "",
                        f"Error: resume watch unavailable: {e}"[:500])
        return
    waited = 0.0
    last = ""
    st: dict = {}
    while waited < _RESUME_WATCH_TOTAL:
        try:
            st = _cs(child_id, wait_seconds=min(_RESUME_WATCH_CHUNK, _RESUME_WATCH_TOTAL - waited))
        except Exception as e:
            _finalize_entry(del_id, parent_chat_id, "failed", "",
                            f"Error: resume watch failed: {e}"[:500])
            return
        last = str(st.get("result") or last)
        if st.get("status") != "running":
            break
        waited += _RESUME_WATCH_CHUNK
        # Cancelled meanwhile (job_cancel finalizes itself) — stop quietly.
        with _LOCK:
            e = _DELEGATIONS.get(del_id)
            if e is None or e.get("status") not in ("queued", "running"):
                return
    else:
        _finalize_entry(del_id, parent_chat_id, "failed", "",
                        "Error: resume turn timed out waiting for the project Dev agent.")
        return
    if st.get("status") == "unknown":
        _finalize_entry(del_id, parent_chat_id, "failed", "",
                        "Error: project Dev session lost while waiting (unknown child).")
        return
    failed = last.startswith("ERROR")
    _finalize_entry(del_id, parent_chat_id, "failed" if failed else "done", last, "")


def _run_resume_item(del_id: str, parent_chat_id: str) -> None:
    """Runner for mode=resume entries: one turn on the durable Dev child.

    Same tracked-job contract as a fresh delegation (ledger, chat mirror,
    parent wake) — the only difference is the session continues instead of
    starting a new chat.
    """
    entry = _DELEGATIONS.get(del_id)
    if entry is None:
        return
    _acquire_slot()
    try:
        with _LOCK:
            cur = _DELEGATIONS.get(del_id)
            if cur is None or cur.get("status") != "queued":
                return  # cancelled while queued
            cur["status"] = "running"
            cur["last_event_at"] = datetime.now().isoformat(timespec="seconds")
        _sync_orchestrator_job_plan(parent_chat_id)
        _run_resume_turn(del_id, parent_chat_id)
    finally:
        _release_slot()


def _run_resume_turn(del_id: str, parent_chat_id: str) -> None:
    entry = _DELEGATIONS.get(del_id)
    if entry is None:
        return
    child_id = entry.get("child_id", "")
    ws = entry.get("workspace", "")
    try:
        timeout = float(entry.get("timeout") or _RESUME_TIMEOUT_DEFAULT)
    except (TypeError, ValueError):
        timeout = _RESUME_TIMEOUT_DEFAULT
    try:
        from agents.subagent_manager import send_message as _sm
    except Exception as e:
        _finalize_entry(del_id, parent_chat_id, "failed", "",
                        f"Error: resume runtime unavailable: {e}"[:500])
        return
    try:
        r = _sm(child_id, entry.get("task", ""), ws, timeout=timeout)
    except Exception as e:
        _finalize_entry(del_id, parent_chat_id, "failed", "",
                        f"Error: resume send failed: {e}"[:500])
        return
    if r.get("queued") or r.get("timed_out"):
        # Busy child (queued) or turn still running past timeout — track it
        # like any other running job; the result lands via the watcher.
        _watch_queued_resume(del_id, parent_chat_id, child_id)
        return
    if r.get("status") == "error":
        _finalize_entry(del_id, parent_chat_id, "failed", "",
                        str(r.get("result") or f"Error: unknown continuable child '{child_id}'")[:2000])
        return
    _finalize_entry(del_id, parent_chat_id, "done", str(r.get("result") or ""), "")


def _watchdog(del_id: str, parent_chat_id: str, timeout_s: float) -> None:
    """Per-job deadline: cooperative abort, grace, then FAILED tombstone.

    The runner may still land later; _finalize_entry is guarded so the
    timeout wake is emitted exactly once.
    """
    import time

    try:
        grace = max(1.0, float(os.environ.get("ORCH_JOB_KILL_GRACE", "30")))
    except Exception:
        grace = 30.0

    time.sleep(max(1.0, float(timeout_s)))
    e = get_entry(del_id)
    if not e or e.get("status") not in ("queued", "running"):
        return
    try:
        from inbox import abort
        abort(e.get("chat_id", ""), keep_inbox=True)
    except Exception:
        pass
    deadline = time.time() + grace
    while time.time() < deadline:
        e = get_entry(del_id)
        if not e or e.get("status") not in ("queued", "running"):
            return
        time.sleep(1.0)
    _finalize_entry(del_id, parent_chat_id, "failed", "",
                    f"Error: job exceeded its {int(timeout_s)}s timeout (aborted).")


def _run_item(del_id: str, item: dict, parent_chat_id: str) -> None:
    entry = _DELEGATIONS.get(del_id)
    if entry is None:
        return
    if entry.get("mode") == "resume":
        _run_resume_item(del_id, parent_chat_id)
        return
    agent_key = entry["agent_key"]

    # Bounded fan-out: wait for a slot; a job cancelled while queued exits here.
    _acquire_slot()
    try:
        with _LOCK:
            cur = _DELEGATIONS.get(del_id)
            if cur is None or cur.get("status") != "queued":
                return
            cur["status"] = "running"
            cur["last_event_at"] = datetime.now().isoformat(timespec="seconds")
        _sync_orchestrator_job_plan(parent_chat_id)

        state_token = None
        try:
            from sync.state_sync import active_state, new_scoped_state
            state_token = active_state.set(new_scoped_state({
                "chat_id": entry["chat_id"],
                "project_path": entry["workspace"],
            }))
        except Exception:
            state_token = None
        try:
            from llm.llm import stream_chat_override
            token = stream_chat_override.set(entry["chat_id"])
        except Exception:
            token = None

        local_status = "done"
        local_result = ""
        local_error = ""
        usage: dict = {}
        try:
            res = _call_node(agent_key, _node_state(
                agent_key, entry["task"], entry["workspace"], entry["chat_id"],
                entry.get("images") or None,
            ))
            report = str(
                res.get("agent_report")
                or res.get("code")
                or "Completed with no output."
            )
            failed = bool(res.get("error")) or report.startswith("ERROR")
            local_status = "failed" if failed else "done"
            local_result = report
            if res.get("error"):
                local_error = str(res["error"])[:500]
            # Telemetry: capture the child's usage before the scoped state resets.
            try:
                from sync.state_sync import safe_get_state
                _tu = (safe_get_state() or {}).get("token_usage") or {}
                usage = {
                    "total_input_tokens": int(_tu.get("total_input_tokens", 0) or 0),
                    "total_output_tokens": int(_tu.get("total_output_tokens", 0) or 0),
                    "total_cost": float(_tu.get("total_cost", 0) or 0),
                    "calls": len(_tu.get("calls") or []),
                }
            except Exception:
                usage = {}
        except Exception as e:
            local_status = "failed"
            local_error = str(e)[:500]
            local_result = ""
            _log(f"[DELEGATE] {del_id} ({entry['title']}) crashed: {e}")
        finally:
            if token is not None:
                try:
                    stream_chat_override.reset(token)
                except Exception:
                    pass
            if state_token is not None:
                try:
                    from sync.state_sync import active_state
                    active_state.reset(state_token)
                except Exception:
                    pass

        with _LOCK:
            e_ = _DELEGATIONS.get(del_id)
            if e_ is not None:
                e_["usage"] = usage
        _finalize_entry(del_id, parent_chat_id, local_status, local_result, local_error)
    finally:
        _release_slot()


# ── Public API ───────────────────────────────────────────────────────────────


def start_delegations(items: list, parent_chat_id: str, fallback_ws: str,
                      background: bool = False) -> list[dict]:
    """Validate + register + launch every item. Returns spawn summaries.

    ``background=True`` with a parent chat id starts one daemon thread per item
    and returns immediately (the orchestrator parks; completions arrive as
    inbox events). Without a parent chat id the call degrades to BLOCKING
    sequential runs so results can be returned directly (no inbox to park on);
    otherwise items run through the pydantic-graph map+join path.
    """
    summaries: list[dict] = []
    graph_items: list[dict] = []
    blocking = not parent_chat_id

    for raw in items or []:
        if not isinstance(raw, dict):
            summaries.append({"ok": False, "error": "item must be object"})
            continue
        agent_key = _norm_agent(raw.get("agent") or raw.get("name"))
        task_text = str(raw.get("task") or "").strip()
        project_name = str(raw.get("project") or "").strip()
        if project_name and not agent_key:
            agent_key = "dev"  # project= implies the durable Dev when agent omitted
        if agent_key not in DELEGATABLE_AGENTS:
            summaries.append({"ok": False, "error": f"unknown agent '{raw.get('agent')}' (use Dev/General/House)"})
            continue
        if not task_text:
            summaries.append({"ok": False, "error": "task text is required"})
            continue

        # RESUME mode: project=<name> continues a registered project's durable
        # Dev session instead of starting a new chat. No new tab, no new
        # folder — the message goes to the project's own chat.
        resume_info = None
        if project_name:
            if agent_key != "dev":
                summaries.append({"ok": False, "error": f"Error: 'project' continues a registered project's durable Dev session — omit agent or use Dev (got '{raw.get('agent')}')"})
                continue
            agent_key = "dev"
            try:
                from agents.project_router import find_project_dev as _find, list_projects as _lp
                resume_info = _find(project_name)
                available = ", ".join(p.get("name", "") for p in _lp()) or "none"
            except Exception as e:
                summaries.append({"ok": False, "error": f"Error: project lookup failed: {e}"})
                continue
            if not resume_info:
                summaries.append({"ok": False, "error": f"Error: [NOT FOUND] No registered project matches '{project_name}'. Available projects: {available}"})
                continue
            if not resume_info.get("child_id"):
                summaries.append({"ok": False, "error": f"Error: project '{resume_info.get('name')}' has no durable Dev agent yet."})
                continue

        title = str(raw.get("title") or "").strip() or " ".join(task_text.split())[:40]
        display = _AGENT_DISPLAY[agent_key]

        try:
            _timeout_raw = float(raw.get("timeout") or 0)
        except (TypeError, ValueError):
            _timeout_raw = 0.0
        # Fresh jobs get a default deadline (env-tunable); resume keeps its own
        # send-message timeout semantics.
        job_timeout = _timeout_raw or (
            _RESUME_TIMEOUT_DEFAULT if resume_info is not None else _job_timeout_default()
        )

        # Brief: outcome + acceptance are WHAT, never HOW. The standing line
        # keeps the specialist in charge of the method (they are the pro).
        outcome = str(raw.get("outcome") or "").strip()
        acceptance = str(raw.get("acceptance") or "").strip()
        brief = [task_text]
        if outcome:
            brief.append(f"Outcome (what done looks like): {outcome}")
        if acceptance:
            brief.append(f"Acceptance (how success will be judged): {acceptance}")
        brief.append("Method is yours to choose — improve on this brief if needed and report any deviation.")
        task_text = "\n\n".join(brief)

        # Images: keep only paths that exist right now (the model hallucinates
        # paths now and then; a missing attachment must not crash the child).
        images: list[str] = []
        for p in (raw.get("images") or []):
            sp = str(p).strip()
            if sp and os.path.isfile(sp):
                images.append(sp)
        if resume_info and images:
            # Resume turns take plain text — embed verified paths like the
            # Dev fresh path does (see _node_state) so nothing is dropped.
            listed = "\n".join(f"- {p}" for p in images)
            task_text = f"{task_text}\n\nAttached image(s) to inspect:\n{listed}"

        if resume_info is not None:
            workspace = resume_info.get("workspace_path") or fallback_ws
            chat_id = str(resume_info.get("chat_id") or "")
            continued = True
            try:
                import workspace_manager as wm
                if not chat_id or not wm.get_chat(workspace, chat_id, include_traces=False):
                    # Never silently start a new chat here — resume must
                    # continue the durable session or fail loudly.
                    summaries.append({"ok": False, "error": f"Error: project '{resume_info.get('name')}' durable chat is missing — open the project once, then retry."})
                    continue
            except Exception as e:
                summaries.append({"ok": False, "error": f"Error: chat setup failed: {e}"})
                continue
        else:
            workspace = _ensure_workspace_dir(_resolve_workspace(raw, agent_key, fallback_ws))

            chat_id = str(raw.get("chat_id") or "").strip()
            continued = bool(chat_id)
            try:
                import workspace_manager as wm
                if continued and not wm.get_chat(workspace, chat_id, include_traces=False):
                    # Unknown chat id — fall back to a fresh chat rather than failing.
                    continued = False
                if not continued:
                    try:
                        parent_ws_id = wm.find_chat_workspace_id(parent_chat_id or "")
                    except Exception:
                        parent_ws_id = ""
                    chat = wm.create_chat(workspace, title=f"{title} · {display}", parent_chat_id=parent_chat_id or "", parent_workspace_id=parent_ws_id or "")
                    chat_id = chat.get("id", "")
            except Exception as e:
                summaries.append({"ok": False, "error": f"chat setup failed: {e}"})
                continue

        del_id = f"del-{uuid.uuid4().hex[:8]}"
        entry = {
            "id": del_id,
            "agent_key": agent_key,
            "agent_display": display,
            "title": title,
            "task": task_text,
            "outcome": outcome,
            "acceptance": acceptance,
            "workspace": workspace,
            "chat_id": chat_id,
            "continued": continued,
            "mode": "resume" if resume_info else "fresh",
            "project": (resume_info.get("name", "") if resume_info else ""),
            "child_id": (resume_info.get("child_id", "") if resume_info else ""),
            "timeout": job_timeout,
            "parent": parent_chat_id or "",
            "status": "queued",
            "result": "",
            "error": "",
            "images": images,
            "usage": {},
            "duration_s": None,
            "artifact": "",
            "delivered": False,
            "started": datetime.now().isoformat(timespec="seconds"),
            "last_event_at": datetime.now().isoformat(timespec="seconds"),
        }
        with _LOCK:
            _DELEGATIONS[del_id] = entry

        # Deadline watchdog (fresh jobs only): abort + tombstone as FAILED.
        if resume_info is None and job_timeout and job_timeout > 0:
            threading.Thread(
                target=_watchdog,
                args=(del_id, parent_chat_id or "", float(job_timeout)),
                daemon=True,
                name=f"watchdog-{del_id}",
            ).start()

        # Seed the chat with what was asked so opening it explains itself.
        try:
            import workspace_manager as wm
            wm.add_message(workspace, chat_id, "user", task_text, {
                "delegated": True,
                "del_id": del_id,
                "by": "orchestrator",
                "parent": parent_chat_id or "",
            })
        except Exception as e:
            _log(f"[DELEGATE] Task seed skipped for {del_id}: {e}")

        _log(f"[SUBAGENT] child={chat_id} name={display} ws={workspace} chat={chat_id} label={title} del={del_id}")

        summaries.append({
            "ok": True,
            "id": del_id,
            "agent": display,
            "title": title,
            "workspace": workspace,
            "chat_id": chat_id,
            "continued": continued,
            "mode": "resume" if resume_info else "fresh",
            "project": (resume_info.get("name", "") if resume_info else ""),
            "status": "queued",
        })
        graph_items.append({"del_id": del_id, "raw": dict(raw), "parent_chat_id": parent_chat_id or ""})

    # Auto job tracker: refresh the orchestrator plan from the registered jobs.
    _sync_orchestrator_job_plan(parent_chat_id or "")

    if not graph_items:
        return summaries

    if background and parent_chat_id:
        # Fire-and-forget fan-out: one daemon thread per item. Completion
        # events land in the parent's inbox (enqueue_or_follow_up), so a
        # parked orchestrator wakes with [DELEGATION RESULT] blocks.
        for gi in graph_items:
            threading.Thread(
                target=_run_item,
                args=(gi["del_id"], gi["raw"], parent_chat_id or ""),
                daemon=True,
                name=f"delegation-{gi['del_id']}",
            ).start()
        return summaries

    if blocking:
        # Degenerate mode: no inbox to park on — run BLOCKING and surface
        # the outcome directly in the summary.
        for gi in graph_items:
            _run_item(gi["del_id"], gi["raw"], "")
            with _LOCK:
                e = _DELEGATIONS[gi["del_id"]]
                s = next((x for x in summaries if x.get("id") == gi["del_id"]), None)
                if s:
                    s["status"] = e["status"]
                    s["result"] = (e.get("result") or e.get("error") or "")
        return summaries

    # Parallel path (parent chat present): run through the graph map+join.
    try:
        import asyncio
        results = asyncio.run(map_run_delegations(graph_items))
        by_id = {r.get("id"): r for r in (results or []) if isinstance(r, dict)}
        for s in summaries:
            if s.get("ok") and s.get("id") in by_id:
                s["status"] = by_id[s["id"]].get("status")
                s["result"] = by_id[s["id"]].get("result", "")
        return summaries
    except Exception as e:
        _log(f"[DELEGATE] parallel fallback failed ({e}); running blocking")
        for gi in graph_items:
            _run_item(gi["del_id"], gi["raw"], "")
            with _LOCK:
                e_ = _DELEGATIONS[gi["del_id"]]
                s = next((x for x in summaries if x.get("id") == gi["del_id"]), None)
                if s:
                    s["status"] = e_["status"]
                    s["result"] = (e_.get("result") or e_.get("error") or "")
        return summaries


def running_count(parent_chat_id: str) -> int:
    """Unfinished delegations spawned from this chat (queued + running)."""
    with _LOCK:
        return sum(
            1 for e in _DELEGATIONS.values()
            if e["status"] in ("queued", "running") and e.get("parent") == (parent_chat_id or "")
        )


def undelivered_count(parent_chat_id: str) -> int:
    """Settled jobs whose completion event has not been delivered yet.

    Closes the race where a job settles (status flips) microseconds before its
    [DELEGATION RESULT] lands in the parent inbox: the orchestrator must park
    once more instead of exiting with a result still in flight.
    """
    with _LOCK:
        return sum(
            1 for e in _DELEGATIONS.values()
            if e.get("parent") == (parent_chat_id or "")
            and e.get("status") in ("done", "failed", "cancelled")
            and not e.get("delivered")
        )


def get_entry(del_id: str) -> dict | None:
    with _LOCK:
        e = _DELEGATIONS.get(del_id)
        return dict(e) if e else None


def cancel_all() -> int:
    """Mark every still-running delegation as cancelled (cooperative best-effort).

    Node loops observe aborts through their own inbox channels; this flips the
    bookkeeping so the orchestrator's park condition clears immediately.
    """
    n = 0
    with _LOCK:
        for e in _DELEGATIONS.values():
            if e["status"] in ("queued", "running"):
                e["status"] = "cancelled"
                e["error"] = "cancelled by user request"
                n += 1
    return n


def status_table(status: str | None = None) -> str:
    with _LOCK:
        entries = sorted(_DELEGATIONS.values(), key=lambda e: e["started"], reverse=True)
    want = str(status or "").strip().lower()
    if want:
        entries = [e for e in entries if e.get("status") == want]
    if not entries:
        return "[OK] No delegations in this session." if not want else f"[OK] No delegations with status '{want}'."
    icons = {"queued": "[..]", "running": "[>]", "done": "[OK]", "failed": "[XX]", "cancelled": "[--]"}
    lines = [f"{len(entries)} delegation(s):"]
    for e in entries[:20]:
        tag = f" [resume: {e.get('project', '')}]" if e.get("mode") == "resume" else ""
        elapsed = f" · {_elapsed(e.get('started', ''))}" if e["status"] in ("queued", "running") else ""
        lines.append(
            f"  {icons.get(e['status'], '[??]')} {e['id']} {e['agent_display']:7s} "
            f"{e['status']:9s} {e['title']}{tag} ({e['workspace']}/{e['chat_id']}){elapsed}"
        )
        if e["status"] in ("done", "failed"):
            reply = str(e.get("result") or e.get("error") or "").strip().replace("\n", " ")
            if reply:
                lines.append(f"      reply: {reply[:600]}{'…' if len(reply) > 600 else ''}")
    return "\n".join(lines)


#: Evidence-bundle budgets for job_detail (chars).
_DETAIL_TASK_CAP = 2000
_DETAIL_RESULT_CAP = 8000
_DETAIL_CHAT_TAIL = 3
_DETAIL_FILE_DIGEST = 10


def _elapsed(started_iso: str) -> str:
    """Human elapsed time since an ISO start timestamp ('?' when unknown)."""
    try:
        dt = datetime.fromisoformat(started_iso)
        secs = max(0, int((datetime.now() - dt).total_seconds()))
    except Exception:
        return "?"
    if secs < 60:
        return f"{secs}s"
    mins, secs = divmod(secs, 60)
    if mins < 60:
        return f"{mins}m{secs:02d}s"
    hrs, mins = divmod(mins, 60)
    return f"{hrs}h{mins:02d}m"


def wait_for_job(del_id: str, wait_seconds: float = 0) -> dict | None:
    """Bounded wait for one job to settle. Returns its snapshot (or None)."""
    import time

    try:
        budget = max(0.0, min(float(wait_seconds or 0), 300.0))
    except (TypeError, ValueError):
        budget = 0.0
    deadline = time.time() + budget
    while True:
        with _LOCK:
            e = _DELEGATIONS.get(del_id)
            snap = dict(e) if e else None
        if snap is None or snap.get("status") not in ("queued", "running") or time.time() >= deadline:
            return snap
        time.sleep(2.0)


def job_detail(del_id: str) -> str:
    """Full evidence bundle for ONE job — the verification read.

    Cheap liveness stays on status_table; this is the on-demand drill-down:
    what was asked, what came back (capped), where it lives, what the job's
    own chat shows, and what files changed under its workspace (best-effort).
    """
    with _LOCK:
        e = dict(_DELEGATIONS[del_id]) if del_id in _DELEGATIONS else None
    if e is None:
        return f"Error: unknown job id '{del_id}'. Call delegation_status() for the board."
    lines = [
        "[JOB DETAIL]",
        f"id: {e['id']}",
        f"agent: {e['agent_display']}",
        f"mode: {e.get('mode', 'fresh')}" + (f" (project: {e.get('project', '')})" if e.get("mode") == "resume" else ""),
        f"title: {e['title']}",
        f"status: {e['status']}",
        f"elapsed: {_elapsed(e.get('started', ''))}",
    ]
    if e.get("duration_s") is not None:
        lines.append(f"duration: {e.get('duration_s')}s")
    usage = e.get("usage") or {}
    if usage:
        lines.append(
            f"usage: in={usage.get('total_input_tokens', 0)} "
            f"out={usage.get('total_output_tokens', 0)} "
            f"cost={usage.get('total_cost', 0)} calls={usage.get('calls', 0)}"
        )
    if e.get("outcome"):
        lines.append(f"outcome: {str(e.get('outcome'))[:400]}")
    if e.get("acceptance"):
        lines.append(f"acceptance: {str(e.get('acceptance'))[:400]}")
    lines.append(f"chat: {e['workspace']}/{e['chat_id']}")
    if e.get("artifact"):
        lines.append(f"artifact: {e['artifact']}")
    task = str(e.get("task") or "")
    lines.append("task:")
    lines.append((task[:_DETAIL_TASK_CAP] + "…" if len(task) > _DETAIL_TASK_CAP else task) or "(none)")
    if e["status"] in ("done", "failed", "cancelled"):
        result = str(e.get("result") or "")
        error = str(e.get("error") or "")
        # Prefer the artifact file (full report) when it exists.
        artifact_text = ""
        if e.get("artifact"):
            try:
                with open(e["artifact"], "r", encoding="utf-8") as f:
                    artifact_text = f.read()
            except Exception:
                artifact_text = ""
        body = artifact_text or result
        lines.append("result:")
        if len(body) > _DETAIL_RESULT_CAP:
            lines.append(body[:_DETAIL_RESULT_CAP]
                         + f"\n[... truncated {len(body) - _DETAIL_RESULT_CAP} chars — open the artifact/job chat for the full report]")
        else:
            lines.append(body or "(no output)")
        if error and error not in body:
            lines.append(f"error: {error[:1000]}")
    # Recent chat tail — child transcripts usually carry test results inline.
    try:
        import workspace_manager as wm
        chat = wm.get_chat(e["workspace"], e["chat_id"], include_traces=False)
        msgs = (chat.get("messages") or []) if chat else []
        tail = [m for m in msgs[-8:] if str(m.get("content") or "").strip()][- _DETAIL_CHAT_TAIL:]
        if tail:
            lines.append("recent chat:")
            for m in tail:
                snippet = " ".join(str(m.get("content") or "").split())
                lines.append(f"  [{m.get('role', '?')}]: {snippet[:500]}{'…' if len(snippet) > 500 else ''}")
    except Exception:
        pass
    # File digest — filter the global watcher ring by workspace prefix.
    try:
        import tools.watcher as _watcher
        ws_prefix = os.path.normpath(e["workspace"]).lower() if e.get("workspace") else ""
        with _watcher._LOCK:
            ring = list(_watcher._RING)
        hits = [it for it in ring if ws_prefix and str(it.get("path", "")).lower().startswith(ws_prefix)]
        if hits:
            import time as _time
            lines.append(f"files changed ({len(hits)} tracked, last {_DETAIL_FILE_DIGEST}):")
            for it in hits[-_DETAIL_FILE_DIGEST:]:
                ts = _time.strftime("%H:%M:%S", _time.localtime(it["ts"]))
                lines.append(f"  {ts}  {it['change']:>6}  {it['path']}")
        else:
            lines.append("files changed: n/a (watcher has no data for this workspace)")
    except Exception:
        lines.append("files changed: n/a")
    return "\n".join(lines)


def park_wait(parent_chat_id: str, timeout_s: float | None = None) -> dict:
    """Block until at least one inbox event arrives (or timeout/abort).

    Returns ``{"events": [...], "steering": [...], "aborted": bool, "timed_out": bool}``.
    """
    try:
        from inbox import consume_abort, drain_follow_up, drain_steering
    except Exception:
        return {"events": [], "steering": [], "aborted": False, "timed_out": True}

    if timeout_s is None:
        try:
            timeout_s = float(os.environ.get("ORCH_WAIT_TIMEOUT", "1800"))
        except Exception:
            timeout_s = 1800.0

    import time

    waited = 0.0
    interval = 0.5
    while waited < timeout_s:
        if consume_abort(parent_chat_id):
            return {"events": [], "steering": [], "aborted": True, "timed_out": False}
        # Drain EVERYTHING queued so far (the per-chat FIFOs may deliver
        # one-at-a-time); a single wake should carry every ready event.
        events: list[str] = []
        steering: list[str] = []
        while True:
            e = drain_follow_up(parent_chat_id)
            s = drain_steering(parent_chat_id)
            if not e and not s:
                break
            events.extend(e)
            steering.extend(s)
        if events or steering:
            return {"events": events, "steering": steering, "aborted": False, "timed_out": False}
        time.sleep(interval)
        waited += interval

    return {"events": [], "steering": [], "aborted": False, "timed_out": True}


def _ledger_path() -> str:
    harness = os.environ.get("HARNESS_ROOT") or os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    return os.path.join(harness, ".agent-memory", "orchestrator_jobs.jsonl")


def _append_job_ledger(entry: dict) -> None:
    path = _ledger_path()
    try:
        os.makedirs(os.path.dirname(path), exist_ok=True)
    except Exception:
        pass
    result_snip = str(entry.get("result") or entry.get("error") or "")[:300]
    usage = entry.get("usage") or {}
    record = {
        "ts": datetime.now().isoformat(timespec="seconds"),
        "id": entry.get("id", ""),
        "agent": entry.get("agent_display", ""),
        "title": entry.get("title", ""),
        "workspace": entry.get("workspace", ""),
        "chat_id": entry.get("chat_id", ""),
        "status": entry.get("status", ""),
        "result": result_snip,
        "duration_s": entry.get("duration_s"),
        "cost": usage.get("total_cost", 0),
        "artifact": entry.get("artifact", ""),
    }
    line = json.dumps(record, ensure_ascii=False)
    try:
        with open(path, "a", encoding="utf-8") as f:
            f.write(line + "\n")
    except Exception:
        return
    with _LOCK:
        try:
            _truncate_ledger_locked(path)
        except Exception:
            pass


def _truncate_ledger_locked(path: str) -> None:
    try:
        with open(path, "r", encoding="utf-8") as f:
            lines = f.readlines()
    except FileNotFoundError:
        return
    if len(lines) <= 500:
        return
    keep = lines[-200:]
    tmp = path + ".tmp"
    try:
        with open(tmp, "w", encoding="utf-8") as f:
            f.writelines(keep)
        os.replace(tmp, path)
    except Exception:
        try:
            os.unlink(tmp)
        except Exception:
            pass


def recent_jobs_block(n: int = 10) -> str:
    path = _ledger_path()
    try:
        with open(path, "r", encoding="utf-8") as f:
            lines = [ln.strip() for ln in f if ln.strip()]
    except FileNotFoundError:
        return ""
    if not lines:
        return ""
    tail = lines[-max(1, n):]
    out = ["## Recent Jobs"]
    for ln in tail:
        try:
            rec = json.loads(ln)
        except Exception:
            continue
        out.append(
            f"- {rec.get('ts','')} [{rec.get('status','')}] "
            f"{rec.get('agent','')} · {rec.get('title','')} "
            f"({rec.get('workspace','')}/{rec.get('chat_id','')})"
            + (f" {rec.get('duration_s')}s" if rec.get("duration_s") is not None else "")
            + f" — {rec.get('result','')[:120]}"
        )
    return "\n".join(out) if len(out) > 1 else ""


# ── Graph-driven parallel delegation (map + join) ────────────────────────────
#
# Each delegation item is a parallel step in a pydantic-graph ``GraphBuilder``
# map+join; results are collected via a join node into one list. Preserves
# per-item chat isolation + inbox delivery (the _run_item body is unchanged;
# only the scheduling mechanism differs).

_DEL_GRAPH: object | None = None
_DEL_GRAPH_LOCK = threading.Lock()


def _build_del_graph():
    """Build (once) the delegation map+join graph.

    ``map_run_delegations`` receives a list of (del_id, raw, parent_chat_id)
    tuples; each is executed in its own parallel step, results are collected
    by a join into a list of per-item summaries.
    """
    from pydantic_graph import GraphBuilder, StepContext, reduce_list_append

    g = GraphBuilder(input_type=list, output_type=list)

    @g.step
    async def run_item(ctx: StepContext[None, None, dict]) -> dict:
        item = ctx.inputs  # {"del_id":..., "raw":..., "parent_chat_id":...}
        del_id = item["del_id"]
        raw = item["raw"]
        parent = item["parent_chat_id"]
        _run_item(del_id, raw, parent)
        entry = _DELEGATIONS.get(del_id) or {}
        return {
            "id": del_id,
            "agent": entry.get("agent_display", ""),
            "title": entry.get("title", ""),
            "status": entry.get("status", "failed"),
            "result": (entry.get("result") or entry.get("error") or ""),
        }

    collect = g.join(reduce_list_append, initial_factory=list)

    g.add(
        g.edge_from(g.start_node).map().to(run_item),
        g.edge_from(run_item).to(collect),
        g.edge_from(collect).to(g.end_node),
    )
    return g.build()


def get_del_graph():
    global _DEL_GRAPH
    if _DEL_GRAPH is None:
        with _DEL_GRAPH_LOCK:
            if _DEL_GRAPH is None:
                _DEL_GRAPH = _build_del_graph()
    return _DEL_GRAPH


async def map_run_delegations(graph_items: list) -> list[dict]:
    """Async parallel fan-out via the graph map+join path.

    ``graph_items``: pre-validated entries from :func:`start_delegations`
    (``{"del_id", "raw", "parent_chat_id"}``) — validation and registration
    happen exactly once, upstream. Returns per-item result dicts keyed by id.
    Order of the returned list matches input order.
    """
    # Run via graph map+join (parallel). If the graph path fails, degrade to
    # blocking sequential runs so delegations never block on graph infra.
    try:
        graph = get_del_graph()
        results = await graph.run(inputs=graph_items)
        return [r for r in (results or []) if isinstance(r, dict)]
    except Exception as e:
        _log(f"[DELEGATE] graph fan-out failed ({e}); running blocking sequential")
        out = []
        for gi in graph_items:
            _run_item(gi["del_id"], gi["raw"], "")
            with _LOCK:
                e_ = _DELEGATIONS[gi["del_id"]]
            out.append({
                "id": gi["del_id"],
                "status": e_["status"],
                "result": (e_.get("result") or e_.get("error") or ""),
            })
        return out


def reset_delegations() -> None:
    """Test helper."""
    global _ACTIVE_SLOTS
    with _LOCK:
        _DELEGATIONS.clear()
        _ACTIVE_SLOTS = 0
        _SLOT_COND.notify_all()
