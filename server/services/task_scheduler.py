"""
Task scheduler service — Phase 7 moved from server/api_server.py:771-879.
Recurrence (daily/weekly/monthly/interval-*) + due-list daemon.

Efficiency (Phase 8): the scheduler no longer scans every workspace → every
chat → full message read on a 15s tick. It keeps an in-memory due-list of
(chats that have a scheduled task with a nextRunAt), maintained by the
notify_tasks_changed() hook that chat_store.save_chat_tasks calls. The 15s
loop only touches chats on that list, and the list is rebuilt lazily once at
start (migrating any pre-existing scheduled tasks).
"""
import threading
import time
import uuid
from datetime import datetime, timedelta, timezone

from server.store.workspace_registry import GENERAL_WORKSPACE_ID, list_workspaces, get_workspace
from server.store.chat_store import list_chats, get_chat_tasks, save_chat_tasks, add_message

_started = False
_started_lock = threading.Lock()

# In-memory due-list: {(workspace_path, chat_id): {"workspace_id": str}}
# Only chats that have at least one scheduled task with a nextRunAt live here.
_due_lock = threading.Lock()
_due_chats: dict[tuple[str, str], dict] = {}


def workspace_path_for_id(workspace_id: str) -> str:
    if workspace_id == GENERAL_WORKSPACE_ID:
        return GENERAL_WORKSPACE_ID
    workspace = get_workspace(workspace_id)
    if not workspace:
        raise ValueError("Workspace not found")
    return workspace.get("path", "")


def notify_tasks_changed(workspace_path: str, chat_id: str) -> None:
    """Hook called by chat_store.save_chat_tasks after a task mutation.

    Re-reads only the lightweight task metadata and refreshes the due-list
    membership for this chat — O(1) instead of a full-store scan.
    """
    try:
        tasks = get_chat_tasks(workspace_path, chat_id)
        key = (workspace_path or GENERAL_WORKSPACE_ID, chat_id)
        has_due = any(
            (t.get("status") == "scheduled")
            and parse_task_time((t.get("schedule") or {}).get("nextRunAt"))
            for t in tasks
        )
        with _due_lock:
            if has_due:
                _due_chats[key] = {"workspace_id": _workspace_id_for_path(workspace_path)}
            else:
                _due_chats.pop(key, None)
    except Exception as exc:
        print(f"[TASK SCHEDULER] notify_tasks_changed error: {exc}")


def _workspace_id_for_path(workspace_path: str) -> str:
    if workspace_path in ("", GENERAL_WORKSPACE_ID):
        return GENERAL_WORKSPACE_ID
    try:
        return get_workspace(workspace_path).get("id", workspace_path) if get_workspace(workspace_path) else workspace_path
    except Exception:
        return workspace_path


def _prime_due_list() -> None:
    """One-time lazy scan at start to catch scheduled tasks persisted before
    the server (re)started. Rebuilds the due-list without any message reads."""
    try:
        with _due_lock:
            _due_chats.clear()
        for workspace in list_workspaces():
            workspace_id = workspace.get("id", "")
            try:
                workspace_path = workspace_path_for_id(workspace_id)
            except ValueError:
                continue
            for chat in list_chats(workspace_path):
                chat_id = chat.get("id", "")
                key = (workspace_path or GENERAL_WORKSPACE_ID, chat_id)
                try:
                    tasks = get_chat_tasks(workspace_path, chat_id)
                except Exception:
                    continue
                has_due = any(
                    (t.get("status") == "scheduled")
                    and parse_task_time((t.get("schedule") or {}).get("nextRunAt"))
                    for t in tasks
                )
                if has_due:
                    with _due_lock:
                        _due_chats[key] = {"workspace_id": workspace_id}
    except Exception as exc:
        print(f"[TASK SCHEDULER] due-list prime failed: {exc}")


def task_now() -> datetime:
    return datetime.now(timezone.utc)


def parse_task_time(value: str | None) -> datetime | None:
    if not value:
        return None
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
        return parsed.astimezone(timezone.utc) if parsed.tzinfo else parsed.replace(tzinfo=timezone.utc)
    except ValueError:
        return None


def next_recurring_time(current: datetime, recurrence: str) -> datetime | None:
    if recurrence == "daily":
        return current + timedelta(days=1)
    if recurrence == "weekly":
        return current + timedelta(days=7)
    if recurrence == "monthly":
        month = current.month % 12 + 1
        year = current.year + (1 if current.month == 12 else 0)
        day = min(current.day, 28)
        return current.replace(year=year, month=month, day=day)
    if recurrence and recurrence.startswith("interval-"):
        try:
            minutes = int(recurrence.split("-", 1)[1])
        except (ValueError, IndexError):
            return None
        if minutes <= 0:
            return None
        return current + timedelta(minutes=minutes)
    return None


def update_task(workspace_path: str, chat_id: str, task_id: str, **changes) -> dict | None:
    tasks = get_chat_tasks(workspace_path, chat_id)
    for task in tasks:
        if task.get("id") == task_id:
            task.update(changes)
            task["updatedAt"] = task_now().isoformat()
            save_chat_tasks(workspace_path, chat_id, tasks)
            return task
    return None


def run_task_background(workspace_id: str, workspace_path: str, chat_id: str, task: dict) -> None:
    task_id = task["id"]
    update_task(workspace_path, chat_id, task_id, status="running", lastRunAt=task_now().isoformat())
    try:
        prompt = task.get("prompt", "")
        attachments = task.get("attachments", [])
        add_message(workspace_path, chat_id, "user", prompt, {"attachments": attachments, "taskId": task_id} if attachments else {"taskId": task_id})
        if workspace_path in ("", GENERAL_WORKSPACE_ID):
            from agents.general_agent import run_general_direct
            for _ in run_general_direct(prompt, chat_id=chat_id, images=[a.get("path") for a in attachments if a.get("isImage") and a.get("path")] or None):
                pass
        else:
            from verify_helper import run_graph_with_verify
            for _ in run_graph_with_verify(prompt, workspace_path=workspace_path, chat_id=chat_id):
                pass
        try:
            from sync.state_sync import safe_get_state
            report = safe_get_state().get("outputs", {}).get("agent_report", "")
        except Exception:
            report = ""
        if report and chat_id:
            add_message(workspace_path, chat_id, "agent", report, {"taskId": task_id})
        recurrence = (task.get("schedule") or {}).get("recurrence", "none")
        current_next = parse_task_time((task.get("schedule") or {}).get("nextRunAt")) or task_now()
        next_run = next_recurring_time(current_next, recurrence)
        if next_run:
            schedule = {**(task.get("schedule") or {}), "nextRunAt": next_run.isoformat()}
            update_task(workspace_path, chat_id, task_id, status="scheduled", schedule=schedule)
        else:
            update_task(workspace_path, chat_id, task_id, status="completed", completedAt=task_now().isoformat())
    except Exception as exc:
        update_task(workspace_path, chat_id, task_id, status="failed", error=str(exc))


def start_task_scheduler() -> None:
    global _started
    with _started_lock:
        if _started:
            return
        _started = True

    def loop() -> None:
        # Lazy one-time prime: pick up scheduled tasks persisted before boot.
        _prime_due_list()
        while True:
            try:
                with _due_lock:
                    snapshot = list(_due_chats.items())
                now = task_now()
                for key, meta in snapshot:
                    workspace_path, chat_id = key
                    try:
                        tasks = get_chat_tasks(workspace_path, chat_id)
                        for task in tasks:
                            due = parse_task_time((task.get("schedule") or {}).get("nextRunAt"))
                            if task.get("status") == "scheduled" and due and due <= now:
                                threading.Thread(
                                    target=run_task_background,
                                    args=(meta.get("workspace_id") or GENERAL_WORKSPACE_ID, workspace_path, chat_id, task.copy()),
                                    daemon=True,
                                ).start()
                    except Exception:
                        continue
            except Exception as exc:
                print(f"[TASK SCHEDULER] {exc}")
            time.sleep(15)

    threading.Thread(target=loop, name="task-scheduler", daemon=True).start()
