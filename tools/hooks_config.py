"""
Stage 2: opt-in user-defined hooks via `.deep_agents/hooks.json`.

Per the mega-plan: the existing `register_pre_hook` / `register_post_hook`
machinery is powerful (veto-capable, can rewrite results) but only
registerable from Python at startup. This module surfaces it via JSON.

Behavior:
- On startup, look for `.deep_agents/hooks.json` in the harness root
  (or under the user's home). If absent, no hooks are registered and
  behavior is identical to today.
- The file is a list of rules. Each rule has:
  - `when`: a tool-name glob (e.g. "run_command", "edit_file", "*")
  - `where`: a regex matched against the rendered args string
  - `action`: "log" (append to live log + a hooks-audit file) or
              "veto" (return `[BLOCKED] <reason>`)
  - `reason`: human-readable explanation (required for `veto`)

The file is hot-reloaded on first call; mutating it requires a process
restart. (Hot-reload is deliberately out of scope; this is a v1.)

This is **additive** by design. Default = no file = no hooks = same as
today.
"""

from __future__ import annotations

import glob as _glob
import json
import os
import re
from dataclasses import dataclass
from typing import Optional


_DEFAULT_PATH_CANDIDATES = [
    ".deep_agents/hooks.json",
    os.path.join("~", ".deep_agents", "hooks.json"),
]


@dataclass
class HookRule:
    when: str          # tool-name glob
    where: str         # regex against rendered args
    action: str        # "log" | "veto"
    reason: str = ""

    def matches(self, tool_name: str, args_repr: str) -> bool:
        if not _glob.fnmatch.fnmatchcase(tool_name, self.when):
            return False
        if self.where:
            try:
                if not re.search(self.where, args_repr):
                    return False
            except re.error:
                return False
        return True


def _candidate_paths(harness_root: Optional[str] = None) -> list[str]:
    roots = []
    if harness_root:
        roots.append(harness_root)
        roots.append(os.path.dirname(harness_root))
    roots.append(os.getcwd())
    out: list[str] = []
    for root in roots:
        for cand in _DEFAULT_PATH_CANDIDATES:
            p = os.path.expanduser(cand)
            if not os.path.isabs(p):
                p = os.path.join(root, p)
            out.append(p)
    # also try relative to this file's grandparent
    out.append(os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), ".deep_agents", "hooks.json"))
    return out


def load_hooks(harness_root: Optional[str] = None) -> list[HookRule]:
    """Load user-defined hooks from `.deep_agents/hooks.json`.

    Returns an empty list if the file does not exist or is malformed
    (errors are surfaced to the live log so the user knows)."""
    for path in _candidate_paths(harness_root):
        if not os.path.isfile(path):
            continue
        try:
            with open(path, "r", encoding="utf-8") as f:
                raw = json.load(f)
        except (OSError, json.JSONDecodeError) as e:
            _log_hooks_error(f"hooks.json at {path} unreadable: {e}")
            return []
        rules = raw.get("rules") if isinstance(raw, dict) else raw
        if not isinstance(rules, list):
            _log_hooks_error(f"hooks.json at {path}: expected a list or {{'rules': [...]}}")
            return []
        out: list[HookRule] = []
        for i, r in enumerate(rules):
            if not isinstance(r, dict):
                _log_hooks_error(f"hooks.json rule #{i} is not an object: {r!r}")
                continue
            try:
                out.append(HookRule(
                    when=str(r.get("when", "*")),
                    where=str(r.get("where", "")),
                    action=str(r.get("action", "log")),
                    reason=str(r.get("reason", "")),
                ))
            except Exception as e:
                _log_hooks_error(f"hooks.json rule #{i} parse error: {e}")
        return out
    return []


def _log_hooks_error(msg: str) -> None:
    try:
        from sync.state_sync import safe_append_live_log
        safe_append_live_log(f"\n[HOOKS] {msg}")
    except Exception:
        pass


def _format_args(args: dict) -> str:
    try:
        return json.dumps(args or {}, default=str, sort_keys=True)
    except Exception:
        return str(args)


def pre_hook_from_rules(rules: list[HookRule]):
    """Returns a pre-hook function for the registry that consults the
    rule list. Returns `None` if no `veto` rules are present (so the
    registry doesn't pay any cost)."""
    veto_rules = [r for r in rules if r.action == "veto"]
    if not veto_rules:
        return None

    def _hook(ctx: dict):
        name = ctx.get("name", "")
        args_repr = _format_args(ctx.get("args") or {})
        for r in veto_rules:
            if r.matches(name, args_repr):
                return ("deny", f"hooks.json veto: {r.reason or r.when}")
        return None

    return _hook


def post_hook_from_rules(rules: list[HookRule]):
    """Returns a post-hook function that logs matching calls. Always safe
    to register — only emits a `[HOOK]` line on match."""
    log_rules = [r for r in rules if r.action == "log"]
    if not log_rules:
        return None

    def _hook(ctx: dict, result: str, is_error: bool):
        name = ctx.get("name", "")
        args_repr = _format_args(ctx.get("args") or {})
        for r in log_rules:
            if r.matches(name, args_repr):
                _log_hooks_error(f"matched rule {r.when!r}: tool={name} args_preview={args_repr[:160]}")
                break
        return None

    return _hook
