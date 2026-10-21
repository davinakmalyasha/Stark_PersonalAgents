"""update_plan incremental ops (orchestrator tool consolidation, Phase 4).

Full-replacement mode is untouched; ops patches the stored plan in place.
Store I/O is isolated to tmp_path via sync_helpers.WORKSPACE_DIR.
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


@pytest.fixture()
def _iso_tmp(tmp_path, monkeypatch):
    import sync.sync_helpers as sh
    import sync.state_sync as ss
    monkeypatch.setattr(sh, "WORKSPACE_DIR", str(tmp_path))
    # Isolate from any chat id leaked by other test modules.
    monkeypatch.setitem(ss.shared_state, "chat_id", "")
    monkeypatch.setitem(ss.shared_state, "project_path", "")
    return tmp_path


def _plan():
    from sync.sync_helpers import load_plan
    return load_plan("", "", "agent")


def test_full_replacement_still_works(_iso_tmp):
    from tools.MainTool.planTools import update_plan
    out = update_plan(plan=[{"step": "build x", "status": "in_progress"},
                            {"step": "verify y", "status": "pending"}])
    assert "0/2 completed" in out
    assert [i["status"] for i in _plan()] == ["in_progress", "pending"]


def test_ops_set_status_and_add(_iso_tmp):
    from tools.MainTool.planTools import update_plan
    update_plan(plan=[{"step": "build x", "status": "in_progress"},
                      {"step": "verify y", "status": "pending"}])
    out = update_plan(ops=[{"op": "set_status", "match": "build", "status": "completed"},
                           {"op": "add", "step": "ship it"}])
    assert "1/3 completed" in out
    assert [(i["step"], i["status"]) for i in _plan()] == [
        ("build x", "completed"), ("verify y", "pending"), ("ship it", "pending")]


def test_ops_rename_and_remove(_iso_tmp):
    from tools.MainTool.planTools import update_plan
    update_plan(plan=[{"step": "build x", "status": "pending"},
                      {"step": "drop me", "status": "pending"}])
    out = update_plan(ops=[{"op": "rename", "match": "build", "step": "build x better"},
                           {"op": "remove", "match": "drop"}])
    assert "0/1 completed" in out
    assert _plan() == [{"step": "build x better", "status": "pending"}]


def test_ops_rejects_both_modes_and_empty(_iso_tmp):
    from tools.MainTool.planTools import update_plan
    both = update_plan(plan=[{"step": "a", "status": "pending"}],
                       ops=[{"op": "add", "step": "b"}])
    assert both.startswith("Error:")
    assert _plan() == []
    empty = update_plan(ops=[{"op": "set_status", "match": "nope", "status": "completed"}])
    assert empty.startswith("Error:")
    assert _plan() == []


def test_ops_notes_bad_ops_without_crashing(_iso_tmp):
    from tools.MainTool.planTools import update_plan
    update_plan(plan=[{"step": "keep me", "status": "pending"}])
    out = update_plan(ops=[{"op": "frobnicate"},
                           {"op": "set_status", "match": "missing", "status": "completed"},
                           {"op": "set_status", "match": "keep", "status": "bogus"}])
    assert "unknown op" in out and "no step matches" in out
    # Invalid status falls back to pending; the step itself is untouched.
    assert _plan() == [{"step": "keep me", "status": "pending"}]


# ── update_plan stays generic (no orchestrator-specific fields) ─────────────


def test_update_plan_requires_something(_iso_tmp):
    from tools.MainTool.planTools import update_plan
    assert update_plan().startswith("Error:")
