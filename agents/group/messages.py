"""Group chat as a view over real events.

The group project already has an append-only event log. This module makes
agent conversation a first-class part of that log instead of inventing a
second transcript: every chat message is a ``message.sent`` event, and a
reader acknowledgement is a ``message.read`` event.

Delivery is deliberately asynchronous. A member receives unread messages when
its *next* work item starts, inside ``build_prompt``. Nothing here blocks a
running model or promises mid-turn delivery.
"""
from __future__ import annotations

from typing import Any, Iterable

from . import events as ev

#: A message is prose, not a report dump. Keep the event log bounded.
MAX_BODY_CHARS = 4000

MESSAGE_KINDS = frozenset({
    "assignment", "report", "verdict", "approval", "note", "owner",
})

#: Assignment/verdict messages are already represented in the work item prompt
#: and on the board. They are visible in chat but must not be re-injected as
#: "unread teammate messages" on the next run.
NON_DELIVERABLE_KINDS = frozenset({"assignment", "verdict"})


def _clip(body: Any) -> str:
    text = str(body or "").strip()
    if len(text) <= MAX_BODY_CHARS:
        return text
    return text[: MAX_BODY_CHARS - 24].rstrip() + "\n… (message truncated)"


def send(
    store: Any,
    project_id: str,
    *,
    sender: str,
    recipient: str,
    body: str,
    kind: str = "note",
    item_id: str | None = None,
    reply_to: int | None = None,
    deliver: bool | None = None,
) -> int:
    """Append one persisted group message and return its event sequence.

    ``deliver=False`` marks a message as transcript-only. Assignment and
    verdict messages default to that; peer/owner messages default to delivery
    on the recipient's next run.
    """
    sender = str(sender or "").strip() or "unknown"
    recipient = str(recipient or "").strip() or "all"
    kind = str(kind or "note").strip().lower()
    if kind not in MESSAGE_KINDS:
        kind = "note"
    text = _clip(body)
    if not text:
        raise ValueError("a group message needs a body")
    if deliver is None:
        deliver = kind not in NON_DELIVERABLE_KINDS
    data: dict[str, Any] = {
        "from": sender,
        "to": recipient,
        "body": text,
        "kind": kind,
        "deliver": bool(deliver),
    }
    if reply_to is not None:
        data["reply_to"] = int(reply_to)
    return store.append(
        project_id, ev.MESSAGE_SENT, sender, item_id=item_id or None, data=data,
    )


def _record(event: dict) -> dict | None:
    if event.get("kind") != ev.MESSAGE_SENT:
        return None
    data = event.get("data") or {}
    body = str(data.get("body") or "")
    if not body:
        return None
    return {
        "seq": int(event.get("seq") or 0),
        "ts": float(event.get("ts") or 0),
        "from": str(data.get("from") or event.get("actor") or "unknown"),
        "to": str(data.get("to") or "all"),
        "body": body,
        "kind": str(data.get("kind") or "note"),
        "item_id": event.get("item_id") or data.get("item_id"),
        "reply_to": data.get("reply_to"),
        "deliver": bool(data.get("deliver", True)),
    }


def from_events(events: Iterable[dict], limit: int | None = None) -> list[dict]:
    """Normalised messages from an event sequence (oldest first)."""
    out = [record for event in events if (record := _record(event)) is not None]
    if limit is not None and limit > 0:
        out = out[-int(limit):]
    return out


def list_messages(store: Any, project_id: str, limit: int | None = None) -> list[dict]:
    return from_events(store.events(project_id), limit=limit)


def latest_message(store: Any, project_id: str) -> dict | None:
    messages = list_messages(store, project_id)
    return messages[-1] if messages else None


def _read_sequences(events: Iterable[dict], reader: str) -> set[int]:
    read: set[int] = set()
    for event in events:
        if event.get("kind") != ev.MESSAGE_READ:
            continue
        data = event.get("data") or {}
        if str(data.get("reader") or event.get("actor") or "") != reader:
            continue
        try:
            read.add(int(data.get("message_seq") or 0))
        except (TypeError, ValueError):
            continue
    return read


def unread_for(store: Any, project_id: str, recipient: str) -> list[dict]:
    """Messages addressed to ``recipient`` that it has not acknowledged."""
    recipient = str(recipient or "").strip()
    if not recipient:
        return []
    events = store.events(project_id)
    read = _read_sequences(events, recipient)
    out = []
    for message in from_events(events):
        if message["to"] not in (recipient, "all"):
            continue
        if message["from"] == recipient:
            continue
        if not message.get("deliver", True):
            continue
        if message["seq"] in read:
            continue
        out.append(message)
    return out


def mark_read(store: Any, project_id: str, message: dict, reader: str) -> None:
    """Acknowledge one delivered message. Idempotent per reader/message."""
    if not message:
        return
    reader = str(reader or "").strip()
    if not reader:
        return
    events = store.events(project_id)
    if int(message.get("seq") or 0) in _read_sequences(events, reader):
        return
    store.append(
        project_id,
        ev.MESSAGE_READ,
        reader,
        item_id=message.get("item_id") or None,
        data={"message_seq": int(message.get("seq") or 0), "reader": reader},
    )


def render_prompt(messages: Iterable[dict]) -> str:
    """Render unread messages for a member's work-item prompt."""
    lines = ["## Messages from your group", ""]
    for message in messages:
        lines.append(
            f"- [{message.get('from')} → {message.get('to')}] "
            f"{message.get('body')}"
        )
    return "\n".join(lines).strip()


def preview(message: dict | None) -> dict | None:
    """Small shape for project list/preview cards."""
    if not message:
        return None
    return {
        "seq": message.get("seq"),
        "ts": message.get("ts"),
        "from": message.get("from"),
        "to": message.get("to"),
        "body": message.get("body"),
        "kind": message.get("kind"),
        "item_id": message.get("item_id"),
    }
