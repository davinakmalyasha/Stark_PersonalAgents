"""Subagent context pack — bounded project context for specialist subagents.

Specialist subagents (UI, DevOps, …) are spawned with only a task string
today, so they re-discover the workspace from scratch. This module assembles a
compact context block once at spawn time:

  - authored project memory (project.md Identity + Design bullets)
  - latest project map (full snapshot, already size-bounded upstream)
  - parent goal (the client request the parent agent is executing)
  - parent plan (update_plan state, when the parent maintains one)
  - latest failure/test report (so a verifier knows what already broke)

The block is injected as its own user message BEFORE the task message, so the
subagent's own transcript stays append-only and the stable system prompt is not
invalidated. Assembly is local work: no LLM calls, no extra latency.

Never raises: every section is best-effort and simply omitted when unavailable.
"""
from __future__ import annotations

import os


def _read_project_map(workspace: str) -> str:
    try:
        from memory.memory_manager import refresh_code_map_context
        ev = refresh_code_map_context(workspace, include_cached_full=True)
        return str((ev or {}).get("content") or "").strip()
    except Exception:
        pass
    try:
        from memory.memory_manager import load_cached_code_map
        return str(load_cached_code_map(workspace) or "").strip()
    except Exception:
        return ""


def _read_parent_state() -> dict:
    try:
        from sync.state_sync import safe_get_state
        return dict(safe_get_state() or {})
    except Exception:
        return {}


def _read_plan(state: dict, name: str = "") -> str:
    try:
        from tools import get_plan_for_context
        from agents.workspace_registry import get_spec

        spec = get_spec(name) if name else None
        scope = (spec.plan_scope if spec and spec.plan_scope else "developer")
        chat_id = str(state.get("chat_id") or "")
        # The parent that spawns specialists is the Developer workspace agent;
        # the registry's plan_scope names the plan owner for each specialist.
        return str(get_plan_for_context(scope, chat_id) or "").strip()
    except Exception:
        return ""


def build_context_pack(name: str = "", max_chars: int = 60000) -> str:
    """Build the context block for a specialist subagent spawn.

    Returns "" when nothing useful is available (the caller then injects
    nothing). ``max_chars`` is a final safety bound only — the map and memory
    are already size-bounded upstream; the pack intentionally does not
    micro-cap section content.
    """
    state = _read_parent_state()
    workspace = str(state.get("project_path") or "") or os.environ.get("HARNESS_ROOT", "")

    sections: list[str] = []

    sections.append(f"## Workspace root\n{workspace}")

    # Authored project memory (project.md): identity + design bullets. A UI
    # spawn on a project with no memory yet gets the authoring trigger
    # (content stays LLM-authored by the specialist).
    try:
        from memory.memory_manager import load_project_memory
        project_memory = load_project_memory(workspace)
    except Exception:
        project_memory = ""
    if project_memory:
        sections.append("## Project memory (project.md — honor these)\n" + project_memory)
    elif str(name or "").strip().lower() == "ui":
        sections.append(
            "## Project memory (not authored yet)\n"
            "No `.agent-memory/project.md` exists. As the UI specialist, "
            "author/update the `## Design` section (direction, theme, palette, "
            "typography, concept) via edit_file before finishing; never touch "
            "`## Identity`."
        )

    goal = str(state.get("client_request") or "").strip()
    if goal:
        sections.append("## Parent goal (in progress)\n" + goal[:4000])

    plan = _read_plan(state, name)
    if plan:
        sections.append("## Parent plan\n" + plan)

    outputs = state.get("outputs") or {}
    test_report = str(outputs.get("test_report") or "").strip()
    if test_report:
        sections.append("## Latest verification / failure report\n" + test_report[:6000])

    project_map = _read_project_map(workspace)
    if project_map:
        sections.append("## Project map\n" + project_map)

    if not sections:
        return ""

    header = (
        f"[CONTEXT PACK for subagent {name or 'specialist'}] Background captured at spawn time. "
        "It is context, not orders: follow the task message that follows, and re-read files you "
        "will modify before editing them."
    )
    text = header + "\n\n" + "\n\n".join(sections)
    if max_chars and len(text) > max_chars:
        text = text[:max_chars] + "\n...[context pack truncated]"
    return text
