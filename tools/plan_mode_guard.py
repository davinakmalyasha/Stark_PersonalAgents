"""
Stage 4: plan-mode guard.

User authority: plan/build mode is decided ONLY by the user via the prompt
dock (POST /api/chat/{id}/mode). The agent can never change the mode in
either direction — no tool exposes a mode switch to the LLM.

When a chat is in `plan` mode, write-class tools are vetoed. The agent
can call `propose_plan` to present the plan for the user-initiated review,
but cannot transition out of plan mode — only the user (prompt dock or
`/build` slash alias) flips the mode back to `build`.

Read-class tools remain available: read_file, search_code, browser_*,
read_pdf, ask_user, update_plan, propose_plan, plus all the existing
read-only registry calls.

Write-class tool blocklist (deliberately narrow): edit_file, create_project,
github_apply_gate, house_set_device, house_run_scene,
create_docx, create_xlsx, delegate, call_subagent.

run_command is not blanket-blocked in plan mode: each call is checked by
`_is_destructive_shell` (delegates to MainTool._cmdpolicy.is_read_only), so
read-only shells stay available while writes/side effects are vetoed.
"""

from __future__ import annotations

# Tools blocked in plan mode.
WRITE_CLASS_TOOLS: frozenset[str] = frozenset({
    "edit_file", "create_project",
    "github_apply_gate", "house_set_device", "house_run_scene",
    "create_docx", "create_xlsx",
    "delegate", "call_subagent",
})


def _is_destructive_shell(command: str) -> bool:
    """Plan mode allows *read-only* shell commands (ls, cat, find, grep).

    Delegates to tools/MainTool/_cmdpolicy.is_read_only: a command is allowed
    only when every segment parses to an allowlisted read-only program, has no
    output redirection, and contains no nested shell. Anything unverifiable is
    treated as destructive.
    """
    try:
        from .MainTool._cmdpolicy import is_read_only
        ok, _reason = is_read_only(command)
        return not ok
    except Exception:
        # Conservative fallback: unknown command shape is vetoed.
        return bool(command)


def plan_mode_guard(ctx: dict) -> str | None:
    """Returns a deny-reason string when the tool call is blocked, or
    None to allow. Callers (native hook / registry guard) convert the
    non-empty string into `[BLOCKED] <reason>`.
    """
    try:
        from tools.chat_mode import get_mode
    except Exception:
        return None

    chat_id = ""
    try:
        deps = ctx.get("deps")
        if deps is not None:
            chat_id = str(getattr(deps, "chat_id", "") or "")
    except Exception:
        chat_id = ""
    if not chat_id:
        try:
            from sync.state_sync import safe_get_state
            chat_id = str((safe_get_state() or {}).get("chat_id") or "")
        except Exception:
            chat_id = ""
    if not chat_id or get_mode(chat_id) != "plan":
        return None

    name = ctx.get("name", "")
    args = ctx.get("args") or {}

    # run_command: per-command destructive check.
    if name == "run_command":
        if _is_destructive_shell(str(args.get("command", ""))):
            return ("Plan mode is read-only. Type /build to leave plan mode. "
                    "(run_command with write/side-effect verbs is blocked; "
                    "use read-only shell like ls/cat/find/grep.)")
        return None

    # Everything else: blocklist lookup.
    if name in WRITE_CLASS_TOOLS:
        return f"Plan mode is read-only. Type /build to leave plan mode. ({name} is a write-class tool.)"

    # Allow everything else (read_file, search_code, browser_*, ask_user,
    # update_plan, propose_plan, read_pdf, etc.).
    return None
