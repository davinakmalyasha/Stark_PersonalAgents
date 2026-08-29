"""
Stage 4: propose_plan — agent-callable plan proposal.

Always allowed (in plan mode or build mode). Persists the plan the same
way `update_plan` does, then returns a friendly "[PLAN-PROPOSED]" line so
the user sees the proposal in the live log.

The agent CANNOT exit plan mode on its own — that requires the user to
press `/build` (handled by the slash-command dispatcher + the API server).
This tool only persists the plan; it does not change the mode.
"""

from __future__ import annotations

from typing import Optional


def propose_plan(
    explanation: str = "",
    rationale: str = "",
    plan: Optional[list] = None,
) -> str:
    """Persist a plan proposal. Behaves like `update_plan` but tags the
    result as a proposal (so the user sees it in the chat header)."""
    if not plan:
        return "[PLAN-PROPOSED] (empty plan — nothing to commit)"

    # Reuse update_plan for persistence. This goes through the same
    # normalization + scope rules.
    from .planTools import update_plan as _update_plan
    res = _update_plan(explanation=explanation, plan=plan)

    # Tag it. The "agent cannot transition out of plan mode" rule is
    # enforced at the registry guard layer, not here.
    if isinstance(res, str) and res.startswith("Plan updated"):
        return f"[PLAN-PROPOSED] rationale: {rationale or '(none)'}\n{res}"
    return f"[PLAN-PROPOSED] {res}"
