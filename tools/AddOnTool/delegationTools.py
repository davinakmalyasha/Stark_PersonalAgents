"""Delegation tools — the Orchestrator's parallel fan-out surface."""

def delegate(items: list) -> str:
    """Spawn one or more workspace agents (Dev/General/House) IN PARALLEL.

    Each item: {agent, task, title?, chat_id?, workspace?}. A new visible chat
    is created per item unless chat_id continues an existing conversation.
    Final replies arrive later as [DELEGATION RESULT] events.
    """
    from tools import shared_state
    from agents.delegation import start_delegations

    if isinstance(items, dict):
        items = [items]
    if not items or not isinstance(items, list):
        return "Error: delegate requires 'items' — a list of {agent, task, title?} objects."

    parent_chat = str(shared_state.get("chat_id") or "")
    fallback_ws = shared_state.get("project_path") or ""
    try:
        summaries = start_delegations(items, parent_chat, fallback_ws)
    except Exception as e:
        return f"Error: delegation failed to start: {e}"

    lines = []
    spawned = 0
    for s in summaries:
        if s.get("ok"):
            spawned += 1
            mode = f"continued {s['workspace']}/{s['chat_id']}" if s["continued"] \
                else f"new chat {s['workspace']}/{s['chat_id']}"
            lines.append(f"[SPAWNED] {s['id']} {s['agent']} — {s['title']} ({mode})")
        else:
            lines.append(f"[REJECTED] {s.get('error', 'invalid item')}")
    if spawned:
        lines.append("")
        lines.append(
            f"{spawned} agent(s) running in parallel. Do NOT redo their work; "
            "their final replies arrive as [DELEGATION RESULT] events — forward each "
            "as a one-liner and summarize when all are in."
        )
    return "\n".join(lines)


def delegation_status() -> str:
    """Show all delegations spawned this session with live statuses."""
    from agents.delegation import status_table
    return status_table()
