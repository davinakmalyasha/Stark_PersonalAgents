"""Run queued wakes on the normal chat pipeline."""
from __future__ import annotations

import threading
import time
from datetime import datetime, timedelta, timezone

from server.store import agent_wakes, schedules
from server.store.chat_store import add_message, get_chat, get_chat_workspace_path
from server.store.run_tracker import begin_run, clear_active_run, set_user_message_id
from server.realtime.sse import mirror_publish
from sync.state_sync import active_state, new_scoped_state, safe_get_state

_started = False
_start_lock = threading.Lock()
_stop = threading.Event()


def _report_from_state(state: dict) -> str:
    outputs = state.get("outputs") or {}
    for value in (
        outputs.get("agent_report"),
        state.get("final_report"),
        state.get("agent_report"),
        state.get("content"),
    ):
        if value and str(value).strip():
            return str(value).strip()
    return ""


def _last_agent_message(workspace_path: str, chat_id: str) -> str:
    try:
        chat = get_chat(workspace_path, chat_id, include_traces=False, include_usage=False)
        if not chat:
            return ""
        for message in reversed(chat.get("messages") or []):
            if message.get("role") == "agent":
                return str(message.get("content") or "").strip()
    except Exception:
        pass
    return ""


def _persist_report(workspace_path: str, chat_id: str, report: str, metadata: dict) -> None:
    if not report:
        return
    if _last_agent_message(workspace_path, chat_id) == report.strip():
        return
    add_message(workspace_path, chat_id, "agent", report, metadata)


def _run_wake(row: dict) -> None:
    wake_id = row["id"]
    chat_id = row["chatId"]
    metadata = row.get("metadata") or {}
    workspace_path = row.get("workspacePath") or get_chat_workspace_path(chat_id) or ""
    if workspace_path == "_general":
        workspace_path = ""
    mode = row.get("mode") or "orchestrator"
    if not chat_id:
        agent_wakes.mark_wake(wake_id, "error")
        return

    if not begin_run(workspace_path, chat_id):
        retry = (datetime.now(timezone.utc) + timedelta(seconds=5)).isoformat()
        agent_wakes.mark_wake(wake_id, "queued", not_before=retry, increment_attempt=True)
        return

    user_message = None
    schedule_id = str(metadata.get("scheduleId") or "")
    try:
        agent_wakes.mark_wake(wake_id, "running", increment_attempt=True)
        user_message = add_message(
            workspace_path, chat_id, "user", row.get("text") or "",
            {**metadata, "wakeId": wake_id, "scheduledWake": bool(schedule_id)},
        )
        if user_message and user_message.get("id"):
            set_user_message_id(chat_id, user_message["id"])
        try:
            from tools.checkpoints import begin_run as begin_checkpoint_run
            begin_checkpoint_run(chat_id, workspace_path,
                                 user_message_id=(user_message or {}).get("id", ""))
        except Exception:
            pass

        run_state = new_scoped_state({
            "selected_model": "Automatic Fallback",
            "selected_temp": 0.7,
            "is_general": not workspace_path,
            "failed_models": [],
            "project_path": workspace_path,
            "chat_id": chat_id,
            "token_usage": {
                "total_input_tokens": 0,
                "total_output_tokens": 0,
                "total_cache_hit_tokens": 0,
                "total_cache_miss_tokens": 0,
                "total_cost": 0.0,
                "calls": [],
            },
        })
        token = active_state.set(run_state)
        final_state: dict = {}
        try:
            try:
                from verify_helper import run_graph_with_verify
            except Exception:
                from Testing_Log.Testing.verify_helper import run_graph_with_verify
            for snapshot in run_graph_with_verify(
                row.get("text") or "",
                workspace_path=workspace_path,
                chat_id=chat_id,
                force_orchestrator=(mode == "orchestrator"),
            ):
                if isinstance(snapshot, dict):
                    final_state = snapshot
                    mirror_publish(chat_id, "state", snapshot)
            try:
                final_state = safe_get_state()
            except Exception:
                pass
            report = _report_from_state(final_state)
            _persist_report(workspace_path, chat_id, report, {
                **metadata, "wakeId": wake_id,
                "scheduledWake": bool(schedule_id),
            })
            try:
                from server.routers.runs import _save_chat_token_usage
                _save_chat_token_usage(workspace_path, chat_id, run_state)
            except Exception:
                pass
            try:
                from tools.checkpoints import finalize_run as finalize_checkpoint
                finalize_checkpoint(chat_id, workspace_path, {})
            except Exception:
                pass
            mirror_publish(chat_id, "done", {"status": "complete", "wakeId": wake_id})
            agent_wakes.mark_wake(wake_id, "done")
        finally:
            active_state.reset(token)
    except Exception as exc:
        try:
            add_message(workspace_path, chat_id, "agent",
                        f"ERROR: scheduled wake failed — {exc}",
                        {**metadata, "wakeId": wake_id, "scheduledWake": True})
        except Exception:
            pass
        agent_wakes.mark_wake(wake_id, "error")
        mirror_publish(chat_id, "done", {"status": "error", "error": str(exc), "wakeId": wake_id})
    finally:
        if schedule_id:
            try:
                schedules.mark_after_run(schedule_id)
            except Exception:
                pass
        clear_active_run(chat_id=chat_id, workspace_path=workspace_path)


def _worker() -> None:
    while not _stop.is_set():
        try:
            for row in agent_wakes.due_wakes(limit=12):
                agent_wakes.mark_wake(row["id"], "dispatching")
                threading.Thread(target=_run_wake, args=(row,), daemon=True,
                                 name=f"wake-{row.get('id','')[:12]}").start()
        except Exception as exc:
            print(f"[WAKE] scheduler error: {exc}", flush=True)
        _stop.wait(2.0)


def start_wake_worker() -> bool:
    global _started
    with _start_lock:
        if _started:
            return False
        _started = True
        _stop.clear()
    try:
        agent_wakes.recover_dispatching()
    except Exception:
        pass
    threading.Thread(target=_worker, name="agent-wake-worker", daemon=True).start()
    return True


def stop_wake_worker(timeout: float = 3.0) -> None:
    global _started
    _stop.set()
    with _start_lock:
        _started = False
    time.sleep(0.05)
