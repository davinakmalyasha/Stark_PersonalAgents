"""Group-project chat tool (member-only)."""
from __future__ import annotations


def group_message(to: str, message: str) -> str:
    """Send a persisted message to a teammate or the supervisor.

    Delivery is asynchronous: an idle teammate reads it when its next work
    item starts. Use this for coordination, not for asking a human — that is
    ``request_approval``.
    """
    from tools._runtime import active_group_messenger

    sender = active_group_messenger.get()
    if sender is None:
        return "Error: group_message is only available inside a group project run."
    target = str(to or "").strip()
    body = str(message or "").strip()
    if not target:
        return "Error: group_message requires a recipient (a member name or 'supervisor')."
    if not body:
        return "Error: group_message requires a message body."
    try:
        return sender(target, body)
    except Exception as exc:  # noqa: BLE001 - a tool error is data
        return f"Error: {exc}"
