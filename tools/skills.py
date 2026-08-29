"""
Skills warehouse.

Stage 1 of the mature-harness gap mega-plan. Per the user's direction:
"warehouse... but don't wire to anything yet, because i want to fill it later
since skill is going to be use by subagents (anything below dev, ui, deploy
subagents, so ui subagents later will get a skill stuff)".

What this module does:
- Discovers SKILL.md files at conventional locations.
- Parses YAML frontmatter into a structured record.
- Returns a plain dict-of-dicts that any future code (subagent loaders,
  prompt injectors, etc.) can consume.

What this module does NOT do (deliberately):
- Does not register a `read_skill` tool.
- Does not inject anything into any agent's system prompt.
- Does not change the existing flow in any way.
- Does not write to disk.

If `tools.skills` is never imported, nothing about the harness changes.
"""

from __future__ import annotations

import os
import re
from dataclasses import dataclass, field
from typing import Iterable, Optional

# Conventional discovery roots, in priority order (workspace > user > first-party).
# These mirror the existing per-workspace / per-user / first-party split that
# `general_profile.json` and `subagents.json` already use.
_WORKSPACE_SKILLS_GLOB = os.path.join(".deep_agents", "skills", "*", "SKILL.md")
_USER_SKILLS_GLOB = os.path.join(".deep_agents", "skills", "*", "SKILL.md")
_FIRSTPARTY_SKILLS_GLOB = os.path.join(".agents", "skills", "*", "SKILL.md")

_FRONTMATTER_RE = re.compile(r"\A---\s*\n(.*?)\n---\s*\n(.*)\Z", re.DOTALL)
_KV_RE = re.compile(r"^([A-Za-z0-9_\-]+)\s*:\s*(.*)$")


@dataclass
class Skill:
    """A discovered skill record."""

    name: str
    description: str
    body: str
    frontmatter: dict = field(default_factory=dict)
    source: str = ""           # absolute path of the SKILL.md file
    scope: str = ""            # "workspace" | "user" | "firstparty"
    allowed_tools: list[str] = field(default_factory=list)
    hidden: bool = False
    keywords: list[str] = field(default_factory=list)

    def to_dict(self) -> dict:
        return {
            "name": self.name,
            "description": self.description,
            "body": self.body,
            "frontmatter": dict(self.frontmatter),
            "source": self.source,
            "scope": self.scope,
            "allowed_tools": list(self.allowed_tools),
            "hidden": self.hidden,
            "keywords": list(self.keywords),
        }


def _parse_frontmatter(text: str) -> tuple[dict, str]:
    """Parse `---` frontmatter; return (frontmatter_dict, body). Empty dict if absent."""
    m = _FRONTMATTER_RE.match(text)
    if not m:
        return {}, text

    raw_fm, body = m.group(1), m.group(2)
    fm: dict = {}
    for line in raw_fm.splitlines():
        kv = _KV_RE.match(line)
        if not kv:
            continue
        key, value = kv.group(1).strip(), kv.group(2).strip()
        # Strip optional surrounding quotes.
        if len(value) >= 2 and value[0] == value[-1] and value[0] in ('"', "'"):
            value = value[1:-1]
        fm[key] = value
    return fm, body


def _parse_list_field(value: str) -> list[str]:
    """Parse `[a, b, c]` or comma-separated text into a list of trimmed strings."""
    if not value:
        return []
    s = value.strip()
    if s.startswith("[") and s.endswith("]"):
        s = s[1:-1]
    parts = [p.strip().strip('"').strip("'") for p in s.split(",")]
    return [p for p in parts if p]


def _parse_skill_file(path: str, scope: str) -> Optional[Skill]:
    try:
        with open(path, "r", encoding="utf-8") as f:
            text = f.read()
    except OSError:
        return None

    fm, body = _parse_frontmatter(text)
    name = fm.get("name") or os.path.basename(os.path.dirname(path))
    description = fm.get("description", "").strip()
    hidden_raw = fm.get("hidden", "false").strip().lower()
    hidden = hidden_raw in ("true", "1", "yes", "on")

    return Skill(
        name=name,
        description=description,
        body=body.strip(),
        frontmatter=fm,
        source=os.path.abspath(path),
        scope=scope,
        allowed_tools=_parse_list_field(fm.get("allowed-tools", "")),
        hidden=hidden,
        keywords=_parse_list_field(fm.get("keywords", "")),
    )


def _iter_skill_paths(roots: Iterable[tuple[str, str]]) -> Iterable[tuple[str, str]]:
    """Yield (path, scope) for every SKILL.md under the given root directories.

    Each tuple is `(root_path, scope_label)`. Walks one level deep:
    `<root>/<prefix>/skills/<name>/SKILL.md` where `prefix` is determined
    by the scope.
    """
    import glob as _glob

    scope_prefix = {"firstparty": ".agents", "user": ".deep_agents", "workspace": ".deep_agents"}

    for root_path, scope in roots:
        if not root_path or not os.path.isdir(root_path):
            continue
        prefix = scope_prefix.get(scope, ".deep_agents")
        pattern = os.path.join(root_path, prefix, "skills", "*", "SKILL.md")
        for match in sorted(_glob.glob(pattern)):
            yield match, scope


def discover_skills(
    workspace_path: Optional[str] = None,
    harness_root: Optional[str] = None,
    include_hidden: bool = False,
) -> dict[str, dict]:
    """Discover skills from all conventional locations.

    Priority order (later wins on name collision):
      1. first-party: `<harness_root>/.agents/skills/<name>/SKILL.md`
      2. user:        `<harness_root>/.deep_agents/skills/<name>/SKILL.md`
      3. workspace:   `<workspace_path>/.deep_agents/skills/<name>/SKILL.md`

    Returns `{name: skill.to_dict()}`. Hidden skills are excluded unless
    `include_hidden=True`.
    """
    if harness_root is None:
        harness_root = os.environ.get("HARNESS_ROOT") or os.getcwd()

    roots: list[tuple[str, str]] = []
    if harness_root:
        roots.append((harness_root, "firstparty"))
        roots.append((harness_root, "user"))
    if workspace_path and os.path.isdir(workspace_path):
        roots.append((workspace_path, "workspace"))

    out: dict[str, dict] = {}
    for path, scope in _iter_skill_paths(roots):
        # Skip the first-party "user" double-claim for the same harness root.
        if scope == "user" and harness_root and os.path.commonpath(
            [os.path.abspath(path), os.path.abspath(harness_root)]
        ) == os.path.abspath(harness_root):
            # Same root, same dir — first-party already covered.
            continue
        skill = _parse_skill_file(path, scope)
        if skill is None:
            continue
        if skill.hidden and not include_hidden:
            continue
        out[skill.name] = skill.to_dict()
    return out


if __name__ == "__main__":
    # CLI check: `python -m tools.skills [workspace]`
    import json
    import sys

    ws = sys.argv[1] if len(sys.argv) > 1 else os.getcwd()
    found = discover_skills(workspace_path=ws)
    print(json.dumps(found, indent=2))
    print(f"\n[skills] {len(found)} skill(s) discovered under {ws}")
