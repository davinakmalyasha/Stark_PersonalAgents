"""
Delegation runtime — parallel fan-out from the Orchestrator to workspace agents.

One ``delegate(...)`` tool call fans out N items through a ThreadPoolExecutor.
Every item routes through the CANONICAL node runner (developer_node /
general_node / house_node) — never the degraded generic subagent loop.

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
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime

#: Canonical delegatable agents (workspace tier). Specialists (UI/Deploy)
#: stay reachable through Dev's call_subagent, not through this runtime.
DELEGATABLE_AGENTS = ("dev", "general", "house")

_AGENT_DISPLAY = {"dev": "Dev", "general": "General", "house": "House"}

_REPLY_CAP = 1200

_EXECUTOR = ThreadPoolExecutor(max_workers=8, thread_name_prefix="delegate")

_LOCK = threading.RLock()

#: del_id -> entry dict. Entries survive until process exit (small).
_DELEGATIONS: dict[str, dict] = {}


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
    if agent_key == "dev":
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


def _result_block(entry: dict) -> str:
    lines = [
        "[DELEGATION RESULT]",
        f"id: {entry['id']}",
        f"agent: {entry['agent_display']}",
        f"title: {entry['title']}",
        f"status: {'FAILED' if entry['status'] == 'failed' else 'DONE'}",
        f"chat: {entry['workspace']}/{entry['chat_id']}",
    ]
    reply = str(entry.get("result") or "").strip()
    if entry["status"] == "failed":
        reply = reply or entry.get("error") or "Agent crashed."
    if len(reply) > _REPLY_CAP:
        reply = reply[:_REPLY_CAP] + "\n… (truncated — full report in the delegation chat)"
    lines.append("reply:")
    lines.append(reply or "(no output)")
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
    if images:
        if agent_key == "dev":
            # Dev drains images from the GLOBAL vision stash (race-prone under
            # parallelism), so embed the paths in the task text instead — the
            # Dev can open them with its own tools, deterministically.
            listed = "\n".join(f"- {p}" for p in images)
            base["client_request"] = f"{task_text}\n\nAttached image(s) to inspect:\n{listed}"
        else:
            # General/House accept images natively through their node state.
            base["images"] = list(images)
    if agent_key == "dev":
        try:
            from sync.state_sync import shared_state
            outs = shared_state.get("outputs", {}) or {}
            base["tech_spec"] = outs.get("tech_spec", "")
            base["requirements"] = outs.get("requirements", "")
        except Exception:
            pass
    return base


def _call_node(agent_key: str, state: dict) -> dict:
    if agent_key == "dev":
        from agents.developer_agent import developer_node
        return developer_node(state)
    if agent_key == "general":
        from agents.general_agent import general_node
        return general_node(state)
    from agents.house_agent import house_node
    return house_node(state)


def _run_item(del_id: str, item: dict, parent_chat_id: str) -> None:
    entry = _DELEGATIONS.get(del_id)
    if entry is None:
        return
    agent_key = entry["agent_key"]
    try:
        from llm.llm import stream_chat_override
        token = stream_chat_override.set(entry["chat_id"])
    except Exception:
        token = None

    local_status = "done"
    local_result = ""
    local_error = ""
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

    snapshot = dict(entry)
    snapshot["status"] = local_status
    snapshot["result"] = local_result
    snapshot["error"] = local_error

    # P4: durable job ledger — one JSON line per completion.
    try:
        _append_job_ledger(snapshot)
    except Exception as e:
        _log(f"[DELEGATE] Ledger append skipped for {del_id}: {e}")

    # Mirror the final reply into the delegation's own chat.
    try:
        import workspace_manager as wm
        body = local_result or local_error or "No output."
        wm.add_message(entry["workspace"], entry["chat_id"], "agent", body, {
            "delegated": True,
            "del_id": del_id,
            "status": local_status,
        })
    except Exception as e:
        _log(f"[DELEGATE] Chat mirror skipped for {del_id}: {e}")

    # Deliver the completion event to the parked orchestrator.
    if parent_chat_id:
        try:
            from inbox import follow_up
            follow_up(parent_chat_id, _result_block(snapshot))
        except Exception as e:
            _log(f"[DELEGATE] Completion delivery skipped for {del_id}: {e}")

    with _LOCK:
        entry["status"] = local_status
        entry["result"] = local_result
        entry["error"] = local_error

    icon = "[OK]" if local_status == "done" else "[XX]"
    _log(f"[DELEGATE] {icon} {entry['agent_display']} finished: {entry['title']}")


# ── Public API ───────────────────────────────────────────────────────────────


def start_delegations(items: list, parent_chat_id: str, fallback_ws: str) -> list[dict]:
    """Validate + register + launch every item. Returns spawn summaries.

    Without a parent chat id the call degrades to BLOCKING sequential runs so
    results can be returned directly (no inbox to park on).
    """
    summaries: list[dict] = []

    for raw in items or []:
        if not isinstance(raw, dict):
            summaries.append({"ok": False, "error": "item must be an object"})
            continue
        agent_key = _norm_agent(raw.get("agent") or raw.get("name"))
        task_text = str(raw.get("task") or "").strip()
        if agent_key not in DELEGATABLE_AGENTS:
            summaries.append({"ok": False, "error": f"unknown agent '{raw.get('agent')}' (use Dev/General/House)"})
            continue
        if not task_text:
            summaries.append({"ok": False, "error": "task text is required"})
            continue

        title = str(raw.get("title") or "").strip() or " ".join(task_text.split())[:40]
        display = _AGENT_DISPLAY[agent_key]
        workspace = _resolve_workspace(raw, agent_key, fallback_ws)

        # Images: keep only paths that exist right now (the model hallucinates
        # paths now and then; a missing attachment must not crash the child).
        images: list[str] = []
        for p in (raw.get("images") or []):
            sp = str(p).strip()
            if sp and os.path.isfile(sp):
                images.append(sp)

        chat_id = str(raw.get("chat_id") or "").strip()
        continued = bool(chat_id)
        try:
            import workspace_manager as wm
            if continued and not wm.get_chat(workspace, chat_id, include_traces=False):
                # Unknown chat id — fall back to a fresh chat rather than failing.
                continued = False
            if not continued:
                chat = wm.create_chat(workspace, title=f"{title} · {display}")
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
            "workspace": workspace,
            "chat_id": chat_id,
            "continued": continued,
            "parent": parent_chat_id or "",
            "status": "running",
            "result": "",
            "error": "",
            "images": images,
            "started": datetime.now().isoformat(timespec="seconds"),
        }
        with _LOCK:
            _DELEGATIONS[del_id] = entry

        # Seed the chat with what was asked so opening it explains itself.
        try:
            import workspace_manager as wm
            wm.add_message(workspace, chat_id, "user", task_text, {
                "delegated": True,
                "del_id": del_id,
                "by": "orchestrator",
            })
        except Exception as e:
            _log(f"[DELEGATE] Task seed skipped for {del_id}: {e}")

        _log(f"[SUBAGENT] child={chat_id} name={display} label={title} del={del_id}")

        summaries.append({
            "ok": True,
            "id": del_id,
            "agent": display,
            "title": title,
            "workspace": workspace,
            "chat_id": chat_id,
            "continued": continued,
        })

        if parent_chat_id:
            _EXECUTOR.submit(_run_item, del_id, dict(raw), parent_chat_id)
        else:
            # Degenerate mode: no inbox to park on — run BLOCKING and surface
            # the outcome directly in the summary.
            _run_item(del_id, dict(raw), "")
            with _LOCK:
                e = _DELEGATIONS[del_id]
                summaries[-1]["status"] = e["status"]
                summaries[-1]["result"] = (e.get("result") or e.get("error") or "")[:_REPLY_CAP]

    return summaries


def running_count(parent_chat_id: str) -> int:
    """How many delegations spawned from this chat have not finished yet."""
    with _LOCK:
        return sum(
            1 for e in _DELEGATIONS.values()
            if e["status"] == "running" and e.get("parent") == (parent_chat_id or "")
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
            if e["status"] == "running":
                e["status"] = "cancelled"
                e["error"] = "cancelled by user request"
                n += 1
    return n


def status_table() -> str:
    with _LOCK:
        entries = sorted(_DELEGATIONS.values(), key=lambda e: e["started"], reverse=True)
    if not entries:
        return "[OK] No delegations in this session."
    icons = {"running": "[>]", "done": "[OK]", "failed": "[XX]", "cancelled": "[--]"}
    lines = [f"{len(entries)} delegation(s):"]
    for e in entries[:20]:
        lines.append(
            f"  {icons.get(e['status'], '[??]')} {e['id']} {e['agent_display']:7s} "
            f"{e['status']:9s} {e['title']} ({e['workspace']}/{e['chat_id']})"
        )
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
    record = {
        "ts": datetime.now().isoformat(timespec="seconds"),
        "id": entry.get("id", ""),
        "agent": entry.get("agent_display", ""),
        "title": entry.get("title", ""),
        "workspace": entry.get("workspace", ""),
        "chat_id": entry.get("chat_id", ""),
        "status": entry.get("status", ""),
        "result": result_snip,
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
            f"({rec.get('workspace','')}/{rec.get('chat_id','')}) — {rec.get('result','')[:120]}"
        )
    return "\n".join(out) if len(out) > 1 else ""


def reset_delegations() -> None:
    """Test helper."""
    with _LOCK:
        _DELEGATIONS.clear()
