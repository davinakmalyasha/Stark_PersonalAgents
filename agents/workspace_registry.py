"""Agent registry — single source of truth for agent tools/profile/tier.

Two input formats, one registry surface:

* **workspace agents** are code entries registered below (they bind to real
  node entrypoints and prompt modules);
* **specialists** are config files under ``agents/specialists/*.json`` loaded
  by :func:`_load_specialists` (stdlib only — this module is imported by path
  from ``tools/_schemas`` during tool bootstrap, so it must not import the
  ``agents`` package).

Consumers derive everything from here:
    * ``tools/_schemas._SCOPE_MAP``  <- :func:`scope_map` (tool ACLs)
    * ``tools/AddOnTool/subagentTools`` <- :func:`all_specs` (runtime registry)
    * ``agents/delegation``          <- delegatable + runtime entrypoints
    * ``llm/provider_modes``          <- tier / provider_tier / backend / model
    * orchestrator + delegate prompts <- display / orchestrator_blurb
"""
from __future__ import annotations

import glob as _glob
import json as _json
import os as _os
from dataclasses import dataclass, field
from typing import Callable, Optional

from llm.deps import WorkspaceDeps


@dataclass(frozen=True)
class WorkspaceSpec:
    """What one agent is allowed to see/do and how it runs.

    Attributes:
        key: canonical lowercase key ("developer", "ui", "devops", ...).
        display: Title-case name used as registry scope ("Developer", ...).
        tools: allowlist of warehouse tool names for this agent.
        tier: model tier ("orchestrator" | "workspace" | "specialist").
        history_ns: memory namespace ("_general", "_dfd", or project path).
        description: one-liner for the orchestrator prompt / catalogs.
        deps_factory: builds WorkspaceDeps for a run (defaults to from_shared_state).
        delegatable: whether orchestrator can fan-out to it via `delegate`.
        kind: "workspace" | "subagent" | "internal".
        when_to_use: specialist catalog line (subagent specs).
        runtime: "module:function" node entrypoint (workspace agents).
        state_builder: node-state shape ("dev" | "plain").
        prompt_module: "module:ATTR" stable system prompt (workspace agents).
        aliases: alternative registry names ("Vision", "FreeAgent", ...).
        delegation_key: key accepted by `delegate` when it differs from `key`.
        delegatable_chat: where delegated chats live ("project" | "_general").
        plan_scope: scope whose plan the context pack reads.
        provider_tier: model-tier override for provider routing ("" = tier).
        model: optional model pin ("" = tier default).
        thinking: reasoning-effort profile ("inherit" | high|medium|low|disabled).
        compaction: optional {checkpoint_tokens, hard_limit, summary_output_tokens}.
        tool_caps: optional per-agent output caps {tool_name: chars}.
        system_prompt: assembled specialist prompt (JSON specs only).
        permissions: raw FilesystemPermission dicts (JSON specs only).
        response_format: structured-output schema (JSON specs only).
        internal: excluded from scope maps and catalogs.
    """

    key: str
    display: str
    tools: tuple[str, ...] = ()
    tier: str = "workspace"
    history_ns: str = ""
    description: str = ""
    delegatable: bool = False
    deps_factory: Optional[Callable[..., WorkspaceDeps]] = None

    kind: str = "workspace"
    when_to_use: str = ""
    orchestrator_blurb: str = ""
    delegate_blurb: str = ""
    runtime_name: str = ""
    runtime: str = ""
    state_builder: str = "plain"
    prompt_module: str = ""
    aliases: tuple[str, ...] = ()
    delegation_key: str = ""
    delegatable_chat: str = "project"
    plan_scope: str = ""
    provider_tier: str = ""
    model: str = ""
    thinking: str = "inherit"
    compaction: dict = field(default_factory=dict)
    tool_caps: dict = field(default_factory=dict)
    backend: str = ""
    skills: tuple[str, ...] = ()
    system_prompt: str = ""
    permissions: tuple = ()
    response_format: object = None
    internal: bool = False

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


# ── Code-defined workspace agents ────────────────────────────────────────────

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
            "edit_file",
            "run_command",
            "search_code",
            "ask_user",
            "update_plan",
            "call_subagent",
        ),
        tier="workspace",
        history_ns="",  # project path — resolved at build time
        description="Developer — pure implementer: receives specs and tasks from the orchestrator, writes clean tested TDD code, handles errors",
        delegatable=True,
        runtime="agents.developer_agent:developer_node",
        runtime_name="Dev",
        state_builder="dev",
        prompt_module="agents.developer_prompt:_STATIC_SYSTEM_TEMPLATE",
        delegation_key="dev",
        delegatable_chat="project",
        plan_scope="developer",
        thinking="high",
        orchestrator_blurb="builds and fixes software, runs tests, verifies",
        delegate_blurb="builds/fixes software",
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
        description="General — non-coding life & work assistant: research, documents (PDF/docx/xlsx), planning, web browsing, image understanding, personal admin. NOT a coder — code requests route back to Dev.",
        delegatable=True,
        runtime="agents.general_agent:general_node",
        prompt_module="agents.general_prompt:_GENERAL_SYSTEM_TEMPLATE",
        aliases=("Vision", "FreeAgent", "general-purpose"),
        delegatable_chat="_general",
        thinking="low",
        orchestrator_blurb=(
            "research, documents, images, personal notes/journal/reminders,\n"
            "  everyday admin"
        ),
        delegate_blurb="handles research, docs, images, personal notes/admin",
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
        description="House — home control specialist: reads and actuates the smart home via house_* tools (lights, climate, locks, plugs, blinds, cameras, scenes). NOT a coder — code requests route back to Dev.",
        delegatable=True,
        runtime="agents.house_agent:house_node",
        prompt_module="agents.house_prompt:_HOUSE_SYSTEM_TEMPLATE",
        delegatable_chat="_general",
        thinking="low",
        orchestrator_blurb="smart home: lights, climate, locks, scenes",
        delegate_blurb="controls smart-home devices",
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
        runtime="agents.dfd_agent:dfd_node",
        prompt_module="agents.dfd_prompt:_DFD_SYSTEM_TEMPLATE",
        thinking="high",
    )
)
_register(
    WorkspaceSpec(
        key="orchestrator",
        display="Orchestrator",
        tools=(
            "create_project",
            "ask_user",
            "delegate",
            "delegation_status",
            "job_message",
            "job_cancel",
            "propose_plan",
        ),
        tier="orchestrator",
        history_ns="",
        description="Orchestrator — routes tasks to workspace agents",
        delegatable=False,
    )
)


# ── Config-defined agents (agents/specialists/*.json) ────────────────────────

_SPECIALIST_REGISTRY: dict[str, WorkspaceSpec] = {}
_SPECIALISTS_LOADED = False

#: Profile-value contracts (invalid values warn and fall back to safe defaults;
#: a typo must never crash provider routing or silently change behavior).
_VALID_THINKING = ("inherit", "disabled", "low", "medium", "high", "max")
_VALID_TIERS = ("workspace", "specialist", "orchestrator")
_COMPACTION_KEYS = ("checkpoint_tokens", "hard_limit", "summary_output_tokens")


def _specialists_dir() -> str:
    return _os.path.join(_os.path.dirname(_os.path.abspath(__file__)), "specialists")


def _clean_list(value) -> tuple:
    return tuple(str(v).strip() for v in (value or []) if str(v).strip())


def _warn(name: str, msg: str) -> None:
    print(f"[AGENT-REGISTRY] {name or '?'}: {msg}")


def _pos_int(value) -> int:
    """Positive int, else 0 (invalid)."""
    try:
        parsed = int(str(value).strip())
        return parsed if parsed > 0 else 0
    except Exception:
        return 0


def _spec_from_json(e: dict) -> Optional[WorkspaceSpec]:
    """Normalize one specialist JSON entry into a WorkspaceSpec. Never raises."""
    if not isinstance(e, dict):
        return None
    name = str(e.get("name") or "").strip()
    if not name:
        return None
    prompt = e.get("system_prompt")
    if isinstance(prompt, list):
        prompt = "\n".join(str(ln) for ln in prompt)
    elif not isinstance(prompt, str):
        prompt = ""
    when = str(e.get("when_to_use") or "").strip()
    internal = bool(e.get("internal"))
    kind = str(e.get("kind") or "subagent").strip().lower()

    tier = str(e.get("tier") or "specialist").strip().lower()
    if tier not in _VALID_TIERS:
        _warn(name, f"invalid tier {tier!r} — using 'specialist'")
        tier = "specialist"
    provider_tier = str(e.get("provider_tier") or "").strip().lower()
    if provider_tier and provider_tier not in _VALID_TIERS:
        _warn(name, f"invalid provider_tier {provider_tier!r} — ignored")
        provider_tier = ""
    thinking = str(e.get("thinking") or "inherit").strip().lower()
    if thinking not in _VALID_THINKING:
        _warn(name, f"invalid thinking {thinking!r} — using 'inherit'")
        thinking = "inherit"

    raw_compaction = e.get("compaction")
    if raw_compaction is not None and not isinstance(raw_compaction, dict):
        _warn(name, "compaction must be an object — ignored")
        raw_compaction = {}
    compaction: dict = {}
    for key, value in (raw_compaction or {}).items():
        if key not in _COMPACTION_KEYS:
            _warn(name, f"compaction.{key} is unknown — dropped")
            continue
        parsed = _pos_int(value)
        if parsed:
            compaction[key] = parsed
        else:
            _warn(name, f"compaction.{key} must be a positive integer — dropped")

    raw_caps = e.get("tool_caps")
    if raw_caps is not None and not isinstance(raw_caps, dict):
        _warn(name, "tool_caps must be an object — ignored")
        raw_caps = {}
    tool_caps: dict = {}
    for key, value in (raw_caps or {}).items():
        parsed = _pos_int(value)
        if parsed:
            tool_caps[str(key)] = parsed
        else:
            _warn(name, f"tool_caps.{key} must be a positive integer — dropped")

    permissions = e.get("permissions") if isinstance(e.get("permissions"), list) else []
    spec = WorkspaceSpec(
        key=name.lower(),
        display=name,
        tools=_clean_list(e.get("tools")),
        tier=tier,
        history_ns=f"_{name.lower()}",
        description=when or "Specialist subagent.",
        delegatable=False,
        kind=kind,
        when_to_use=when,
        provider_tier=provider_tier,
        model=str(e.get("model") or "").strip(),
        thinking=thinking,
        compaction=compaction,
        tool_caps=tool_caps,
        backend=str(e.get("backend") or "").strip().lower(),
        skills=_clean_list(e.get("skills")),
        system_prompt=prompt or "You are a specialized subagent.",
        permissions=tuple(permissions),
        response_format=e.get("response_format"),
        internal=internal,
    )
    if not spec.tools and not internal:
        return None
    return spec


def _load_specialists() -> dict[str, WorkspaceSpec]:
    """Load agents/specialists/*.json into specs. Never raises."""
    global _SPECIALISTS_LOADED
    if _SPECIALISTS_LOADED:
        return _SPECIALIST_REGISTRY
    out: dict[str, WorkspaceSpec] = {}
    spec_dir = _specialists_dir()
    if _os.path.isdir(spec_dir):
        for path in sorted(_glob.glob(_os.path.join(spec_dir, "*.json"))):
            try:
                with open(path, "r", encoding="utf-8") as f:
                    data = _json.load(f)
                entries = []
                if isinstance(data, dict) and "subagents" in data:
                    entries = data.get("subagents") or []
                elif isinstance(data, dict) and data.get("name"):
                    entries = [data]
                elif isinstance(data, list):
                    entries = data
                for e in entries:
                    spec = _spec_from_json(e)
                    if spec:
                        out[spec.key] = spec
            except Exception as err:
                print(f"[AGENT-REGISTRY] {_os.path.basename(path)} skipped: {err}")
    _SPECIALIST_REGISTRY.clear()
    _SPECIALIST_REGISTRY.update(out)
    _SPECIALISTS_LOADED = True
    return _SPECIALIST_REGISTRY


# ── Lookups / derivations ────────────────────────────────────────────────────

def _all() -> dict[str, WorkspaceSpec]:
    merged = dict(WORKSPACE_REGISTRY)
    for key, spec in _load_specialists().items():
        merged.setdefault(key, spec)
    return merged


def get_spec(key: str) -> Optional[WorkspaceSpec]:
    if not key:
        return None
    k = str(key).lower()
    return WORKSPACE_REGISTRY.get(k) or _load_specialists().get(k)


def get_tool_names(key: str) -> list[str]:
    spec = get_spec(key)
    return list(spec.tools) if spec else []


def get_delegatable() -> list[str]:
    return [k for k, s in WORKSPACE_REGISTRY.items() if s.delegatable]


def delegatable_specs() -> list[WorkspaceSpec]:
    """Delegatable workspace agents, in registry order (orchestrator prompt)."""
    return [s for s in WORKSPACE_REGISTRY.values() if s.delegatable]


def all_specs() -> list[WorkspaceSpec]:
    return list(_all().values())


def thinking_prefer_reasoning(thinking: str) -> Optional[bool]:
    """Map a thinking value to the free pool's ``prefer_reasoning`` flag.

    high/max -> True, low/disabled -> False, anything else (medium/inherit)
    -> None, which keeps today's pool default behavior.
    """
    value = str(thinking or "inherit").strip().lower()
    if value in ("high", "max", "xhigh"):
        return True
    if value in ("low", "disabled", "minimal"):
        return False
    return None


def get_profile(key: str) -> dict:
    """Execution profile for one agent (spec fields with safe defaults)."""
    s = get_spec(key)
    if s is None:
        return {"key": str(key or ""), "display": str(key or ""), "kind": "workspace",
                "tier": "workspace", "provider_tier": "workspace", "backend": "",
                "model": "", "thinking": "inherit", "compaction": {}, "tool_caps": {}}
    return {
        "key": s.key,
        "display": s.display,
        "kind": s.kind,
        "tier": s.tier,
        "provider_tier": s.provider_tier or s.tier,
        "backend": s.backend,
        "model": s.model,
        "thinking": s.thinking or "inherit",
        "compaction": dict(s.compaction or {}),
        "tool_caps": dict(s.tool_caps or {}),
    }


# ── _SCOPE_MAP derivation (single source of truth) ───────────────────────────

def scope_map() -> dict[str, list[str]]:
    """Map lowercase scope key -> tool names.

    tools/_schemas._SCOPE_MAP is derived from this, so agent → tool ACLs live
    here only (adding/removing a tool for an agent changes one place).
    Internal specs (Compactor) are excluded — they never execute warehouse tools.
    """
    out: dict[str, list[str]] = {}
    for key, spec in _all().items():
        if spec.internal:
            continue
        out[key] = list(spec.tools)
    return out
