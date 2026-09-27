"""
provider_modes.py --- single source of truth for the LLM provider switch.

The UI dropdown ("provider") selects one of four MODES. A mode is a routing
contract: for every agent tier it pins the backend, the model, and the
thinking (reasoning-effort) policy.

Modes
-----
free      Every tier rides the multi-provider free pool
          (llm/free_providers.py). Orchestrator biases AWAY from reasoning
          models; workspace agents bias TOWARD them.
deepseek  Direct DeepSeek API billing.
              orchestrator: deepseek-v4.1-flash (native vision), thinking disabled
              workspace:    deepseek-v4.1-flash,  thinking high on iter-1/trouble
              specialists:  free pool
opencode  Identical matrix, billed through the OpenCode Go gateway.
command   Command Code Provider API (OpenAI-compatible).
               orchestrator: xiaomi/mimo-v2.6-flash, thinking low
              workspace:    deepseek/deepseek-v4.1-flash, thinking high
              specialists:  poolside/laguna-s-2.1-free (free slot)

Tiers
-----
orchestrator  OrchestratorAgent
workspace     Developer / General / House workspace agents
specialist    true subagents spawned below the workspace tier (UI, DevOps, ...)

Precedence contract: explicit environment overrides always win over mode
defaults (GENERAL_MODEL_BACKEND, DFD_MODEL_BACKEND, SUBAGENT_MODEL_BACKEND).
This module only supplies DEFAULTS.

Per-agent models: AGENT_MODELS below is the one place that pins "agent X uses
model M in mode Y" (None = inherit the tier default from MODE_MATRIX).
Resolution precedence: explicit environment override > AGENT_MODELS cell >
tier default > registry spec "model" (kept as a fallback until every agent
migrates here).

Model-name conventions: canonical names ("deepseek-v4.1-flash") are mapped to
the active provider's slug by llm.llm._map_model; org-scoped literals
("deepseek/deepseek-v4.1-flash", "poolside/laguna-s-2.1-free") pass through
untouched, so command-mode entries can name gateway models directly.
"""

from __future__ import annotations

import os

MODES = ("free", "deepseek", "opencode", "command")

# Canonical V4.1-Flash model (maps per-provider inside llm.py). Vision is native
# in V4.1, so vision and flash share the same id. Kept here so the
# matrix stays readable; do not confuse with the command-mode literal slugs.
_CANONICAL_VISION = "deepseek-v4.1-flash"
_CANONICAL_FLASH = "deepseek-v4.1-flash"

_COMMAND_ORCH_MODEL_DEFAULT = "xiaomi/mimo-v2.6-flash"
_COMMAND_LIGHT_MODEL_DEFAULT = "poolside/laguna-s-2.1-free"

MODE_MATRIX: dict[str, dict[str, dict]] = {
    "free": {
        "orchestrator": {"backend": "free", "think_bias": False},
        "workspace": {"backend": "free", "think_bias": True},
        "specialist": {"backend": "free"},
    },
    "deepseek": {
        "orchestrator": {"backend": "paid", "model": _CANONICAL_VISION},
        "workspace": {"backend": "paid", "model": _CANONICAL_FLASH},
        "specialist": {"backend": "free"},
    },
    "opencode": {
        "orchestrator": {"backend": "paid", "model": _CANONICAL_VISION},
        "workspace": {"backend": "paid", "model": _CANONICAL_FLASH},
        "specialist": {"backend": "free"},
    },
    "command": {
        "orchestrator": {
            "backend": "paid",
            "model_env": "COMMAND_CODE_MODEL_ORCH",
            "model": _COMMAND_ORCH_MODEL_DEFAULT,
            "supports_audio": True,
        },
        "workspace": {"backend": "paid", "model": _CANONICAL_FLASH},
        "specialist": {
            "backend": "paid",
            "model_env": "COMMAND_CODE_MODEL_LIGHT",
            "model": _COMMAND_LIGHT_MODEL_DEFAULT,
        },
    },
}

# ── Per-agent per-mode model pins (single place) ─────────────────────────────
#
# To pin an agent in a mode, put the model slug in its cell — e.g. DevOps on
# the command gateway would be "command": "xiaomi/mimo-v2.6-flash". A None
# cell means "inherit the tier default". Pinned cells must equal the agent's
# current effective model unless a deliberate switch is intended (with a
# golden-fixture update). Agents migrate here one row at a time; unknown or
# alias roles simply miss the table and inherit.
AGENT_MODELS: dict[str, dict[str, str | None]] = {
    "devops": {"free": None, "deepseek": None, "opencode": None, "command": "xiaomi/mimo-v2.6-flash"},
    "ui": {"free": None, "deepseek": None, "opencode": None, "command": None},
    "explore": {"free": None, "deepseek": None, "opencode": None, "command": None},
    "developer": {"free": None, "deepseek": "deepseek-v4.1-flash", "opencode": "deepseek-v4.1-flash", "command": "deepseek-v4.1-flash"},
    "general": {"free": None, "deepseek": "deepseek-v4.1-flash", "opencode": "deepseek-v4.1-flash", "command": "deepseek-v4.1-flash"},
    "house": {"free": None, "deepseek": "deepseek-v4.1-flash", "opencode": "deepseek-v4.1-flash", "command": "deepseek-v4.1-flash"},
    "dfd": {"free": None, "deepseek": None, "opencode": None, "command": None},
    "orchestrator": {"free": None, "deepseek": None, "opencode": None, "command": None},
}


def _canonical_agent_key(agent: str) -> str:
    """Resolve any agent name to its AGENT_MODELS row key.

    Matches registry key/display/runtime_name/delegation_key/aliases
    case-insensitively ("Dev" -> "developer", "Vision" -> "general").
    Unknown names pass through unchanged (they miss the table and inherit).
    Tier/backend resolution is deliberately NOT routed through here — this
    only widens the model-pin lookup so runtime names hit their agent's row.
    """
    key = str(agent or "").strip().lower()
    if not key or key in AGENT_MODELS:
        return key
    try:
        from agents.workspace_registry import all_specs
        for spec in all_specs():
            names = (spec.key, spec.display, spec.runtime_name,
                     spec.delegation_key, *(spec.aliases or ()))
            if any(str(n or "").strip().lower() == key for n in names if n):
                return str(spec.key or key)
    except Exception:
        pass
    return key


def agent_mode_model(agent: str, mode: str | None = None) -> str | None:
    """Pinned model for one agent in one mode, or None to inherit the default."""
    m = mode if mode in MODES else resolve_mode()
    row = AGENT_MODELS.get(_canonical_agent_key(agent)) or {}
    return row.get(m)


_TIER_ROLES = {
    "orchestrator": ("orchestrator",),
    "workspace": ("developer", "developerfixing", "general", "house", "dfd"),
}

#: Legacy alias overrides kept for roles with no registry entry (aliases like
#: "ui-design" and historical names). Registry provider_tier wins when present.
_TIER_OVERRIDES = {
    "ui": "workspace",
    "ui-design": "workspace",
}


def _registry_spec(role: str):
    """Lazy registry lookup (avoids an import cycle at llm import time)."""
    try:
        from agents.workspace_registry import get_spec

        return get_spec(role)
    except Exception:
        return None


def get_tier(role: str) -> str:
    """Map a role/agent name to its tier: orchestrator | workspace | specialist.

    The agent registry is the source of truth (provider_tier overrides tier);
    code fallbacks below keep alias/substring roles (ui-design, developerfixing,
    general-purpose, ...) behaving exactly as before.
    """
    r = str(role or "").strip().lower()
    spec = _registry_spec(r)
    if spec is not None:
        return str(spec.provider_tier or spec.tier or "specialist")
    if r in _TIER_OVERRIDES:
        return _TIER_OVERRIDES[r]
    for tier, names in _TIER_ROLES.items():
        if any(r == n or n in r for n in names):
            return tier
    return "specialist"


def resolve_mode() -> str:
    """Active provider mode.

    Resolution order: shared_state["llm_provider"] (running server / UI) >
    persisted file (.deep_agents/llm_provider.json, written by POST /api/provider)
    > LLM_PROVIDER_MODE env > "deepseek".
    """
    mode = ""
    try:
        from sync.state_sync import safe_get_state

        mode = str(safe_get_state().get("llm_provider") or "").strip().lower()
    except Exception:
        mode = ""
    if not mode:
        try:
            from llm.llm import _load_persisted_provider

            mode = str(_load_persisted_provider() or "").strip().lower()
        except Exception:
            mode = ""
    if not mode:
        mode = os.getenv("LLM_PROVIDER_MODE", "").strip().lower()
    return mode if mode in MODES else "deepseek"


def get_role_plan(role: str, mode: str | None = None) -> dict:
    """Routing plan for one role under a mode. Always includes mode/tier keys."""
    m = mode if mode in MODES else resolve_mode()
    tier = get_tier(role)
    plan = dict(MODE_MATRIX[m][tier])
    # Resolve env-tunable model overrides (command mode pins live in env).
    env_name = plan.get("model_env")
    if env_name:
        plan["model"] = os.getenv(env_name, plan.get("model") or "")
    # Per-agent pin (AGENT_MODELS): an explicitly set env value still wins;
    # otherwise a pinned cell overrides the tier default. All cells are None
    # today, so this is a no-op until the first pin lands.
    _cell = agent_mode_model(role, m)
    if _cell is not None and not (plan.get("model_env") and plan["model_env"] in os.environ):
        plan["model"] = _cell
    plan.setdefault("backend", "paid")
    plan["mode"] = m
    plan["tier"] = tier
    return plan


def specialist_backend(name: str, spec_backend: str = "") -> str:
    """Backend for a specialist subagent.

    Precedence: SUBAGENT_MODEL_BACKEND env > active-mode tier entry >
    agents/specialists/*.json spec "backend" > "deepseek". Tier entries are
    resolved per role: UI rides the workspace tier (Dev parity), everything
    else rides the specialist entry. Only "free" is dispatched to the free
    pool; every other value rides the paid llm.py path.
    """
    env_backend = os.getenv("SUBAGENT_MODEL_BACKEND", "").strip().lower()
    if env_backend:
        return env_backend
    entry = MODE_MATRIX[resolve_mode()].get(get_tier(name)) or {}
    backend = str(entry.get("backend") or "").strip().lower()
    if backend:
        return backend
    return str(spec_backend or "deepseek").strip().lower()


def specialist_model(spec_model: str | None = None, name: str = "") -> str | None:
    """Model override for specialist turns, or None to keep caller default.

    Only the true-specialist tier pins a concrete model (command mode ->
    laguna). Workspace-tier specialists (UI) return None so the runtime
    resolves the same canonical flash the Dev agent uses.
    """
    mode = resolve_mode()
    tier = get_tier(name)
    entry = MODE_MATRIX[mode].get(tier) or {}
    # Per-agent pin (AGENT_MODELS) sits between the env override and the tier
    # default. All cells are None today, so each branch below matches the old
    # expression exactly (including the empty-env-var edge: set-but-empty
    # still resolves to None, unset falls back to the tier default).
    _cell = agent_mode_model(name, mode)
    if tier == "specialist":
        env_name = entry.get("model_env")
        if env_name:
            if env_name in os.environ:
                return os.environ[env_name] or None
            return _cell or entry.get("model") or None
    return _cell or spec_model or None
