"""Delegation tools — the Orchestrator's parallel fan-out surface."""

def delegate(*args, **kwargs) -> str:
    """Spawn one or more workspace agents (Dev/General/House) IN PARALLEL.

    Each item: {agent, task, title?, chat_id?, project?, workspace?, timeout?, images?}.
    A new visible chat is created per item unless chat_id continues an existing
    conversation. project=<registered name> instead continues that project's
    durable Dev session in its own project chat (resume mode — no new tab).
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
        # Launch every item on its own daemon thread and return immediately:
        # the orchestrator parks while they run, and completions arrive as
        # [DELEGATION RESULT] inbox events (enqueue_or_follow_up). Without a
        # parent chat there is no inbox to park on, so run blocking.
        summaries = start_delegations(items, parent_chat, fallback_ws, background=bool(parent_chat))
    except Exception as e:
        return f"Error: delegation failed to start: {e}"

    lines = []
    spawned = 0
    for s in summaries:
        if s.get("ok"):
            spawned += 1
            if s.get("mode") == "resume":
                mode = f"resumed durable Dev in {s['workspace']}/{s['chat_id']}"
            elif s["continued"]:
                mode = f"continued {s['workspace']}/{s['chat_id']}"
            else:
                mode = f"new chat {s['workspace']}/{s['chat_id']}"
            lines.append(f"[SPAWNED] {s['id']} {s['agent']} — {s['title']} ({mode})")
        else:
            lines.append(f"[REJECTED] {s.get('error', 'invalid item')}")
    if not spawned:
        # Deterministic failure contract: a fully-rejected call must read as
        # an error, not a success (see ToolExecutionResult.from_content).
        return "Error: no delegations started.\n" + "\n".join(lines)
    if spawned:
        lines.append("")
        lines.append(
            f"{spawned} agent(s) started (jobs beyond the parallel cap wait as 'queued'). "
            "Do NOT redo their work; their final replies arrive as [DELEGATION RESULT] "
            "events — forward each as a one-liner and summarize when all are in."
        )
    return "\n".join(lines)


def delegation_status(id: str = "", wait_seconds: float = 0, status: str = "") -> str:
    """Job board + drill-down.

    No args: every delegation this session with live statuses (cheap — call
    freely). id=<del-id>: full evidence bundle for one job (task, outcome,
    acceptance, full artifact report, recent chat, file digest). wait_seconds
    (0-300): wait briefly for a job first. status: filter the board
    (queued/running/done/failed/cancelled).
    """
    from agents.delegation import status_table, job_detail, wait_for_job
    del_id = str(id or "").strip()
    if del_id:
        try:
            wait = max(0.0, min(float(wait_seconds or 0), 300.0))
        except (TypeError, ValueError):
            wait = 0.0
        if wait > 0:
            wait_for_job(del_id, wait)
        return job_detail(del_id)
    return status_table(status=status or None)


def _resolve_job(job_id: str):
    """Fetch a job entry snapshot, or (None, error string)."""
    from agents.delegation import get_entry
    del_id = str(job_id or "").strip()
    if not del_id:
        return None, "Error: job_id is required (a del-xxxx id from delegate or delegation_status)."
    e = get_entry(del_id)
    if e is None:
        return None, f"Error: unknown job id '{del_id}'. Call delegation_status() for the board."
    return e, ""


def job_message(job_id: str, message: str) -> str:
    """Steer a RUNNING delegation mid-flight (the user added scope / changed direction).

    Fresh jobs: the message is mirrored into the job's chat and queued on its
    steering channel (the child picks it up at its next step boundary).
    Resume jobs: the message is queued on the durable Dev child (FIFO).
    Steering only works while the job is running — otherwise delegate a
    follow-up instead. Never blocks: queued delivery returns immediately.
    """
    msg = str(message or "").strip()
    if not msg:
        return "Error: message is required."
    e, err = _resolve_job(job_id)
    if e is None:
        return err
    if e.get("status") != "running":
        return (f"Error: job {e['id']} is {e.get('status', 'unknown')} — steering only works "
                "while running. Delegate a follow-up instead.")
    if e.get("mode") == "resume":
        try:
            from agents.subagent_manager import send_message as _sm, child_status as _cs
            st = _cs(e.get("child_id", ""))
            if st.get("status") != "running":
                return (f"Error: job {e['id']}'s Dev turn is not running "
                        f"(child {st.get('status', 'unknown')}) — delegate a follow-up instead.")
            r = _sm(e.get("child_id", ""), msg, e.get("workspace", ""), timeout=30.0)
        except Exception as ex:
            return f"Error: steer failed: {ex}"
        if r.get("status") == "error":
            return str(r.get("result") or "Error: steer failed.")[:1000]
        return f"[OK] Message queued for {e['id']} ({e.get('project', '')} durable Dev; FIFO)."
    try:
        import workspace_manager as wm
        wm.add_message(e["workspace"], e["chat_id"], "user", f"[STEERED by the user via orchestrator]\n{msg}", {
            "delegated": True, "del_id": e["id"], "steered": True,
        })
    except Exception as ex:
        return f"Error: steer failed: {ex}"
    try:
        from inbox import steer
        steer(e["chat_id"], msg)
    except Exception:
        pass
    return f"[OK] Steering sent to {e['id']} ({e['agent_display']} — {e['title']})."


def job_cancel(job_id: str, reason: str = "") -> str:
    """Cancel ONE running delegation (the user said stop).

    Fresh jobs: cooperative abort of the job's chat (queued steering kept).
    Resume jobs: interrupt of the durable Dev child. Settles the entry as
    CANCELLED and reports back as a [DELEGATION RESULT] so the wrap-up stays
    truthful. Already-settled jobs are a no-op.
    """
    from agents.delegation import _LOCK, _DELEGATIONS, _emit_completion
    e, err = _resolve_job(job_id)
    if e is None:
        return err
    if e.get("status") not in ("queued", "running"):
        return f"[OK] Job {e['id']} is already {e.get('status', 'unknown')} — nothing to cancel."
    why = str(reason or "").strip() or "cancelled by the user via orchestrator"
    if e.get("status") == "running" and e.get("mode") == "resume":
        try:
            from agents.subagent_manager import interrupt as _interrupt
            _interrupt(e.get("child_id", ""))
        except Exception:
            pass
    else:
        try:
            from inbox import abort
            abort(e["chat_id"], keep_inbox=True)
        except Exception:
            pass
    try:
        import workspace_manager as wm
        wm.add_message(e["workspace"], e["chat_id"], "user", f"[CANCELLED] {why}", {
            "delegated": True, "del_id": e["id"], "cancelled": True,
        })
    except Exception:
        pass
    with _LOCK:
        cur = _DELEGATIONS.get(e["id"])
        if cur is None or cur.get("status") not in ("queued", "running"):
            return f"[OK] Job {e['id']} already settled."
        cur["status"] = "cancelled"
        cur["error"] = why
        snapshot = dict(cur)
    _emit_completion(snapshot, cur.get("parent", ""))
    return f"[OK] Job {e['id']} cancelled ({why})."
