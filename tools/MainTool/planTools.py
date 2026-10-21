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
    MAX_STEP_CHARS,
    save_plan,
    plan_state,
    load_plan,
)


def _resolve_scope(ctx: object = None) -> str:
    """Scope of the caller (Developer / Orchestrator / ...).

    Prefers the typed per-run ``ctx`` (pydantic-ai ``RunContext.deps.role``
    or the loop-passed WorkspaceDeps); falls back to the legacy scope name.
    """
    if ctx is not None:
        try:
            role = getattr(ctx, "role", None)
            if role:
                return str(role).lower()
        except Exception:
            pass
    return "agent"


def _resolve_chat(ctx: object = None) -> str:
    """Chat key for the plan store: the typed per-run ctx chat first, else
    the shared-state chat (developer_node / orchestrator paths)."""
    if ctx is not None:
        try:
            cid = getattr(ctx, "chat_id", None)
            if cid:
                return str(cid)
        except Exception:
            pass
    try:
        from sync.state_sync import safe_get_state
        return str((safe_get_state() or {}).get("chat_id") or "")
    except Exception:
        return ""


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

def _clean_status(value, notes: list) -> str:
    status = str(value or "pending").strip().lower()
    if status not in PLAN_STATUSES:
        notes.append(f"invalid status '{value}' (want pending|in_progress|completed) — used pending")
        return "pending"
    return status


def _apply_ops(current: list[dict], ops) -> tuple[list[dict], list[str]]:
    """Patch loaded plan items with incremental ops. Returns (items, notes)."""
    working = [dict(it) for it in (current or []) if isinstance(it, dict)]
    notes: list[str] = []
    for op in (ops or []):
        if not isinstance(op, dict):
            notes.append("skipped a non-object op")
            continue
        kind = str(op.get("op", "")).strip().lower()
        if kind == "add":
            step = str(op.get("step", "")).strip()
            if not step:
                notes.append("add needs 'step'")
                continue
            working.append({"step": step[:MAX_STEP_CHARS],
                            "status": _clean_status(op.get("status", "pending"), notes)})
            continue
        if kind in ("set_status", "rename", "remove"):
            match = str(op.get("match", "")).strip().lower()
            if not match:
                notes.append(f"{kind} needs 'match'")
                continue
            targets = [it for it in working if match in str(it.get("step", "")).lower()]
            if not targets:
                notes.append(f"no step matches '{op.get('match', '')}'")
                continue
            if kind == "remove":
                working = [it for it in working if not any(it is t for t in targets)]
            elif kind == "set_status":
                status = _clean_status(op.get("status"), notes)
                for t in targets:
                    t["status"] = status
            else:
                new_step = str(op.get("step", "")).strip()
                if not new_step:
                    notes.append("rename needs 'step' (new text)")
                    continue
                for t in targets:
                    t["step"] = new_step[:MAX_STEP_CHARS]
            continue
        notes.append(f"unknown op '{op.get('op', '')}' (want add|set_status|rename|remove)")
    return working, notes


def update_plan(explanation: str = "", plan: list = None, ops: list = None, ctx: object = None) -> str:
    """Update the task plan (codex-style full replacement) or patch it (ops).

    Full mode: provide the entire plan array. At most one step may be
    `in_progress`; extras are demoted to `pending`.
    Incremental mode: pass ops instead of plan — [{op, match?, step?, status?}]
    with op = add (needs step) | set_status (needs match+status) |
    rename (needs match+step) | remove (needs match). match is a
    case-insensitive substring of the step text. Small tweaks cost a fraction
    of a full rewrite; use full mode when the plan shape itself changes.
    """
    from tools import shared_state, WORKSPACE
    from sync.state_sync import safe_update_state, safe_append_live_log

    if plan is not None and ops is not None:
        return "Error: pass either plan (full replacement) or ops (incremental), not both."

    scope = _resolve_scope(ctx)
    project_path = shared_state.get("project_path") or WORKSPACE
    chat_id = _resolve_chat(ctx) or shared_state.get("chat_id") or ""

    if ops is not None:
        patched, patch_notes = _apply_ops(load_plan(project_path, chat_id, scope), ops)
        items, errors = _normalize_plan(patched)
        errors = patch_notes + errors
        if not items:
            return "Error: ops left the plan empty" + (
                f": {'; '.join(errors)}" if errors else "")
    else:
        items, errors = _normalize_plan(plan)
        if not items:
            return "Error: update_plan requires a non-empty plan" + (
                f": {'; '.join(errors)}" if errors else " with valid steps"
            )

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


def get_plan_for_context(scope: str = "", chat_id: str = "", ctx: object = None) -> str:
    """Return the current plan block for a scope for LLM re-injection, or ''.

    ``chat_id`` may be given explicitly (child session's own chat); otherwise
    the typed ctx chat or shared-state chat is used.
    """
    from tools import shared_state, WORKSPACE
    from sync.sync_helpers import build_plan_progress_block, load_task_tracking

    project_path = shared_state.get("project_path") or WORKSPACE
    if not chat_id:
        chat_id = _resolve_chat(ctx) or shared_state.get("chat_id") or ""
    task_data = load_task_tracking(project_path, chat_id)
    if not task_data:
        return ""
    return build_plan_progress_block(task_data, scope)