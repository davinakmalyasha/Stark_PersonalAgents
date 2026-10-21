"""Golden locks for the agent-registry consolidation (P0).

These assert that the registry refactor keeps every externally observable
contract byte-identical: scope ACLs, provider tier routing, specialist
backend/model resolution, the runtime subagent registry shape, the rendered
catalogs, and the orchestrator/delegate prompt fragments.

If a deliberate behavior change is wanted, update the fixture explicitly —
never loosen an assertion silently.
"""
import json
import os
import sys

import pytest

FIXTURE = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                       "fixtures", "agent_registry_golden.json")

with open(FIXTURE, "r", encoding="utf-8") as _f:
    GOLDEN = json.load(_f)


def _ensure_real_tools():
    """tests/_util.py installs an offline stub as sys.modules['tools'].

    These goldens exercise the REAL package, so evict the stub (and any stub
    submodules) before importing. Safe when the real package is already loaded.
    """
    mod = sys.modules.get("tools")
    if mod is not None and not hasattr(mod, "__path__"):
        for name in [n for n in list(sys.modules)
                     if n == "tools" or n.startswith("tools.")]:
            sys.modules.pop(name, None)
    import tools  # noqa: F401


@pytest.fixture(autouse=True)
def _real_tools_env():
    """Use the real tools package inside these tests, restore the stub after."""
    prev = sys.modules.get("tools")
    _ensure_real_tools()
    yield
    if prev is not None and not hasattr(prev, "__path__"):
        sys.modules["tools"] = prev


def _scope_map():
    from tools._schemas import _SCOPE_MAP
    return {k: list(v) for k, v in _SCOPE_MAP.items()}


def _registry():
    from tools.AddOnTool.subagentTools import _load_subagents, _SUBAGENTS_REGISTRY, subagent_catalog
    _load_subagents()
    out = {}
    for name in sorted(_SUBAGENTS_REGISTRY):
        s = _SUBAGENTS_REGISTRY[name]
        out[name] = {
            "kind": s.get("kind", ""),
            "internal": bool(s.get("internal")),
            "description": s.get("description", ""),
            "tools": list(s.get("tools") or []),
            "skills": list(s.get("skills") or []),
            "backend": s.get("backend", ""),
            "model": s.get("model", ""),
        }
    return out, subagent_catalog(filter_kind="subagent")


def test_scope_map_unchanged():
    assert _scope_map() == GOLDEN["scope_map"]


def test_provider_tiers_unchanged():
    import llm.provider_modes as pm
    for role, expected in GOLDEN["tier"].items():
        assert pm.get_tier(role) == expected, f"tier changed for {role}"


def test_role_plans_unchanged():
    import llm.provider_modes as pm
    for mode, plans in GOLDEN["role_plans"].items():
        for role, expected in plans.items():
            got = pm.get_role_plan(role, mode=mode)
            assert got == expected, f"role plan changed for {role} in {mode}"


def test_specialist_backend_model_unchanged(monkeypatch):
    import llm.provider_modes as pm
    for mode, entries in GOLDEN["specialist_backend_model"].items():
        monkeypatch.setattr(pm, "resolve_mode", lambda m=mode: m)
        for name, expected in entries.items():
            got = [pm.specialist_backend(name, "free"), pm.specialist_model(None, name)]
            assert got == expected, f"specialist route changed for {name} in {mode}"


def test_runtime_subagent_registry_unchanged():
    registry, _ = _registry()
    assert registry == GOLDEN["subagent_registry"]


def test_subagent_catalog_unchanged():
    _, catalog = _registry()
    assert catalog == GOLDEN["subagent_catalog"]


def test_orchestrator_specialists_block_unchanged():
    from agents.orchestrator_agent import _ORCHESTRATOR_SYSTEM_TEMPLATE
    start = _ORCHESTRATOR_SYSTEM_TEMPLATE.index("Specialists:")
    end = _ORCHESTRATOR_SYSTEM_TEMPLATE.index("## HOW YOU TALK")
    assert _ORCHESTRATOR_SYSTEM_TEMPLATE[start:end].rstrip("\n") == \
        GOLDEN["orchestrator_specialists_block"]


def test_delegate_tool_text_unchanged():
    from tools._schemas import TOOL_DEFINITIONS, _TOOL_DOCS
    delegate = next(d for d in TOOL_DEFINITIONS if d["name"] == "delegate")
    assert delegate["description"] == GOLDEN["delegate_description"]
    assert _TOOL_DOCS["delegate"] == GOLDEN["delegate_doc"]
