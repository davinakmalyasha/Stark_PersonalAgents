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

Write-class tool blocklist (deliberately narrow, per user direction
"read tools only" — no edit_file, no destructive shell):

  edit_file, run_command (any), create_project,
  github_apply_gate, house_set_device, house_run_scene,
  create_docx, create_xlsx, delegate, task, call_subagent,
  async_task, resume_project, interrupt_agent.
"""

from __future__ import annotations

# Tools blocked in plan mode.
WRITE_CLASS_TOOLS: frozenset[str] = frozenset({
    "edit_file", "create_project", "resume_project",
    "github_apply_gate", "house_set_device", "house_run_scene",
    "house_apply_state", "create_docx", "create_xlsx",
    "delegate", "delegation_status",
    "task", "call_subagent", "async_task", "start_async_task",
    "interrupt_agent",
    "create_chat", "add_message", "delete_chat", "delete_workspace",
    "update_feature", "update_project_meta",
})


def _is_destructive_shell(command: str) -> bool:
    """Plan mode allows *read-only* shell commands (ls, cat, find, grep).
    Anything that looks like a write or remote/exec is vetoed."""
    if not command:
        return False
    cmd = command.strip().lower()
    # Read-only allowlist (very small; safer to veto anything else).
    read_only_prefixes = (
        "ls", "dir", "cat", "type", "head", "tail", "find", "grep",
        "rg ", "git status", "git log", "git diff", "git show",
        "git branch", "git remote -v", "git ls-files",
        "where ", "which ", "echo ", "printf ",
        "python -c ", "python3 -c ",
    )
    for prefix in read_only_prefixes:
        if cmd == prefix.rstrip() or cmd.startswith(prefix):
            # Even read-only allowlist can be tricked — block obvious writes.
            for bad in (">", ">>", "rm ", "del ", "mv ", "cp ", "move ",
                        "copy ", "tee ", "invoke-webrequest", "curl ",
                        "wget ", "invoke-restmethod", "bitsadmin",
                        "certutil", "reg ", "schtasks", "sc ",
                        "format-", "clear-disk", "initialize-disk",
                        "remove-item", "set-content", "add-content",
                        "new-item -type file", "ni -type file",
                        "out-file", "sc create", "stop-service",
                        "start-service", "restart-service", "kill ",
                        "taskkill", "pkill", "remove-variable",
                        "set-variable ", "$env:", "[environment]",
                        "pip install", "npm install", "npm i ",
                        "cargo install", "go install", "brew install",
                        "gem install", "iex ", "invoke-expression",
                        "sudo ", "runas ", "icacls ", "takeown ",
                        "format-volume"):
                if bad in cmd:
                    return True
            return False
    return True  # anything not in the read-only allowlist is vetoed


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
