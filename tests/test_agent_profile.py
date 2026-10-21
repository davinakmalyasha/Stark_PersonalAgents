"""Agent-profile + context-management contracts (P1/P2/P4).

Covers the registry profile fields, the model-window lookup, the compaction
policy precedence/clamp, per-agent tool caps, and the free-path compaction
no-op guard. Uses the real tools package (the offline harness may have
installed its stub) and restores the stub afterwards.
"""
import sys

import pytest


def _ensure_real_tools():
    mod = sys.modules.get("tools")
    if mod is not None and not hasattr(mod, "__path__"):
        for name in [n for n in list(sys.modules)
                     if n == "tools" or n.startswith("tools.")]:
            sys.modules.pop(name, None)
    import tools  # noqa: F401


@pytest.fixture(autouse=True)
def _real_tools_env():
    prev = sys.modules.get("tools")
    _ensure_real_tools()
    yield
    if prev is not None and not hasattr(prev, "__path__"):
        sys.modules["tools"] = prev


def test_devops_profile_fields():
    from agents.workspace_registry import get_profile

    p = get_profile("DevOps")
    assert p["thinking"] == "high"
    assert p["tier"] == "specialist"
    assert p["compaction"]["summary_output_tokens"] == 6144
    assert p["tool_caps"]["run_command"] == 12000
    assert p["tool_caps"]["mcp"] == 12000


def test_ui_profile_provider_tier_and_thinking():
    from agents.workspace_registry import get_profile

    p = get_profile("UI")
    assert p["provider_tier"] == "workspace"
    assert p["thinking"] == "high"
    assert p["tier"] == "specialist"  # registry tier; provider routing uses workspace


def test_explore_profile_fields():
    from agents.workspace_registry import get_profile

    ex = get_profile("Explore")
    assert ex["backend"] == "free"
    assert ex["tier"] == "specialist"
    assert ex["thinking"] == "medium"
    assert ex["tool_caps"]["read_file"] == 40000


def test_specialist_permissions_enforced():
    import os

    from agents.subagent_manager import _child_permissions
    from tools import active_permissions
    from tools._runtime import _HARNESS_ROOT, enforce_fs_permission

    ws_file = os.path.join(_HARNESS_ROOT, "agents", "developer_agent.py")
    scratch_file = os.path.join(_HARNESS_ROOT, ".deep_agents", "scratch", "repros", "x.py")

    def allowed(rules, op, path):
        token = active_permissions.set(rules)
        try:
            enforce_fs_permission(op, path)
            return True
        except PermissionError:
            return False
        finally:
            active_permissions.reset(token)

    explore = _child_permissions("Explore")
    assert explore, "Explore spec permissions must load through the tools facade"
    assert allowed(explore, "read", ws_file)
    assert not allowed(explore, "write", ws_file)
    assert not allowed(explore, "write", scratch_file)


def test_workspace_agent_profile_defaults():
    from agents.workspace_registry import get_profile

    p = get_profile("Developer")
    assert p["thinking"] == "high"
    assert p["model"] == ""
    assert p["provider_tier"] == "workspace"


def test_workspace_thinking_registry_values():
    from agents.workspace_registry import get_profile

    assert get_profile("Developer")["thinking"] == "high"
    assert get_profile("DFD")["thinking"] == "high"
    assert get_profile("General")["thinking"] == "low"
    assert get_profile("House")["thinking"] == "low"
    assert get_profile("Orchestrator")["thinking"] == "high"


def test_registry_thinking_resolver_byte_compat():
    from llm.native_tools import _registry_thinking

    # Byte-compat lock: these equal the hardcoded literals the nodes used to
    # pass before the refactor (Developer/DFD high, General/House low).
    assert _registry_thinking("Developer") == "high"
    assert _registry_thinking("DFD") == "high"
    assert _registry_thinking("General") == "low"
    assert _registry_thinking("House") == "low"
    assert _registry_thinking("Orchestrator") == "high"
    assert _registry_thinking("DevOps") == "high"
    assert _registry_thinking("unknown-role") is None


def test_window_for_model():
    from llm.model_history import window_for_model

    assert window_for_model("deepseek-v4.1-flash") == 1_000_000
    assert window_for_model("deepseek-v4.1-flash[1m]") == 1_000_000
    assert window_for_model("poolside/laguna-s-2.1-free") > 0
    assert window_for_model("some-unknown-model") == 0
    assert window_for_model("") == 0


def test_policy_for_deps_registry_overrides_and_window_clamp():
    from llm.compaction_native import policy_for_deps
    from llm.deps import WorkspaceDeps

    deps = WorkspaceDeps(
        role="DevOps",
        compaction={"summary_output_tokens": 6144},
        context_window=262144,
    )
    pol = policy_for_deps(deps)
    assert pol.checkpoint_trigger == int(262144 * 0.75)
    assert pol.hard_limit == min(550_000, int(262144 * 0.85))
    assert pol.summary_output_tokens == 6144


def test_policy_for_deps_specialist_role_passthrough():
    from llm.compaction_native import policy_for_deps
    from llm.deps import WorkspaceDeps

    pol = policy_for_deps(WorkspaceDeps(role="DevOps"))
    assert pol.checkpoint_trigger == 500_000  # shared default; no role remap
    # An explicit spec override beats the shared default without a window.
    pol2 = policy_for_deps(WorkspaceDeps(role="DevOps",
                                         compaction={"checkpoint_tokens": 150_000}))
    assert pol2.checkpoint_trigger == 150_000
    assert pol2.hard_limit == 550_000


def test_tool_caps_contextvar_and_env_precedence(monkeypatch):
    from tools import active_tool_caps
    from tools._runtime import _tool_cap

    assert _tool_cap("run_command") == 30_000
    token = active_tool_caps.set({"run_command": 1234})
    try:
        assert _tool_cap("run_command") == 1234
    finally:
        active_tool_caps.reset(token)
    assert _tool_cap("run_command") == 30_000
    # Explicit env tuning stays the outermost override.
    monkeypatch.setenv("CAP_RUN_COMMAND", "7")
    token = active_tool_caps.set({"run_command": 1234})
    try:
        assert _tool_cap("run_command") == 7
    finally:
        active_tool_caps.reset(token)


def test_maybe_compact_free_noop_on_paid_path():
    from tools.AddOnTool.subagentTools import _maybe_compact_free

    messages = [object(), object(), object(), object()]
    _maybe_compact_free("DevOps", messages, "paid")
    assert len(messages) == 4


def test_likely_min_window_bounds():
    from llm.free_providers import likely_min_window

    w = likely_min_window("coding", prefer_reasoning=True, top_n=3)
    assert 131_072 <= w <= 1_048_576
    # Unknown capability -> no candidates -> no clamp.
    assert likely_min_window("no-such-capability") == 0


def test_clamp_policy_uses_window():
    from llm.compaction_native import clamp_policy, policy_for_deps
    from llm.deps import WorkspaceDeps

    # Mirror the spawner: deps carry the registry spec's compaction override.
    base = policy_for_deps(WorkspaceDeps(role="DevOps",
                                         compaction={"summary_output_tokens": 6144}))
    clamped = clamp_policy(base, 200_000)
    assert clamped.checkpoint_trigger == 150_000
    assert clamped.hard_limit == 170_000
    assert clamped.summary_output_tokens == 6144  # spec override survives the clamp
    # No window -> untouched policy (identity).
    assert clamp_policy(base, 0) is base


def test_maybe_compact_free_triggers_and_replaces(monkeypatch):
    import llm.model_history as mh
    import llm.native_tools as nt
    import memory.compaction_engine as ce
    import tools.AddOnTool.subagentTools as st

    monkeypatch.setattr(mh, "estimate_history_tokens", lambda msgs: 10 ** 7)
    monkeypatch.setattr(mh, "compact_history_native", lambda history, **kw: ["COMPACTED"])

    async def fake_engine(messages, project_path, **kwargs):
        return {"summary": "SUM", "chunks": 1, "model": "fake", "window": 200000}

    monkeypatch.setattr(ce, "run_compaction_summary", fake_engine)

    def fake_block(coro):
        coro.close()
        return {
            "summary": "SUM", "chunks": 1, "model": "fake", "window": 200000,
            "full_map": None, "project_memory": "", "user_messages": [],
        }

    monkeypatch.setattr(nt, "_run_coro_blocking", fake_block)

    messages = [object()] * 6
    st._maybe_compact_free("DevOps", messages, "free")
    assert messages == ["COMPACTED"]


def test_thinking_prefer_reasoning_mapping():
    from agents.workspace_registry import thinking_prefer_reasoning

    assert thinking_prefer_reasoning("high") is True
    assert thinking_prefer_reasoning("max") is True
    assert thinking_prefer_reasoning("low") is False
    assert thinking_prefer_reasoning("disabled") is False
    assert thinking_prefer_reasoning("medium") is None
    assert thinking_prefer_reasoning("inherit") is None
    assert thinking_prefer_reasoning("") is None
    assert thinking_prefer_reasoning(None) is None


def test_free_path_passes_prefer_reasoning(monkeypatch):
    import llm.free_providers as fp
    import tools.AddOnTool.subagentTools as st

    calls = {}

    def fake_turn(name, messages, **kwargs):
        calls[name] = kwargs
        return ("ok", [], None)

    monkeypatch.setattr(st, "_specialist_backend", lambda name: "free")
    monkeypatch.setattr(fp, "free_native_turn", fake_turn)

    st._specialist_call_backend("DevOps", [])
    assert calls["DevOps"]["prefer_reasoning"] is True
    st._specialist_call_backend("UnknownRole", [])
    assert calls["UnknownRole"]["prefer_reasoning"] is None


def test_spec_from_json_normalizes_profile_fields():
    from agents.workspace_registry import _spec_from_json

    spec = _spec_from_json({
        "name": "Tmp",
        "tools": ["read_file"],
        "thinking": "medium",
        "provider_tier": "workspace",
        "model": "deepseek:v4.1-flash",
        "compaction": {"checkpoint_tokens": 150000},
        "tool_caps": {"read_file": 5000},
        "skills": ["deploy-gate"],
        "when_to_use": "test agent",
    })
    assert spec is not None
    assert spec.key == "tmp" and spec.display == "Tmp"
    assert spec.thinking == "medium"
    assert spec.provider_tier == "workspace"
    assert spec.model == "deepseek:v4.1-flash"
    assert spec.compaction["checkpoint_tokens"] == 150000
    assert spec.tool_caps["read_file"] == 5000
    assert spec.skills == ("deploy-gate",)
    # A non-internal spec with no tools is rejected (registry hygiene).
    assert _spec_from_json({"name": "Broken", "tools": []}) is None
    # Internal specs may have no tools.
    assert _spec_from_json({"name": "Internal", "tools": [], "internal": True}) is not None


def test_spec_validation_invalid_values_fall_back(capsys):
    from agents.workspace_registry import _spec_from_json

    spec = _spec_from_json({
        "name": "Bad",
        "tools": ["read_file"],
        "thinking": "hgih",
        "tier": "workspac",
        "provider_tier": "nonsense",
        "compaction": {"checkpoint_tokens": -5, "bogus": 10, "hard_limit": "200000"},
        "tool_caps": {"read_file": "x", "run_command": 500},
    })
    assert spec is not None
    assert spec.thinking == "inherit"          # invalid -> safe default
    assert spec.tier == "specialist"           # invalid -> safe default
    assert spec.provider_tier == ""            # invalid -> ignored
    assert spec.compaction == {"hard_limit": 200000}  # valid kept; invalid/unknown dropped
    assert spec.tool_caps == {"run_command": 500}
    out = capsys.readouterr().out
    for marker in ("invalid thinking", "invalid tier", "invalid provider_tier",
                   "compaction.checkpoint_tokens", "tool_caps.read_file"):
        assert marker in out, f"missing validation warning: {marker}"


def test_spec_validation_malformed_sections_ignored(capsys):
    from agents.workspace_registry import _spec_from_json

    spec = _spec_from_json({
        "name": "Malformed",
        "tools": ["read_file"],
        "compaction": ["nope"],
        "tool_caps": "nope",
    })
    assert spec is not None
    assert spec.compaction == {} and spec.tool_caps == {}
    out = capsys.readouterr().out
    assert "compaction must be an object" in out
    assert "tool_caps must be an object" in out


def test_spec_validation_valid_config_untouched():
    from agents.workspace_registry import _spec_from_json

    spec = _spec_from_json({
        "name": "Good",
        "tools": ["read_file"],
        "thinking": "high",
        "compaction": {"summary_output_tokens": 6144},
        "tool_caps": {"run_command": 12000},
    })
    assert spec.thinking == "high"
    assert spec.compaction == {"summary_output_tokens": 6144}
    assert spec.tool_caps == {"run_command": 12000}


def test_orchestrator_and_tool_text_have_no_persona():
    from agents.orchestrator_agent import _ORCHESTRATOR_SYSTEM_TEMPLATE
    from tools._schemas import TOOL_DEFINITIONS

    text = _ORCHESTRATOR_SYSTEM_TEMPLATE + "\n" + "\n".join(
        str(d.get("description") or "") for d in TOOL_DEFINITIONS
    )
    lowered = text.lower()
    for marker in ("sir", "jarvis", "right-hand", "chief of staff"):
        assert marker not in lowered, f"persona marker remains: {marker}"


def test_compaction_uses_one_same_model_summary(monkeypatch):
    import asyncio

    from pydantic_ai.messages import ModelRequest, UserPromptPart

    import memory.compaction_engine as ce

    calls = []

    async def fake_request(model, messages, prompt_text, max_tokens):
        calls.append((model, messages, prompt_text, max_tokens))
        return "[SUMMARY]\nDONE\n[PROJECT.MD]\nNO_CHANGE", "same-model"

    monkeypatch.setattr(ce, "_request", fake_request)
    monkeypatch.setattr(
        ce,
        "_resolve_model",
        lambda *args, **kwargs: ("MODEL", "same-model", 1_000_000),
    )

    history = [ModelRequest(parts=[UserPromptPart(content="hello")])]
    result = asyncio.run(
        ce.run_compaction_summary(history, "", role="Developer")
    )

    assert len(calls) == 1
    assert result["chunks"] == 1
    assert result["ledgers"] == 0
    assert result["model"] == "same-model"
    assert result["served_models"] == ["same-model"]
    assert result["summary"].startswith(ce.SUMMARY_PREFIX)
