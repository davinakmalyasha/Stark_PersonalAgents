"""Shared offline test harness for the group/offline suites.

Never touches the network: the heavy `tools` facade is replaced by a stub
for tests that need to import a standalone module.
"""
from __future__ import annotations

import contextlib
import importlib.util
import os
import shutil
import subprocess
import sys
import tempfile
import types

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if REPO_ROOT not in sys.path:
    sys.path.insert(0, REPO_ROOT)


class Skip(Exception):
    """Raise inside a test to mark it skipped (e.g. git not available)."""


# ── module loading (bypasses the tools package facade) ─────────────────────

def _load_module(name: str, path: str):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


_TOOLS_STUB = None
_GT = None
_CG = None


def github_tools():
    """tools/AddOnTool/githubTools.py loaded standalone with fake-able _gh_api."""
    global _GT
    if _GT is None:
        _GT = _load_module("gh_tools_test",
                           os.path.join(REPO_ROOT, "tools", "AddOnTool", "githubTools.py"))
    return _GT


def ci_gate():
    """Testing_Log/Testing/ci_gate.py loaded standalone (needs branch_levels importable)."""
    global _CG
    if _CG is None:
        _CG = _load_module("ci_gate_test",
                           os.path.join(REPO_ROOT, "Testing_Log", "Testing", "ci_gate.py"))
    return _CG


def tools_stub():
    """Minimal `tools` module stub with inert harness bits."""
    global _TOOLS_STUB
    if _TOOLS_STUB is not None:
        return _TOOLS_STUB
    stub = types.ModuleType("tools")
    stub._HARNESS_ROOT = REPO_ROOT
    stub.enforce_fs_permission = lambda *a, **k: None
    stub.shared_state = {}
    stub.ask_user = lambda *a, **k: []
    stub._increment_cache_version = lambda *a, **k: None
    sys.modules["tools"] = stub
    _TOOLS_STUB = stub
    return _TOOLS_STUB


def restore_tools() -> None:
    """Evict the ``tools`` stub so the REAL package imports again.

    :func:`tools_stub` replaces ``sys.modules['tools']`` with a minimal module
    and (deliberately) never restores it, because the suites that use it need
    it for their whole module. Any LATER test that wants the real facade must
    therefore evict it first — otherwise ``import tools`` silently yields the
    stub and every attribute lookup fails with a confusing ``AttributeError``.

    Clearing the cached stub means the next :func:`tools_stub` call rebuilds
    and reinstalls it, so this is safe to call in either order.
    """
    global _TOOLS_STUB
    current = sys.modules.get("tools")
    if current is None or current is _TOOLS_STUB or not hasattr(current, "__path__"):
        # Drop the stub and anything imported through it (e.g. a submodule the
        # stub briefly satisfied) so the real package re-imports cleanly.
        for name in [n for n in list(sys.modules)
                     if n == "tools" or n.startswith("tools.")]:
            sys.modules.pop(name, None)
    _TOOLS_STUB = None


def real_tools():
    """The real ``tools`` package, evicting the stub if one is installed."""
    restore_tools()
    import tools
    return tools


@contextlib.contextmanager
def agents_shim(*module_names):
    """Import ``agents.<x>`` submodules WITHOUT executing ``agents/__init__.py``.

    ``agents/__init__.py`` pulls the whole agent stack (``developer_agent`` ->
    ``from tools import ...``). This suite replaces ``sys.modules['tools']``
    with a stub via :func:`tools_stub` and never restores it, so a plain
    ``import agents.foo`` raises ``ImportError: cannot import name
    'ToolExecutionResult' from 'tools'`` depending on test order. A path-only
    package object sidesteps the ``__init__`` entirely — and is ~12s faster
    than importing the real stack.

    Yields ``{module_name: module}``. Every touched ``sys.modules`` entry is
    restored on exit, with ``agents`` restored last so the tree is never left
    half-swapped.
    """
    names = ["agents", *module_names]
    saved = {name: sys.modules.get(name) for name in names}

    pkg = types.ModuleType("agents")
    pkg.__path__ = [os.path.join(REPO_ROOT, "agents")]
    sys.modules["agents"] = pkg
    try:
        yield {name: importlib.import_module(name) for name in module_names}
    finally:
        # Submodules first, then the package itself, so `agents` is never a
        # shim pointing at a restored child (or vice versa).
        for name in sorted(saved, key=lambda n: n == "agents"):
            module = saved[name]
            if module is None:
                sys.modules.pop(name, None)
            else:
                sys.modules[name] = module


# ── env isolation ───────────────────────────────────────────────────────────

_CI_ENV_KEYS = ("CI_DEV_BRANCH", "CI_STAGING_BRANCH", "CI_PROD_BRANCHES")


@contextlib.contextmanager
def clean_ci_env(**overrides):
    """Remove CI_* gate vars for the block, optionally setting overrides."""
    saved = {k: os.environ.pop(k, None) for k in _CI_ENV_KEYS}
    for k, v in overrides.items():
        os.environ[k] = v
    try:
        yield
    finally:
        for k in _CI_ENV_KEYS:
            os.environ.pop(k, None)
        for k, v in saved.items():
            if v is not None:
                os.environ[k] = v
