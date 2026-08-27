"""Shared offline test harness for the template/gate test suites.

Zero-dependency (stdlib + jinja2/pyyaml like the production validators).
Never touches the network: githubTools is exercised through a scripted
fake `_gh_api`, and the heavy `tools` facade is replaced by a stub module
that carries the REAL apply_template so lazy imports inside
templateTools/githubTools resolve without deepagents.
"""
from __future__ import annotations

import contextlib
import importlib.util
import json
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
_TT = None
_GT = None
_CG = None


def template_tools():
    """tools/AddOnTool/templateTools.py loaded standalone (lazy imports intact)."""
    global _TT
    if _TT is None:
        tools_stub()  # ensure sys.modules['tools'] exists before any lazy import
        _TT = _load_module("tpl_tools_test",
                           os.path.join(REPO_ROOT, "tools", "AddOnTool", "templateTools.py"))
        # the stub must expose the REAL renderer: githubTools._seed_gate does
        # `from tools import apply_template` at call time
        _TOOLS_STUB.apply_template = _TT.apply_template
    return _TT


def github_tools():
    """tools/AddOnTool/githubTools.py loaded standalone with fake-able _gh_api."""
    global _GT
    if _GT is None:
        template_tools()  # ensures the stub carries the real apply_template
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
    """Minimal `tools` module stub: the real apply_template plus inert harness bits."""
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


def attach_real_apply_template():
    """Point the stub's apply_template at the real renderer (used by _seed_gate)."""
    stub = tools_stub()
    stub.apply_template = template_tools().apply_template
    return stub


# ── env isolation ───────────────────────────────────────────────────────────

_CI_ENV_KEYS = ("CI_DEV_BRANCH", "CI_STAGING_BRANCH", "CI_PROD_BRANCHES", "CI_GATE_BRANCH")


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


# ── tier template rendering (real renderer, real metadata) ─────────────────

def render_template(name: str, extra: dict | None = None) -> tuple[dict, dict[str, str]]:
    """Render a catalog template's files with sample_vars (+defaults, +extra).

    Returns (variables_used, {dest_rel: rendered_text})."""
    tt = template_tools()
    meta = tt._template_meta(name)
    assert meta and "error" not in meta, f"template {name} missing/broken: {meta}"
    variables = {}
    for v in meta.get("variables", []):
        if not v.get("required") and "default" in v:
            variables[v["name"]] = v["default"]
    variables.update(meta.get("sample_vars") or {})
    if extra:
        variables.update(extra)
    tdir = tt._template_dir(name)
    out = {}
    for fspec in meta.get("files", []):
        src = os.path.join(tdir, "files", fspec["src"])
        with open(src, encoding="utf-8") as f:
            text = f.read()
        out[fspec["dest"]] = tt._render_str(text, dict(variables), meta.get("delimiters"))
    return variables, out


def parse_workflow_yaml(text: str) -> dict:
    """yaml.safe_load with the bare-`on`-parses-as-True quirk handled."""
    import yaml
    data = yaml.safe_load(text)
    assert isinstance(data, dict), f"workflow did not parse to a mapping: {type(data)}"
    return data


def triggers_of(rendered_text: str) -> dict:
    """The `on:` block of a rendered workflow (PyYAML reads bare on: as True)."""
    data = parse_workflow_yaml(rendered_text)
    return data.get(True) or data.get("on") or {}


def jobs_of(rendered_text: str) -> dict:
    return parse_workflow_yaml(rendered_text).get("jobs") or {}


def concurrency_of(rendered_text: str) -> dict:
    """Top-level concurrency block (sibling of on:/jobs:, NOT under on:)."""
    return parse_workflow_yaml(rendered_text).get("concurrency") or {}


# ── temp workspace helper ───────────────────────────────────────────────────

@contextlib.contextmanager
def temp_dir(prefix="gatetest-"):
    d = tempfile.mkdtemp(prefix=prefix)
    try:
        yield d
    finally:
        shutil.rmtree(d, ignore_errors=True)


def write_file(path: str, content: str) -> None:
    os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(content)


# ── git helpers for the ci_gate push matrix (local bare remotes) ────────────

def _git_run(cwd: str, args: list, timeout: int = 120) -> tuple[int, str]:
    try:
        r = subprocess.run(["git", "-C", cwd, *args], capture_output=True, text=True,
                           timeout=timeout, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        return r.returncode, (r.stdout or "") + (r.stderr or "")
    except FileNotFoundError:
        raise Skip("git not available")
    except subprocess.TimeoutExpired:
        return -1, "timeout"


def git_ok(cwd: str, *args, timeout: int = 120) -> str:
    rc, out = _git_run(cwd, list(args), timeout=timeout)
    assert rc == 0, f"git {' '.join(args)} failed rc={rc}: {out[:400]}"
    return out


def require_git() -> None:
    rc, _ = _git_run(".", ["--version"])
    if rc != 0:
        raise Skip("git not available")


FAKE_ORIGIN = "https://github.com/testowner/testrepo.git"


def make_repo_pair(base: str, name: str) -> tuple[str, str]:
    """Bare remote + worktree whose origin URL looks like GitHub but rewrites
    (via insteadOf) to the local bare path — exercises the real
    remote_owner_name parsing while staying fully offline."""
    remote = os.path.join(base, f"{name}.git")
    ws = os.path.join(base, name + "-ws")
    os.makedirs(remote)
    os.makedirs(ws)
    git_ok(remote, "init", "--bare")
    git_ok(ws, "-c", "init.defaultBranch=main", "init")
    git_ok(ws, "config", "user.name", "TestAgent")
    git_ok(ws, "config", "user.email", "agent@test.local")
    git_ok(ws, "remote", "add", "origin", FAKE_ORIGIN)
    # URL rewrite: github URL -> local bare path (fetch AND push)
    git_ok(ws, "config", f"url.{remote.replace(os.sep, '/')}.insteadOf", FAKE_ORIGIN)
    return remote, ws


def initial_commit_and_push(ws: str, branch: str = "main") -> str:
    write_file(os.path.join(ws, "README.md"), "seed\n")
    git_ok(ws, "add", "-A")
    git_ok(ws, "commit", "-m", "seed")
    git_ok(ws, "push", "-u", "origin", branch)
    return git_ok(ws, "rev-parse", "HEAD").strip()


def commit_file(ws: str, rel: str, content: str, msg: str = "change") -> str:
    write_file(os.path.join(ws, rel), content)
    git_ok(ws, "add", "-A")
    git_ok(ws, "commit", "-m", msg)
    return git_ok(ws, "rev-parse", "HEAD").strip()


def remote_heads(remote: str) -> list[str]:
    out = git_ok(remote, "ls-remote", "--heads", ".")
    heads = []
    for ln in out.splitlines():
        if "\t" in ln and ln.split("\t", 1)[1].startswith("refs/heads/"):
            heads.append(ln.split("\t", 1)[1][len("refs/heads/"):])
    return heads


def remote_sha(remote: str, branch: str) -> str:
    return git_ok(remote, "rev-parse", branch).strip()
