"""agents.group.manifest — the authored half of a project (Phase 2).

``docs/group-projects/01-concept-model.md`` section 10.

``PROJECT.md`` is the one file in a project a human is expected to read and
edit. It is **authored state**: prose for people, plus one fenced YAML block
for the harness. It is never a source of run state — that lives in
``.gp/state.db`` (the two-state rule, ``01`` section 3).

    # Landing page for the coffee business

    ## Why
    ...
    ## Done when
    - [ ] artifacts/index.html builds
    ## Milestones
    ...
    ---
    ```yaml
    schema: 1
    id: landing-page-coffee
    milestones: [...]
    completion: {...}
    ```

Both halves are kept: the prose is what the human maintains, the YAML is what
the supervisor reads. Editing the prose alone is always safe.
"""
from __future__ import annotations

import json
import os
import re
from dataclasses import dataclass, field
from typing import Any

try:  # PyYAML is already a repo dependency
    import yaml
except ImportError:  # pragma: no cover
    yaml = None

#: Fenced block carrying the machine-readable half. Last one wins.
_YAML_FENCE = re.compile(
    r"^```ya?ml\s*\n(?P<body>.*?)\n```\s*$", re.DOTALL | re.MULTILINE
)

SCHEMA_VERSION = 1


class ManifestError(Exception):
    """The manifest is missing or malformed."""


@dataclass
class Manifest:
    """Parsed ``PROJECT.md``."""

    title: str = ""
    prose: str = ""
    data: dict = field(default_factory=dict)

    # ── accessors ───────────────────────────────────────────────────────

    @property
    def id(self) -> str:
        return str(self.data.get("id") or "")

    @property
    def owner(self) -> str:
        return str(self.data.get("owner") or "localhost")

    @property
    def goal(self) -> str:
        return str(self.data.get("goal") or self.title)

    @property
    def budget_usd(self) -> float | None:
        value = self.data.get("budget_usd")
        return float(value) if value is not None else None

    @property
    def budget_tokens(self) -> int | None:
        value = self.data.get("budget_tokens")
        return int(value) if value is not None else None

    @property
    def deadline(self) -> float | None:
        value = self.data.get("deadline")
        return float(value) if value is not None else None

    @property
    def completion(self) -> dict:
        return dict(self.data.get("completion") or {"kind": "all_milestones_done"})

    @property
    def autonomy(self) -> dict:
        return dict(self.data.get("autonomy") or {})

    @property
    def provider(self) -> dict:
        return dict(self.data.get("provider") or {})

    @property
    def milestones(self) -> list[dict]:
        return list(self.data.get("milestones") or [])

    @property
    def members(self) -> list[dict]:
        return list(self.data.get("members") or [])

    @property
    def environment(self) -> dict:
        return dict(self.data.get("environment") or {})

    @property
    def idempotency(self) -> dict:
        return dict(self.data.get("idempotency") or {})

    def agent_spec(self, name: str) -> dict | None:
        for member in self.members:
            if str(member.get("name") or "") == name:
                return dict(member)
        return None

    def to_plan(self) -> dict:
        """The shape ``project.created`` expects."""
        return {
            "goal": self.goal,
            "owner": self.owner,
            "budget_usd": self.budget_usd,
            "budget_tokens": self.budget_tokens,
            "deadline": self.deadline,
            "completion": self.completion,
            "milestones": self.milestones,
        }

    def validate(self) -> None:
        """Raise :class:`ManifestError` on anything the harness cannot run.

        Deliberately strict. An item without acceptance criteria can never
        reach ``ready`` (DM-06), so catching it at creation beats discovering
        it as a silent deadlock hours later.
        """
        if not self.id:
            raise ManifestError("manifest is missing `id`")
        if not self.milestones:
            raise ManifestError("manifest declares no milestones")
        seen_milestones: set[str] = set()
        for milestone in self.milestones:
            mid = str(milestone.get("id") or "")
            if not mid:
                raise ManifestError("a milestone is missing `id`")
            if mid in seen_milestones:
                raise ManifestError(f"duplicate milestone id {mid!r}")
            seen_milestones.add(mid)
            for dep in milestone.get("depends_on") or []:
                if str(dep) not in seen_milestones and str(dep) not in {
                    str(m.get("id") or "") for m in self.milestones
                }:
                    raise ManifestError(
                        f"milestone {mid!r} depends on unknown {dep!r}"
                    )
        for milestone in self.milestones:
            mid = str(milestone.get("id") or "")
            for item in milestone.get("items") or []:
                item_id = str(item.get("id") or "")
                if not item_id:
                    raise ManifestError(f"milestone {mid!r} has an item without `id`")
                if not (item.get("acceptance") or []):
                    raise ManifestError(
                        f"item {item_id!r} has no acceptance criteria; it could "
                        f"never reach ready (DM-06)"
                    )
        for member in self.members:
            if not str(member.get("name") or ""):
                raise ManifestError("a member is missing `name`")
            if not str(member.get("derived_from") or ""):
                raise ManifestError(
                    f"member {member.get('name')!r} is missing `derived_from`"
                )


def parse(text: str) -> Manifest:
    """Parse ``PROJECT.md`` text. The YAML block is authoritative."""
    if yaml is None:  # pragma: no cover
        raise ManifestError("PyYAML is required to parse a manifest")
    match = None
    for candidate in _YAML_FENCE.finditer(text):
        match = candidate  # last fence wins, so a quoted example is harmless
    prose = text
    data: dict = {}
    if match:
        prose = text[: match.start()].rstrip()
        try:
            loaded = yaml.safe_load(match.group("body")) or {}
        except yaml.YAMLError as exc:
            raise ManifestError(f"manifest YAML is invalid: {exc}") from exc
        if not isinstance(loaded, dict):
            raise ManifestError("manifest YAML must be a mapping")
        data = loaded

    title = ""
    for line in prose.splitlines():
        if line.startswith("# "):
            title = line[2:].strip()
            break
    if not data.get("id") and title:
        data["id"] = re.sub(r"[^a-z0-9]+", "-", title.lower()).strip("-")
    if not data.get("goal"):
        goal = re.search(r"^##\s+Why\s*\n(.*?)(?=\n##|\Z)", prose,
                         re.DOTALL | re.MULTILINE)
        if goal:
            data["goal"] = " ".join(goal.group(1).split())

    return Manifest(title=title, prose=prose, data=data)


def render(manifest: Manifest) -> str:
    """Serialise back to markdown. Round-trips through :func:`parse`."""
    if yaml is None:  # pragma: no cover
        raise ManifestError("PyYAML is required to render a manifest")
    body = yaml.safe_dump(manifest.data, sort_keys=False, allow_unicode=True,
                          default_flow_style=False).rstrip()
    prose = manifest.prose.rstrip()
    if not prose:
        prose = f"# {manifest.title or manifest.id}"
    return f"{prose}\n\n---\n```yaml\n{body}\n```\n"


def load(path: str) -> Manifest:
    if not os.path.isfile(path):
        raise ManifestError(f"no manifest at {path}")
    with open(path, encoding="utf-8") as handle:
        return parse(handle.read())


def save(path: str, manifest: Manifest) -> None:
    os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(render(manifest))


def autonomy_from_yaml(text: str) -> dict:
    """Empty-safe helper for callers that only need the envelope."""
    try:
        return parse(text).autonomy
    except ManifestError:
        return {}


def dumps(data: Any) -> str:
    """Stable JSON for the files materialise.py writes."""
    return json.dumps(data, indent=2, sort_keys=False, default=str) + "\n"
