"""Orchestrator-only scheduled wakes."""

from __future__ import annotations

from server.store import schedules as _schedules
from server.store.chat_store import get_chat_workspace_path


def _ctx() -> tuple[str, str]:
    from tools import shared_state

    chat_id = str(shared_state.get("chat_id") or "")
    workspace_path = get_chat_workspace_path(chat_id) or ""
    if workspace_path == "_general":
        workspace_path = ""
    return chat_id, workspace_path


def _format(item: dict) -> str:
    return (
        f"- {item.get('id')} · {item.get('name')} · {item.get('nextRunAt')} · "
        f"{item.get('recurrence')} · {item.get('status')}"
    )


def schedule(action: str = "create", name: str = "", at: str = "",
             prompt: str = "", recurrence: str = "none",
             schedule_id: str = "") -> str:
    """Create, list, or cancel a time-triggered wake for this orchestrator chat."""
    action = (action or "create").strip().lower()
    chat_id, workspace_path = _ctx()
    if not chat_id:
        return "Error: no orchestrator chat is active."
    try:
        if action == "create":
            if not at.strip():
                return "Error: at is required (ISO timestamp or HH:MM)."
            label = (name or "Scheduled wake").strip()
            item = _schedules.create_schedule(
                label,
                (prompt or label).strip(),
                workspace_path,
                chat_id,
                at,
                recurrence=recurrence,
            )
            return (
                f"[OK] Scheduled '{item.get('name')}' for {item.get('nextRunAt')} "
                f"({item.get('recurrence')}). It will wake this chat. Id: {item.get('id')}"
            )
        if action == "list":
            items = _schedules.list_schedules(chat_id=chat_id)
            if not items:
                return "[OK] No active schedules for this chat."
            return "[SCHEDULES]\n" + "\n".join(_format(item) for item in items)
        if action == "cancel":
            if not schedule_id:
                return "Error: schedule_id is required to cancel."
            if not _schedules.cancel_schedule(schedule_id):
                return f"Error: unknown schedule '{schedule_id}'."
            return f"[OK] Cancelled {schedule_id}."
        return f"Error: unknown action '{action}' (create|list|cancel)."
    except Exception as exc:
        return f"Error: {exc}"
