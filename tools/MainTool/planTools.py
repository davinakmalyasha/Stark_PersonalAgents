"""MainTool.planTools — the codex-style `update_plan` tool.

The plan is LLM-driven. The model writes the entire plan array on each call
(full replacement); the harness only persists it per scope, re-injects it on
resume, and mirrors it into UI state. There is no harness-side step machine.

Schema follows OpenAI codex `update_plan`:
    update_plan(explanation?: string, plan: [{step, status}])
with at most one step `in_progress` at a time.
"""
from ._shared import _log_tool

from sync.sync_helpers import (
    PLAN_STATUSES,
    MAX_PLAN_STEPS,
    MAX_STEP_CHARS,
    save_plan,
    plan_state,
)


def _resolve_scope() -> str:
    """Scope of the caller (Developer / Orchestrator / ...)."""
    from tools._registry import _current_scope
    return (_current_scope.get() or "agent").lower()


def _resolve_chat() -> str:
    """Chat key for the plan store: the active child session's own chat first,
    else the shared-state chat (developer_node / orchestrator paths)."""
    from tools._registry import _current_chat
    return _current_chat.get() or ""


def _normalize_plan(plan) -> tuple[list[dict], list[str]]:
    """Validate + cap a raw plan array. Returns (items, warnings)."""
    items = []
    errors = []
    for it in (plan or []):
        if not isinstance(it, dict):
            errors.append("plan items must be objects with 'step' and 'status'")
            continue
        step = str(it.get("step", "")).strip()
        if not step:
            continue
        status = str(it.get("status", "pending")).strip().lower()
        if status not in PLAN_STATUSES:
            errors.append(f"invalid status '{status}' (must be pending|in_progress|completed)")
            continue
        items.append({"step": step[:MAX_STEP_CHARS], "status": status})
        if len(items) >= MAX_PLAN_STEPS:
            break
    if not items:
        return [], errors or ["plan must contain at least one step"]
    in_progress = [i for i in items if i["status"] == "in_progress"]
    if len(in_progress) > 1:
        for it in in_progress[1:]:
            it["status"] = "pending"
        errors.append(
            f"demoted {len(in_progress) - 1} extra in_progress step(s) to pending (at most one allowed)"
        )
    return items, errors


def update_plan(explanation: str = "", plan: list = None) -> str:
    """Update the task plan (codex-style full replacement).

    Provide an optional explanation and the entire plan array. At most one step
    may be `in_progress`; extras are demoted to `pending`.
    """
    from tools import shared_state, WORKSPACE
    from sync.state_sync import safe_update_state, safe_append_live_log

    scope = _resolve_scope()
    items, errors = _normalize_plan(plan)
    if not items:
        return "Error: update_plan requires a non-empty plan" + (
            f": {'; '.join(errors)}" if errors else " with valid steps"
        )

    project_path = shared_state.get("project_path") or WORKSPACE
    chat_id = _resolve_chat() or shared_state.get("chat_id") or ""
    save_plan(items, project_path, chat_id, scope)

    completed = sum(1 for it in items if it["status"] == "completed")
    in_progress = sum(1 for it in items if it["status"] == "in_progress")
    note = (" (" + "; ".join(errors) + ")") if errors else ""
    safe_update_state({"plan": plan_state(project_path, chat_id)})
    safe_append_live_log(
        f"[PLAN] {scope}: {completed}/{len(items)} completed, {in_progress} in progress{note}"
    )
    _log_tool(
        "update_plan",
        {"scope": scope, "explanation": (explanation or "")[:100], "steps": len(items)},
        f"Plan updated ({completed}/{len(items)} completed)",
    )
    return f"Plan updated ({completed}/{len(items)} completed, {in_progress} in progress){note}"


def get_plan_for_context(scope: str = "", chat_id: str = "") -> str:
    """Return the current plan block for a scope for LLM re-injection, or ''.

    ``chat_id`` may be given explicitly (child session's own chat); otherwise
    the active child chat or shared-state chat is used.
    """
    from tools import shared_state, WORKSPACE
    from sync.sync_helpers import build_plan_progress_block, load_task_tracking

    project_path = shared_state.get("project_path") or WORKSPACE
    if not chat_id:
        chat_id = _resolve_chat() or shared_state.get("chat_id") or ""
    task_data = load_task_tracking(project_path, chat_id)
    if not task_data:
        return ""
    return build_plan_progress_block(task_data, scope)