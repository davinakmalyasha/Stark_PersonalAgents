"""
Unified routing — single deterministic front-door for all workspace kinds.

Orchestrator LLM routing is UNCHANGED: when decide_route returns
("orchestrator", None) the caller invokes orchestrator_node which
may create_project / resume_project / delegate.

Deterministic mapping (zero LLM tokens):
  workspace_kind="house"    → House agent (run_house_direct)
  workspace_kind="personal" → General agent (personal workspace)
  workspace_path truthy + registered project → Dev direct (stream_direct)
  workspace_path truthy + mention of project name → Dev direct
  creation phrasing ("create project X") → orchestrator (LLM decides)
  empty workspace_path + no kind → General direct (legacy) or orchestrator if creation
"""
from routing.router import decide_route, stream_for_kind, RouteKind, RouteDecision

__all__ = ["decide_route", "stream_for_kind", "RouteKind", "RouteDecision"]
