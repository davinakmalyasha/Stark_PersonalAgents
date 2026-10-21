"""Canvas mode: watch selected agent chats and wake the orchestrator."""

from __future__ import annotations

from server.services import canvas_watch


def _ctx() -> tuple[str, str]:
    from tools import shared_state

    chat_id = str(shared_state.get("chat_id") or "")
    workspace_path = str(shared_state.get("project_path") or "")
    if workspace_path == "_general":
        workspace_path = ""
    return chat_id, workspace_path


def _format_snapshot(data: dict) -> str:
    watch = data.get("watch")
    if not watch:
        return "[OK] Canvas mode is off."
    lines = [
        f"[CANVAS MODE] {watch.get('mode')} · {watch.get('actionsUsed')}/{watch.get('maxActions')} actions",
        f"Targets: {', '.join(watch.get('targets') or []) or 'all agent chats'}",
    ]
    for chat in data.get("chats") or []:
        state = "running" if chat.get("running") else (chat.get("lastRole") or "idle")
        lines.append(f"- chat {chat.get('id')} · {chat.get('title')} · {state}")
    for group in data.get("groups") or []:
        lines.append(
            f"- group {group.get('id')} · {group.get('title')} · {group.get('status')} · "
            f"items {group.get('itemsDone')}/{group.get('itemsTotal')}"
        )
    for event in data.get("events") or []:
        lines.append(f"  event {event.get('kind')} · {event.get('sourceChatId')}")
    return "\n".join(lines)


def canvas_mode(action: str = "status", mode: str = "all", targets=None,
                wake_on=None, max_actions: int = 20, until: str = "",
                target: str = "", message: str = "") -> str:
    """Enter, inspect, message, or exit canvas oversight mode."""
    chat_id, workspace_path = _ctx()
    if not chat_id:
        return "Error: no orchestrator chat is active."
    action = (action or "status").strip().lower()
    try:
        if action == "available":
            data = canvas_watch.available()
            lines = ["[CANVAS TARGETS]"]
            for chat in data.get("chats") or []:
                lines.append(f"- chat:{chat.get('id')} · {chat.get('title')} · {chat.get('lastRole') or 'idle'}")
            for group in data.get("groups") or []:
                lines.append(f"- group:{group.get('id')} · {group.get('title')} · {group.get('status')}")
            if len(lines) == 1:
                lines.append("(no agent chats or groups)")
            return "\n".join(lines)
        if action == "enter":
            data = canvas_watch.enter(
                chat_id, workspace_path, mode=mode, targets=targets or [],
                wake_on=wake_on or ["completed", "failed"],
                max_actions=max_actions, until=until or None,
            )
            return "[OK] Canvas mode on.\n" + _format_snapshot(data)
        if action in ("status", "read"):
            return _format_snapshot(canvas_watch.read(chat_id))
        if action == "exit":
            count = canvas_watch.exit(chat_id)
            return f"[OK] Canvas mode off ({count} watch closed)."
        if action == "message":
            if not target or not message:
                return "Error: target and message are required."
            result = canvas_watch.message(chat_id, target, message)
            return (
                f"[OK] Message queued for {target} "
                f"({result.get('actionsUsed')}/{result.get('maxActions')} actions)."
            )
        return f"Error: unknown action '{action}' (available|enter|status|read|message|exit)."
    except Exception as exc:
        return f"Error: {exc}"
