"""Orchestrator-owned sticky notes for chats and group projects."""

from __future__ import annotations

from server.store import agent_notes


def _target(value: str) -> tuple[str, str]:
    text = str(value or "").strip()
    if text.startswith("chat:") or text.startswith("group:"):
        kind, ident = text.split(":", 1)
        return kind, ident
    from tools import shared_state

    chat_id = str(shared_state.get("chat_id") or "")
    if chat_id:
        return "chat", chat_id
    return "", text


def _format(notes: list[dict]) -> str:
    if not notes:
        return "[OK] No notes."
    lines = ["[NOTES]"]
    for note in notes:
        lines.append(f"- {note.get('id')} · {note.get('title')}")
        if note.get("body"):
            lines.append(f"  {note.get('body')}")
        for todo in note.get("todos") or []:
            mark = "x" if todo.get("done") else " "
            lines.append(f"  [{mark}] {todo.get('text')}")
    return "\n".join(lines)


def give_note(action: str = "create", target: str = "", title: str = "",
              body: str = "", todos=None, note_id: str = "") -> str:
    """Create, update, list, or remove a sticky note attached to a chat or group."""
    action = (action or "create").strip().lower()
    kind, ident = _target(target)
    try:
        if action == "create":
            if not kind or not ident:
                return "Error: no target chat/group."
            note = agent_notes.create_note(kind, ident, title or "Note", body, todos)
            return f"[OK] Note created: {note.get('id')}\n{_format([note])}"
        if action == "update":
            if not note_id:
                return "Error: note_id is required to update."
            note = agent_notes.update_note(
                note_id,
                title=title if title else None,
                body=body if body else None,
                todos=todos if todos is not None else None,
            )
            if not note:
                return f"Error: unknown note '{note_id}'."
            return f"[OK] Note updated.\n{_format([note])}"
        if action == "remove":
            if not note_id:
                return "Error: note_id is required to remove."
            return f"[OK] Note removed." if agent_notes.delete_note(note_id) else f"Error: unknown note '{note_id}'."
        if action == "list":
            if not kind or not ident:
                return "Error: no target chat/group."
            return _format(agent_notes.list_notes(kind, ident))
        return f"Error: unknown action '{action}' (create|update|remove|list)."
    except Exception as exc:
        return f"Error: {exc}"
