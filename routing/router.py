"""
Unified deterministic router — single source of truth.

Preserves flow contracts:
- orchestrator LLM still owns create_project / resume_project / delegate
- creation phrasing ("create project", "new project", "project named") ALWAYS goes to orchestrator
- existing project work (by workspace_path or name mention) goes direct to Dev (zero tokens)
- house/personal are explicit workspace_kind hints from the frontend; without hint behavior is unchanged

Callers:
  server/api_server.py  → decide_route() then stream_for_kind()
  sync/sync_manager.py  → decide_route() (Telegram parity)
"""
from __future__ import annotations

import os
from typing import Literal

from routing.types import RouteDecision, RouteKind

# Reuse existing project_router for index / matching / creation guard — single implementation
# Import lazily to avoid circular deps at module load
def _project_router():
    import agents.project_router as pr
    return pr


# House / Personal / DFD virtual kinds — frontend sends workspace_kind hint
_HOUSE_KINDS = {"house", "house_workspace"}
_PERSONAL_KINDS = {"personal", "personal_workspace", "general_workspace"}
_DFD_KINDS = {"dfd", "dfd_workspace"}


def decide_route(
    prompt: str,
    workspace_path: str = "",
    chat_id: str = "",
    workspace_kind: str = "",
) -> RouteDecision:
    """
    Deterministic pre-router. No LLM, no side effects.

    Returns {"kind": RouteKind, "project": dict|None, "reason": str}
    kind:
      project_dev  → caller should stream_direct(project)
      house        → run_house_direct
      general      → run_general_direct
      dfd          → run_dfd_direct
      orchestrator → orchestrator_node (LLM)
    """
    kind_hint = (workspace_kind or "").strip().lower()

    # ── 1) Explicit workspace_kind hints (house / personal / dfd) take precedence
    # Frontend HouseWorkspace / PersonalWorkspace / DfdWorkspace will send these.
    if kind_hint in _HOUSE_KINDS:
        return {"kind": "house", "project": None, "reason": "workspace_kind=house"}
    if kind_hint in _PERSONAL_KINDS:
        return {"kind": "general", "project": None, "reason": f"workspace_kind={kind_hint}"}
    if kind_hint in _DFD_KINDS:
        return {"kind": "dfd", "project": None, "reason": f"workspace_kind={kind_hint}"}

    pr = _project_router()

    # ── 2) Creation phrasing — MUST go to orchestrator LLM (it decides)
    try:
        if pr._CREATION_RE.search(prompt or ""):
            return {"kind": "orchestrator", "project": None, "reason": "creation_phrasing"}
    except Exception:
        pass

    # ── 3) Existing project work (by workspace_path or name mention) → direct Dev
    # This preserves the zero-token bypass for coding workspaces and cross-project mentions.
    try:
        route_kind, project = pr.route_request(prompt or "", workspace_path or "", chat_id or "")
        if route_kind == "project_dev" and project:
            return {"kind": "project_dev", "project": project, "reason": "existing_project"}
    except Exception as e:
        # On router error, fall through to orchestrator (safe)
        return {"kind": "orchestrator", "project": None, "reason": f"route_error:{e}"}

    # ── 4) Fallback by workspace_path
    if workspace_path:
        # Coding workspace with no direct hit → orchestrator handles (create / ad-hoc / fix)
        return {"kind": "orchestrator", "project": None, "reason": "coding_workspace_no_direct_match"}

    # Empty workspace_path (General / main) with no direct hit → orchestrator.
    # The orchestrator answers directly when no delegation is needed and only
    # hands out jobs to specialists when the request requires it — so main /
    # voice messages get the same fast coordinator path as everything else.
    # (Creation phrasing already returned orchestrator above.)
    return {"kind": "orchestrator", "project": None, "reason": "main_workspace_orchestrator"}


def stream_for_kind(
    kind: RouteKind,
    project: dict | None,
    prompt: str,
    workspace_path: str,
    chat_id: str,
    attachments: list[dict] | None = None,
    cancel_event=None,
):
    """
    Yield state snapshots for the decided kind. Mirrors previous per-branch
    streaming contracts so callers just do: for snap in stream_for_kind(...): yield snap
    """
    attachments = attachments or []
    image_paths = [a.get("path") for a in attachments if a.get("isImage") and a.get("path")]

    if kind == "project_dev" and project:
        from agents.project_router import stream_direct
        yield from stream_direct(project, prompt, workspace_path or "", chat_id or "")
        return

    if kind == "house":
        from agents.house_agent import run_house_direct
        yield from run_house_direct(prompt, chat_id=chat_id or "", images=image_paths or None)
        return

    if kind == "general":
        from agents.general_agent import run_general_direct
        yield from run_general_direct(prompt, chat_id=chat_id or "", images=image_paths or None)
        return

    if kind == "dfd":
        from agents.dfd_agent import run_dfd_direct
        yield from run_dfd_direct(prompt, chat_id=chat_id or "", images=image_paths or None)
        return

    if kind == "orchestrator":
        # Orchestrator path — includes creation / resume / delegate via LLM
        # Use verify_helper wrapper so CI gate push/watch still runs
        try:
            from verify_helper import run_graph_with_verify
        except Exception:
            from Testing_Log.Testing.verify_helper import run_graph_with_verify  # fallback for test layout
        yield from run_graph_with_verify(prompt, workspace_path=workspace_path or "", chat_id=chat_id or "", cancel_event=cancel_event)
        return

    # Unknown kind — fallback to orchestrator
    try:
        from verify_helper import run_graph_with_verify
    except Exception:
        from Testing_Log.Testing.verify_helper import run_graph_with_verify
    yield from run_graph_with_verify(prompt, workspace_path=workspace_path or "", chat_id=chat_id or "", cancel_event=cancel_event)
