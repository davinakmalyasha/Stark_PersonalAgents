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
              orchestrator: deepseek-v4-flash-vision-exp, thinking disabled
              workspace:    deepseek-v4-flash,  thinking high on iter-1/trouble
              specialists:  free pool
opencode  Identical matrix, billed through the OpenCode Go gateway.
command   Command Code Provider API (OpenAI-compatible).
              orchestrator: laguna-s-2.1-free, thinking low
              workspace:    deepseek/deepseek-v4-flash, thinking high
              specialists:  laguna-s-2.1-free

Tiers
-----
orchestrator  OrchestratorAgent
workspace     Developer / General / House workspace agents
specialist    true subagents spawned below the workspace tier (UI, Deploy, ...)

Precedence contract: explicit environment overrides always win over mode
defaults (REASONING_ITER1, ORCH_REASONING_ITER1, REASONING_ROUTINE,
REASONING_ORCH_ROUTINE, REASONING_SUBAGENT_ROUTINE, GENERAL_MODEL_BACKEND,
SUBAGENT_MODEL_BACKEND). This module only supplies DEFAULTS.

Model-name conventions: canonical names ("deepseek-v4-flash") are mapped to
the active provider's slug by llm.llm._map_model; org-scoped literals
("deepseek/deepseek-v4-flash", "laguna-s-2.1-free") pass through untouched,
so command-mode entries can name gateway models directly.
"""

from __future__ import annotations

import os

MODES = ("free", "deepseek", "opencode", "command")

# Canonical vision model (maps per-provider inside llm.py). Kept here so the
# matrix stays readable; do not confuse with the command-mode literal slugs.
_CANONICAL_VISION = "deepseek-v4-flash-vision-exp"
_CANONICAL_FLASH = "deepseek-v4-flash"

_COMMAND_ORCH_MODEL_DEFAULT = "laguna-s-2.1-free"
_COMMAND_LIGHT_MODEL_DEFAULT = "laguna-s-2.1-free"

MODE_MATRIX: dict[str, dict[str, dict]] = {
    "free": {
        "orchestrator": {"backend": "free", "think_bias": False},
        "workspace": {"backend": "free", "think_bias": True},
        "specialist": {"backend": "free"},
    },
    "deepseek": {
        "orchestrator": {"backend": "paid", "model": _CANONICAL_VISION, "iter_effort": "disabled"},
        "workspace": {"backend": "paid", "model": _CANONICAL_FLASH, "iter_effort": "high"},
        "specialist": {"backend": "free"},
    },
    "opencode": {
        "orchestrator": {"backend": "paid", "model": _CANONICAL_VISION, "iter_effort": "disabled"},
        "workspace": {"backend": "paid", "model": _CANONICAL_FLASH, "iter_effort": "high"},
        "specialist": {"backend": "free"},
    },
    "command": {
        "orchestrator": {
            "backend": "paid",
            "model_env": "COMMAND_CODE_MODEL_ORCH",
            "model": _COMMAND_ORCH_MODEL_DEFAULT,
            "iter_effort": "low",
        },
        "workspace": {"backend": "paid", "model": _CANONICAL_FLASH, "iter_effort": "high"},
        "specialist": {
            "backend": "paid",
            "model_env": "COMMAND_CODE_MODEL_LIGHT",
            "model": _COMMAND_LIGHT_MODEL_DEFAULT,
        },
    },
}

_TIER_ROLES = {
    "orchestrator": ("orchestrator",),
    "workspace": ("developer", "developerfixing", "general", "house"),
}


def get_tier(role: str) -> str:
    """Map a role/agent name to its tier: orchestrator | workspace | specialist."""
    r = str(role or "").strip().lower()
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
    plan.setdefault("backend", "paid")
    plan["mode"] = m
    plan["tier"] = tier
    return plan


def specialist_backend(name: str, spec_backend: str = "") -> str:
    """Backend for a specialist subagent.

    Precedence: SUBAGENT_MODEL_BACKEND env > active-mode specialist entry >
    agents/specialists/*.json spec "backend" > "deepseek". Only "free" is
    dispatched to the free pool; every other value rides the paid llm.py path.
    """
    env_backend = os.getenv("SUBAGENT_MODEL_BACKEND", "").strip().lower()
    if env_backend:
        return env_backend
    entry = MODE_MATRIX[resolve_mode()].get("specialist") or {}
    backend = str(entry.get("backend") or "").strip().lower()
    if backend:
        return backend
    return str(spec_backend or "deepseek").strip().lower()


def specialist_model(spec_model: str | None = None) -> str | None:
    """Model override for specialist turns, or None to keep caller default.

    Non-empty when the active mode pins a concrete model for specialists
    (command mode -> laguna); otherwise falls back to the spec/caller value.
    """
    entry = MODE_MATRIX[resolve_mode()].get("specialist") or {}
    env_name = entry.get("model_env")
    if env_name:
        return os.getenv(env_name, entry.get("model") or "") or None
    return spec_model or None
