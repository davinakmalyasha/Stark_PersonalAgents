"""Delegation tools — the Orchestrator's parallel fan-out surface."""

def delegate(*args, **kwargs) -> str:
    """Spawn one or more workspace agents (Dev/General/House) IN PARALLEL.

    Each item: {agent, task, title?, chat_id?, workspace?}. A new visible chat
    is created per item unless chat_id continues an existing conversation.
    Final replies arrive later as [DELEGATION RESULT] events.

    Native path (C1): when called inside a pydantic-ai run, the first arg is
    RunContext[WorkspaceDeps] and deps.chat_id/project_path are used instead of
    shared_state. Completion delivery prefers RunContext.enqueue when available.
    """
    # C1: detect RunContext as first arg (native tool call with ctx)
    ctx = None
    items = None
    if args and hasattr(args[0], "deps"):
        # Native call: delegate(ctx, items)
        ctx = args[0]
        items = args[1] if len(args) > 1 else kwargs.get("items")
    elif "ctx" in kwargs and hasattr(kwargs["ctx"], "deps"):
        ctx = kwargs.pop("ctx")
        items = kwargs.get("items", args[0] if args else None)
    else:
        # Legacy call: delegate(items)
        items = args[0] if args else kwargs.get("items")

    # Also support items passed as kwargs
    if items is None and "items" in kwargs:
        items = kwargs["items"]

    from agents.delegation import start_delegations

    # Resolve parent chat / workspace from ctx when available (C1)
    parent_chat = ""
    fallback_ws = ""
    if ctx is not None:
        try:
            parent_chat = str(getattr(ctx.deps, "chat_id", "") or "")
            fallback_ws = str(getattr(ctx.deps, "project_path", "") or "")
            # Register enqueue bridge for this chat so child completions can
            # use ctx.enqueue instead of inbox.follow_up when the run is active
            try:
                from server.realtime.enqueue_bridge import register_active_ctx
                register_active_ctx(parent_chat, ctx)
            except Exception:
                pass
        except Exception:
            pass
    if not parent_chat:
        try:
            from tools import shared_state
            parent_chat = str(shared_state.get("chat_id") or "")
            fallback_ws = str(shared_state.get("project_path") or "")
        except Exception:
            pass
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
