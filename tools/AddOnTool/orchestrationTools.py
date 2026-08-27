"""AddOnTool subagent, user-question, and project-routing tools."""

def send_message(child_id: str, message: str, timeout: int = 1800) -> str:
    """Deliver a follow-up turn to a continuable subagent (P6).

    A running child queues the message (FIFO); an idle child resumes from its
    durable session log and runs the turn now. Returns the child's reply.
    """
    from tools import shared_state, _HARNESS_ROOT
    from agents.subagent_manager import send_message as _sm
    ws = shared_state.get("project_path") or _HARNESS_ROOT
    try:
        r = _sm(child_id, message, ws, timeout=float(timeout or 1800))
    except Exception as e:
        return f"Error: {e}"
    if r.get("queued"):
        return f"[OK] Message queued for {child_id} (child is running; it will process it at its next step)."
    if r.get("status") == "error":
        return r.get("result", f"Error: unknown continuable child '{child_id}'")
    reply = r.get("result", "")
    return f"[OK] {child_id} replied:\n{reply}"[:4000]

def list_agents() -> str:
    """List continuable subagents (durable enumeration, no resume)."""
    from tools import shared_state, _HARNESS_ROOT
    from agents.subagent_manager import list_agents as _la
    ws = shared_state.get("project_path") or _HARNESS_ROOT
    chat_id = str(shared_state.get("chat_id") or "")
    try:
        agents = _la(ws, chat_id)
    except Exception as e:
        return f"Error: {e}"
    if not agents:
        return "[OK] No continuable subagents for this chat."
    lines = []
    for a in agents:
        label = a.get("label") or a.get("name") or a["child_id"]
        lines.append(f"- {a['child_id']} ({label}) — {a['status']}")
    return "[CONTINUABLE SUBAGENTS]\n" + "\n".join(lines)

def interrupt_agent(child_id: str) -> str:
    """Cooperatively interrupt a running continuable subagent (keeps queued msgs)."""
    from agents.subagent_manager import interrupt
    try:
        ok = interrupt(child_id)
    except Exception as e:
        return f"Error: {e}"
    return f"[OK] Interrupt requested for {child_id}." if ok else f"Error: unknown child '{child_id}'"

def report(message: str, quiet: bool = False) -> str:
    """Report back to the parent agent (continuable subagents only).

    Default (wakeup) queues a follow-up to the parent chat; quiet=1 records
    the report without waking the parent.
    """
    from agents.subagent_manager import report as _report
    try:
        return _report(message, quiet=bool(quiet))
    except Exception as e:
        return f"Error: {e}"

def ask_user(questions: list, timeout: int = None) -> str:
    """Ask the human one or more structured questions and wait for answers.

    Blocks until the user answers (or the timeout aborts). Supports the
    plan-review intent: pass intent={"kind": "plan-review", "approve": "Yes"}.
    """
    from user_questions import ask, UserQuestionError
    from sync.state_sync import shared_state as _ss
    chat_id = str(_ss.get("chat_id") or "")
    try:
        answers = ask(questions, timeout=timeout, chat_id=chat_id)
    except UserQuestionError as e:
        return f"Error: {e.code}: {e}"
    lines = []
    for a in answers:
        parts = []
        if a.selected:
            parts.append(", ".join(a.selected))
        if a.custom:
            parts.append(f"custom: {a.custom}")
        lines.append(f"- {a.id}: {('; '.join(parts)) if parts else '(no answer)'}")
    return "[USER ANSWERS]\n" + "\n".join(lines)

def create_project(name: str, concept: str, wait: bool = True) -> str:
    """Create a NEW project: folder + workspace + chat + durable Dev agent.

    Orchestrator-only tool. Never call for work on an existing project.
    wait=false runs the Dev's first turn in the background — use it when
    creating SEVERAL projects in one burst (they spin up in parallel).
    """
    from tools import shared_state
    from agents.project_router import create_project as _cp
    chat_id = str(shared_state.get("chat_id") or "")
    try:
        entry = _cp(name, concept, chat_id, wait=bool(wait))
    except Exception as e:
        return f"Error: {e}"
    if entry.get("error"):
        return f"Error: {entry['error']}"
    if entry.get("already_existed"):
        return (
            f"[OK] Project '{entry.get('name')}' already has a durable Dev agent.\n"
            f"Chat: {entry.get('chat_id')}\n"
            f"Dev agent: {entry.get('child_id')}\n"
            "Use resume_project(name, message) to continue it."
        )
    lines = [
        f"[OK] Project '{entry.get('name')}' created.",
        f"Folder: {entry.get('workspace_path')}",
        f"Workspace: {entry.get('workspace_id')}",
        f"Chat: {entry.get('chat_id')}",
    ]
    if entry.get("background"):
        lines += [
            "Dev agent: starting in the background — its first report lands in the project chat.",
            "",
            "Continue creating any other projects now; do not wait for this one.",
        ]
    else:
        lines += [
            f"Dev agent: {entry.get('child_id')}",
            "",
            "Dev's first report:",
            str(entry.get("first_reply", ""))[:2000],
        ]
    return "\n".join(lines)

def list_projects_tool() -> list:
    from agents.project_router import list_projects as _lp
    try:
        return _lp()
    except Exception:
        return []

def resume_project(name: str, message: str, timeout: int = 1800) -> str:
    """One-call resume: resolve project and deliver message to its durable Dev.

    Replaces find_project_dev + send_message (+ list_agents for discovery).
    Returns the Dev reply or a [NOT FOUND] with available project names.
    """
    from tools import shared_state, _HARNESS_ROOT
    from agents.project_router import find_project_dev as _find
    from agents.subagent_manager import send_message as _sm
    # resolve
    try:
        info = _find(name)
    except Exception as e:
        return f"Error: {e}"
    if not info:
        available = ", ".join(p.get("name", "") for p in list_projects_tool()) or "none"
        return f"[NOT FOUND] No registered project matches '{name}'. Available projects: {available}"
    child_id = info.get("child_id", "")
    if not child_id:
        return f"Error: project '{info.get('name')}' has no durable Dev agent."
    ws = shared_state.get("project_path") or _HARNESS_ROOT
    # Prefer the project's own workspace if available
    ws = info.get("workspace_path") or ws
    try:
        r = _sm(child_id, message, ws, timeout=float(timeout or 1800))
    except Exception as e:
        return f"Error: {e}"
    if r.get("queued"):
        return f"[OK] Message queued for {child_id} (child is running; it will process it at its next step)."
    if r.get("status") == "error":
        return r.get("result", f"Error: unknown continuable child '{child_id}'")
    reply = r.get("result", "")
    return f"[OK] {child_id} replied:\n{reply}"[:4000]
