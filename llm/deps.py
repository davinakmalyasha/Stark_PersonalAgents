"""Typed per-run workspace context (pydantic-ai deps).

Replaces ad-hoc global reads of ``shared_state["project_path"]`` /
``shared_state["chat_id"]`` with one immutable object passed per run, so
parallel workspace agents are isolated and tests construct deps directly.

Backward-compatible: every consumer falls back to :func:`from_shared_state`
when no explicit deps object is provided — the global remains the source of
truth until all call sites migrate.
"""
from __future__ import annotations

import os
from dataclasses import dataclass, field
from typing import Optional


@dataclass
class WorkspaceDeps:
    """Per-run context for one agent turn inside a workspace.

    Attributes:
        project_path: Active workspace root (absolute path). Tools that read
            ``shared_state["project_path"]`` today read this once migrated.
        chat_id: Chat/session id used for history, SSE streaming and aborts.
        history_ns: Memory namespace ("_general", "_dfd", or the project path
            for developer workspaces).
        role: Agent role ("Developer", "General", "House", "DFD", "Orchestrator").
        llm_provider: Resolved provider config dict {base, key, flash, pro, ...}
            from ``llm._resolve_llm_provider()`` (None -> resolve on demand).
        allowed_tools: Optional allowlist of tool names for this agent (None ->
            use registry scope ACL).
        mcp_scopes: Optional list of MCP server scope names visible to this
            agent (None -> registry default).
        caps: Optional {site: max_chars} override for tool output offload.
    """

    project_path: str = ""
    chat_id: str = ""
    history_ns: str = ""
    role: str = ""
    llm_provider: Optional[dict] = None
    allowed_tools: Optional[list[str]] = None
    mcp_scopes: Optional[list[str]] = None
    caps: Optional[dict] = None
    pending_images: Optional[list] = None

    def __post_init__(self) -> None:
        if not self.project_path:
            self.project_path = os.environ.get(
                "HARNESS_ROOT",
                os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
            )
        if not self.history_ns:
            self.history_ns = self.project_path

    @classmethod
    def from_shared_state(
        cls,
        role: str = "",
        chat_id: str = "",
        project_path: str = "",
        history_ns: str = "",
    ) -> "WorkspaceDeps":
        """Shim: build deps from the global shared_state (pre-migration path)."""
        try:
            from sync.state_sync import safe_get_state
        except Exception:
            safe_get_state = None

        state: dict = {}
        if safe_get_state is not None:
            try:
                state = safe_get_state()
            except Exception:
                state = {}

        if not project_path:
            project_path = state.get("project_path") or os.environ.get(
                "HARNESS_ROOT",
                os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
            )
        if not chat_id:
            chat_id = str(state.get("chat_id") or "")
        if not history_ns:
            history_ns = (
                project_path
                if role.lower() in ("developer", "orchestrator")
                else (f"_{role.lower()}" if role else project_path)
            )

        provider = None
        try:
            from llm.llm import _resolve_llm_provider

            provider = _resolve_llm_provider()
        except Exception:
            provider = None

        return cls(
            project_path=project_path,
            chat_id=chat_id,
            history_ns=history_ns,
            role=role,
            llm_provider=provider,
        )
