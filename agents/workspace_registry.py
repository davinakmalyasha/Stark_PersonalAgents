"""Workspace registry — single source of truth for agent → tools/MCP/tier.

Today adding a workspace agent touches 5+ files (_SCOPE_MAP, provider_modes,
subagentTools registry, delegation tuple, orchestrator prompt). This registry
is the data-driven replacement: one entry per workspace agent declares what
tools/MCP servers/model tier it needs, and consumers derive from it.

``tools/_schemas._SCOPE_MAP`` derives from :func:`scope_map` here, so this
registry is the single source of truth for agent → tool ACLs.
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import Callable, Optional

from llm.deps import WorkspaceDeps


@dataclass(frozen=True)
class WorkspaceSpec:
    """What one workspace agent is allowed to see/do.

    Attributes:
        key: canonical lowercase key ("developer", "general", ...).
        display: Title-case name used as registry scope ("Developer", ...).
        tools: allowlist of warehouse tool names for this agent.
        tier: model tier ("orchestrator" | "workspace" | "specialist").
        history_ns: memory namespace ("_general", "_dfd", or project path).
        description: one-liner for the orchestrator prompt / catalogs.
        deps_factory: builds WorkspaceDeps for a run (defaults to from_shared_state).
        delegatable: whether orchestrator can fan-out to it via `delegate`.
    """

    key: str
    display: str
    tools: tuple[str, ...] = ()
    tier: str = "workspace"
    history_ns: str = ""
    description: str = ""
    delegatable: bool = False
    deps_factory: Optional[Callable[..., WorkspaceDeps]] = None

    def build_deps(self, chat_id: str = "", project_path: str = "") -> WorkspaceDeps:
        if self.deps_factory is not None:
            return self.deps_factory(
                role=self.display, chat_id=chat_id, project_path=project_path
            )
        return WorkspaceDeps.from_shared_state(
            role=self.display,
            chat_id=chat_id,
            project_path=project_path,
            history_ns=self.history_ns or self.key,
        )


# ── The registry ─────────────────────────────────────────────────────────────

WORKSPACE_REGISTRY: dict[str, WorkspaceSpec] = {}


def _register(spec: WorkspaceSpec) -> None:
    WORKSPACE_REGISTRY[spec.key] = spec


# Order matters for display; matches agents/ delegation DELEGATABLE_AGENTS.
_register(
    WorkspaceSpec(
        key="developer",
        display="Developer",
        tools=(
            "read_file",
            "outline",
            "edit_file",
            "run_command",
            "search_code",
            "ask_user",
            "update_plan",
            "propose_plan",
            "call_subagent",
        ),
        tier="workspace",
        history_ns="",  # project path — resolved at build time
        description="Developer — codes in a specific workspace via read/edit/run tools",
        delegatable=True,
    )
)
_register(
    WorkspaceSpec(
        key="general",
        display="General",
        tools=(
            "run_command",
            "read_pdf",
            "create_docx",
            "create_xlsx",
            "ask_user",
            "update_plan",
            "browser_act",
            "propose_plan",
            "files_changed",
            "look_at_screen",
            "set_screen_share",
        ),
        tier="workspace",
        history_ns="_general",
        description="General — personal stuff, docs, browsing, no codebase edits",
        delegatable=True,
    )
)
_register(
    WorkspaceSpec(
        key="house",
        display="House",
        tools=(
            "house_get_state",
            "house_get_device",
            "house_set_device",
            "house_run_scene",
            "house_get_history",
            "run_command",
            "browser_act",
            "ask_user",
            "update_plan",
            "propose_plan",
        ),
        tier="workspace",
        history_ns="_general",
        description="House — home devices, scenes, sensors via house_* tools",
        delegatable=True,
    )
)
_register(
    WorkspaceSpec(
        key="dfd",
        display="DFD",
        tools=(
            "dfd_bridge",
            "create_client",
            "create_project",
            "run_command",
            "ask_user",
            "update_plan",
            "propose_plan",
            "files_changed",
        ),
        tier="workspace",
        history_ns="_dfd",
        description="DFD — agency business: clients, projects, scaffold only (no money tools)",
        delegatable=False,  # reachable via run_dfd_direct / client_manager only
    )
)
# Specialist scopes (ui / deploy) mirror the live _SCOPE_MAP ACL so the
# derived _SCOPE_MAP stays byte-identical. They are not workspace agents.
# UI rides the paid native path like Dev, so its scope must match the
# specialist spec's tools exactly (the FilteredToolset intersects on it).
_register(
    WorkspaceSpec(
        key="ui",
        display="UI",
        tools=("browser_act", "read_file", "search_code", "run_command", "edit_file", "ask_user"),
        tier="specialist",
        history_ns="_ui",
        description="UI — frontend design and browser verification specialist",
        delegatable=False,
    )
)
_register(
    WorkspaceSpec(
        key="deploy",
        display="Deploy",
        tools=("github_apply_gate", "run_command", "read_file", "edit_file", "search_code", "ask_user"),
        tier="specialist",
        history_ns="_deploy",
        description="Deploy — go-live owner: runtime hardening, CI gate, branch ladder, deploy verification",
        delegatable=False,
    )
)
_register(
    WorkspaceSpec(
        key="orchestrator",
        display="Orchestrator",
        tools=(
            "create_project",
            "resume_project",
            "ask_user",
            "delegate",
            "delegation_status",
            "update_plan",
            "propose_plan",
            "files_changed",
        ),
        tier="orchestrator",
        history_ns="",
        description="Orchestrator — routes tasks to workspace agents",
        delegatable=False,
    )
)


def get_spec(key: str) -> Optional[WorkspaceSpec]:
    return WORKSPACE_REGISTRY.get(key.lower())


def get_tool_names(key: str) -> list[str]:
    spec = get_spec(key)
    return list(spec.tools) if spec else []


def get_delegatable() -> list[str]:
    return [k for k, s in WORKSPACE_REGISTRY.items() if s.delegatable]


def all_specs() -> list[WorkspaceSpec]:
    return list(WORKSPACE_REGISTRY.values())


# ── _SCOPE_MAP derivation (single source of truth) ───────────────────────────

def scope_map() -> dict[str, list[str]]:
    """Map lowercase scope key -> tool names.

    tools/_schemas._SCOPE_MAP is derived from this, so agent → tool ACLs live
    here only (adding/removing a tool for an agent changes one place).
    """
    out: dict[str, list[str]] = {}
    for key, spec in WORKSPACE_REGISTRY.items():
        out[key] = list(spec.tools)
    return out
