"""
Stage 4: propose_plan — agent-callable plan proposal (plan mode ONLY).

User authority: plan/build mode is decided ONLY by the user via the prompt
dock. The agent can never enter plan mode by itself — this tool is a no-op
outside plan mode so a build-mode agent cannot self-initiate a plan review.
In plan mode it persists the plan the same way `update_plan` does, then
returns a friendly "[PLAN-PROPOSED]" line.

The agent CANNOT change the mode in either direction — that requires the
user (prompt dock -> POST /api/chat/{id}/mode). This tool only persists
the plan; it does not change the mode.
"""

from __future__ import annotations

from typing import Optional


def _current_mode(ctx: object = None) -> str:
    """Resolve the chat's plan/build mode (read-only). Defaults to build."""
    chat_id = ""
    try:
        if ctx is not None:
            chat_id = str(getattr(ctx, "chat_id", "") or "")
    except Exception:
        chat_id = ""
    if not chat_id:
        try:
            from sync.state_sync import safe_get_state
            chat_id = str((safe_get_state() or {}).get("chat_id") or "")
        except Exception:
            chat_id = ""
    if not chat_id:
        return "build"
    try:
        from server.store.mode_store import get_mode
        return get_mode(chat_id) or "build"
    except Exception:
        return "build"


def propose_plan(
    explanation: str = "",
    rationale: str = "",
    plan: Optional[list] = None,
    ctx: object = None,
) -> str:
    """Persist a plan proposal (plan mode only). In build mode this is a
    no-op — the agent must keep working with `update_plan`, never divert
    into a plan review on its own."""
    if _current_mode(ctx) != "plan":
        return (
            "[PLAN-PROPOSED] (no-op — chat is in build mode. Plan reviews are "
            "user-initiated via the prompt dock only. Continue the task with "
            "`update_plan` instead of proposing a plan.)"
        )
    if not plan:
        return "[PLAN-PROPOSED] (empty plan — nothing to commit)"

    # Reuse update_plan for persistence. This goes through the same
    # normalization + scope rules.
    from .planTools import update_plan as _update_plan
    try:
        res = _update_plan(explanation=explanation, plan=plan, ctx=ctx)
    except TypeError:
        res = _update_plan(explanation=explanation, plan=plan)

    # Tag it. The "agent cannot transition out of plan mode" rule is
    # enforced at the registry guard layer, not here.
    if isinstance(res, str) and res.startswith("Plan updated"):
        return f"[PLAN-PROPOSED] rationale: {rationale or '(none)'}\n{res}"
    return f"[PLAN-PROPOSED] {res}"
