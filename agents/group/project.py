"""agents.group.project — the project folder (Phase 2).

``docs/group-projects/01-concept-model.md`` section 2.

A group project **is** a folder. This module creates it and — more usefully —
*checks* the two rules that decide whether it can leave this machine:

- **P-1** every path in a project file is relative to the project root
- **P-2** credentials appear as environment-variable names, never values

Both are enforced by :func:`check_portability`, which the creation path calls
so a folder cannot be born un-portable, and tests re-run after a copy.

Layout::

    <project>/
      PROJECT.md   AGENTS.md   inbox.md
      agents/      skills/     tools/bindings.json
      mcp/servers.json         deps/
      .gp/state.db .gp/plan.md .gp/lock.json .gp/runs/
      context/decisions.md  context/findings.md  context/artifacts/
"""
from __future__ import annotations

import json
import os
import re
from dataclasses import dataclass
from typing import Any

from .manifest import Manifest, dumps
from .manifest import save as save_manifest

#: Where project folders live. A cloud host points this at a persistent
#: volume; nothing else about the folder changes (``05`` section 4.3).
DEFAULT_PROJECTS_ROOT = os.environ.get("STARK_PROJECTS_ROOT", r"D:\StarkProjects")

SCHEMA = 1

#: Windows drive-qualified path, the most common accidental absolute path.
_WINDOWS_ABS = re.compile(r"[A-Za-z]:[\\/]")
_UNIX_ABS = re.compile(r"(?<![\w:/])/(?:home|Users|root|var|etc|opt|srv|mnt|tmp)/")

_TEXT_SUFFIXES = (".md", ".json", ".txt", ".yaml", ".yml")

AGENTS_MD_TEMPLATE = """# Notes for the members of {title}

Everything here is read by every member at wake. Keep it short and current.

## Standing context
- (what this project is and why, in your own words)

## Constraints
- (anything a member must not do, beyond the autonomy envelope)

## Conventions
- (output format, naming, where things go)
"""


@dataclass
class ProjectPaths:
    """Every place a project keeps something. All derived from ``root``."""

    root: str
    project_dir: str

    @property
    def manifest(self) -> str:
        return os.path.join(self.project_dir, "PROJECT.md")

    @property
    def agents_md(self) -> str:
        return os.path.join(self.project_dir, "AGENTS.md")

    @property
    def inbox(self) -> str:
        return os.path.join(self.project_dir, "inbox.md")

    @property
    def agents_dir(self) -> str:
        return os.path.join(self.project_dir, "agents")

    @property
    def skills_dir(self) -> str:
        return os.path.join(self.project_dir, "skills")

    @property
    def tools_dir(self) -> str:
        return os.path.join(self.project_dir, "tools")

    @property
    def mcp_dir(self) -> str:
        return os.path.join(self.project_dir, "mcp")

    @property
    def deps_dir(self) -> str:
        return os.path.join(self.project_dir, "deps")

    @property
    def gp_dir(self) -> str:
        return os.path.join(self.project_dir, ".gp")

    @property
    def state_db(self) -> str:
        return os.path.join(self.gp_dir, "state.db")

    @property
    def plan_md(self) -> str:
        return os.path.join(self.gp_dir, "plan.md")

    @property
    def lock_json(self) -> str:
        return os.path.join(self.gp_dir, "lock.json")

    @property
    def runs_dir(self) -> str:
        return os.path.join(self.gp_dir, "runs")

    @property
    def context_dir(self) -> str:
        return os.path.join(self.project_dir, "context")

    @property
    def artifacts_dir(self) -> str:
        return os.path.join(self.context_dir, "artifacts")

    @property
    def decisions_md(self) -> str:
        return os.path.join(self.context_dir, "decisions.md")

    @property
    def findings_md(self) -> str:
        return os.path.join(self.context_dir, "findings.md")


def paths_for(root: str, project_id: str) -> ProjectPaths:
    return ProjectPaths(root=root, project_dir=os.path.join(root, project_id))


def _write(path: str, text: str) -> str:
    os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(text)
    return path


def scaffold(paths: ProjectPaths, manifest: Manifest) -> ProjectPaths:
    """Create the folder tree and the authored files. Idempotent.

    Safe to re-run: existing files are left alone, so calling this to repair a
    half-created folder does not discard anything a human wrote.
    """
    for directory in (
        paths.project_dir, paths.agents_dir, paths.skills_dir, paths.tools_dir,
        paths.mcp_dir, paths.deps_dir, paths.gp_dir, paths.runs_dir,
        paths.context_dir, paths.artifacts_dir,
    ):
        os.makedirs(directory, exist_ok=True)

    if not os.path.isfile(paths.manifest):
        save_manifest(paths.manifest, manifest)
    if not os.path.isfile(paths.agents_md):
        _write(paths.agents_md,
               AGENTS_MD_TEMPLATE.format(title=manifest.title or manifest.id))
    if not os.path.isfile(paths.inbox):
        _write(paths.inbox, "# Inbox\n\n"
                            "## PENDING APPROVAL\n\n_(none)_\n\n"
                            "## OPEN QUESTIONS\n\n_(none)_\n")
    if not os.path.isfile(paths.decisions_md):
        _write(paths.decisions_md,
               "# Decisions\n\nADR-style and append-only. "
               "One `## <date> — <decision>` per entry: what was chosen, what "
               "was rejected, and why.\n")
    if not os.path.isfile(paths.findings_md):
        _write(paths.findings_md,
               "# Findings\n\nFacts the group established, with sources. "
               "Append-only.\n")
    return paths


def write_lock(paths: ProjectPaths, *, harness_version: str,
               digests: dict | None = None) -> str:
    """``.gp/lock.json`` — what this folder was built against (P-6).

    The lock is what makes a folder *reproducible* rather than merely
    self-describing: without it, regenerating a materialised agent means
    guessing which harness version produced it.
    """
    return _write(paths.lock_json, dumps({
        "schema": SCHEMA,
        "harness_version": harness_version,
        "digests": dict(digests or {}),
    }))


def write_plan(paths: ProjectPaths, store: Any, project_id: str) -> str:
    """``.gp/plan.md`` — a rendered view of the board, for humans.

    Explicitly a **view**: never parsed back, never a source of truth
    (``01`` section 2.2). It exists so the folder explains itself to someone
    opening it without the app.
    """
    project = store.project(project_id) or {}
    milestones = store.milestones(project_id)
    items = store.items(project_id)
    lines = [f"# Plan — {project_id}", ""]
    if project:
        lines.append(
            f"status **{project.get('status')}** · "
            f"spent ${float(project.get('spent_usd') or 0):.2f}"
            + (f" / ${float(project['budget_usd']):.2f}"
               if project.get("budget_usd") else "")
        )
        lines.append("")
    done_milestones = sum(1 for m in milestones if m["status"] == "done")
    done_items = sum(1 for i in items if i["status"] == "done")
    lines.append(f"milestones {done_milestones}/{len(milestones)} · "
                 f"items {done_items}/{len(items)}")
    lines.append("")
    for milestone in milestones:
        lines.append(f"## {milestone['id']} — {milestone['title']} "
                     f"[{milestone['status']}]")
        deps = milestone.get("depends_on") or []
        if deps:
            lines.append(f"_depends on: {', '.join(deps)}_")
        for item in items:
            if item["milestone_id"] != milestone["id"]:
                continue
            marker = {"done": "x", "doing": "~", "failed": "!",
                      "invalidated": "?", "blocked": "."}.get(item["status"], " ")
            lines.append(f"- [{marker}] {item['id']} — {item['title']} "
                         f"({item['status']})")
        lines.append("")
    return _write(paths.plan_md, "\n".join(lines))


# ── the portability contract, executable ────────────────────────────────────


def _scan_json_strings(value: Any, path: str = "") -> list[tuple[str, str]]:
    found: list[tuple[str, str]] = []
    if isinstance(value, dict):
        for key, child in value.items():
            found.extend(_scan_json_strings(child, f"{path}.{key}" if path else str(key)))
    elif isinstance(value, list):
        for index, child in enumerate(value):
            found.extend(_scan_json_strings(child, f"{path}[{index}]"))
    elif isinstance(value, str):
        absolute = _WINDOWS_ABS.search(value) or (
            value.startswith("/") and len(value) > 1)
        if absolute:
            found.append((path, value))
    return found


def check_portability(paths: ProjectPaths) -> list[str]:
    """Return a list of violations. Empty means the folder can move.

    P-1 is checked structurally for JSON (every string value) and textually for
    markdown. P-2 reuses the MCP validator's rules and additionally refuses any
    file that contains something that looks like a live credential.
    """
    from .materialize import _SECRET_LOOKING

    problems: list[str] = []
    scanned_roots = [
        paths.agents_dir, paths.skills_dir, paths.tools_dir, paths.mcp_dir,
        paths.deps_dir, paths.context_dir,
    ]
    files: list[str] = [paths.manifest, paths.agents_md]
    for base in scanned_roots:
        for dirpath, _dirnames, filenames in os.walk(base):
            files.extend(os.path.join(dirpath, f) for f in filenames)

    for file_path in files:
        if not file_path.endswith(_TEXT_SUFFIXES) or not os.path.isfile(file_path):
            continue
        relative = os.path.relpath(file_path, paths.project_dir)
        try:
            with open(file_path, encoding="utf-8", errors="replace") as handle:
                text = handle.read()
        except OSError:
            continue

        if file_path.endswith(".json"):
            try:
                payload = json.loads(text)
            except (TypeError, ValueError):
                payload = None
            if payload is not None:
                for where, value in _scan_json_strings(payload):
                    problems.append(
                        f"P-1 {relative}:{where} holds an absolute path {value!r}"
                    )
                continue

        for line_number, line in enumerate(text.splitlines(), 1):
            if _WINDOWS_ABS.search(line) or _UNIX_ABS.search(line):
                problems.append(
                    f"P-1 {relative}:{line_number} holds an absolute path"
                )
        match = _SECRET_LOOKING.search(text)
        if match:
            problems.append(
                f"P-2 {relative} contains something that looks like a live "
                f"credential ({match.group(0)[:8]}...)"
            )
    return problems


def assert_portable(paths: ProjectPaths) -> None:
    """Raise :class:`PortabilityError` if the folder could not move."""
    problems = check_portability(paths)
    if problems:
        raise PortabilityError(
            "project folder is not portable:\n  - " + "\n  - ".join(problems)
        )


class PortabilityError(Exception):
    """The folder violates the portability contract (P-1 / P-2)."""


def copy_to(paths: ProjectPaths, new_root: str) -> ProjectPaths:
    """Move a project to another root, as a **copy**.

    Only ever called with the store closed — never rsync a live SQLite file.
    WAL mode plus a mid-write copy is how a project gets corrupted
    (``06`` section 2.5).
    """
    import shutil

    target = paths_for(new_root, os.path.basename(paths.project_dir))
    if os.path.exists(target.project_dir):
        raise PortabilityError(f"{target.project_dir} already exists")
    os.makedirs(new_root, exist_ok=True)
    shutil.copytree(paths.project_dir, target.project_dir)
    return target


# ── creation ────────────────────────────────────────────────────────────────


def create_group_project(root: str, manifest: Manifest, *,
                         spec_lookup: Any = None,
                         profile_lookup: Any = None,
                         skill_lookup: Any = None,
                         mcp_catalogue: dict | None = None) -> tuple[ProjectPaths, Any]:
    """Create a project folder, materialise its capabilities, seed the log.

    Returns ``(paths, store)``. ``spec_lookup(key) -> WorkspaceSpec`` is
    injectable so this module never imports the registry (which pulls the
    whole agent stack) — the app passes the real one, tests pass a stub.

    Order matters: the folder and its capability files are written **before**
    the first event, so a project can never exist in the log without the
    files its members need.
    """
    from . import events as ev
    from . import materialize as mat
    from .store import ProjectStore

    manifest.validate()
    paths = paths_for(root, manifest.id)
    scaffold(paths, manifest)

    environment = manifest.environment

    # 1. members -> agents/*.json, with provenance (DM-09)
    agents: list[dict] = []
    for member in manifest.members:
        name = str(member["name"])
        derived_from = str(member["derived_from"])
        spec = spec_lookup(derived_from) if spec_lookup else None
        if spec is not None:
            agent = mat.agent_from_spec(
                spec, name,
                model=str(member.get("model") or ""),
                thinking=str(member.get("thinking") or ""),
                tools=member.get("tools"),
                skills=list(member.get("skills") or []),
                mcp=list(member.get("mcp") or []),
                tool_caps=member.get("tool_caps"),
            )
        elif profile_lookup is not None:
            profile = profile_lookup(derived_from)
            if profile is None:
                raise mat.MaterializationError(
                    f"member {name!r} derives from unknown agent {derived_from!r}"
                )
            agent = mat.agent_from_profile(profile, name)
        else:
            raise mat.MaterializationError(
                f"member {name!r} derives from unknown agent {derived_from!r}"
            )
        mat.write_agent(agent, paths.agents_dir)
        agents.append(agent)

    # 2. skills -> skills/<name>/SKILL.md (copied, not referenced)
    skills: list[dict] = []
    for skill_name in environment.get("skills") or []:
        found = (skill_lookup or (lambda _n: None))(skill_name)
        if found is None:
            raise mat.MaterializationError(f"skill {skill_name!r} was not found")
        mat.copy_skill(found, paths.skills_dir)
        skills.append(found)
    if skills:
        mat.write_skills_index(skills, paths.skills_dir)

    # 3. tools -> tools/bindings.json
    mat.write_bindings(agents, paths.tools_dir)

    # 4. mcp -> mcp/servers.json (credentials by NAME only, DM-10)
    wanted_mcp = list(environment.get("mcp") or [])
    if wanted_mcp:
        mat.write_mcp_servers(
            mat.select_mcp_servers(mcp_catalogue or {}, wanted_mcp),
            paths.mcp_dir,
        )

    # 5. deps/ — this project's own dependencies
    deps = dict(environment.get("deps") or {})
    if deps:
        deps.setdefault("project_id", manifest.id)
        mat.write_deps(deps, paths.deps_dir)

    # 6. the event log: the project begins here
    store = ProjectStore(paths.state_db)
    store.append(manifest.id, ev.PROJECT_CREATED, "orchestrator",
                 data=manifest.to_plan())
    store.append(manifest.id, ev.PROJECT_RESUMED, "supervisor")

    # 7. lock + plan, then prove the folder can leave this machine
    write_lock(paths, harness_version=mat.HARNESS_VERSION,
               digests={"manifest": manifest.id})
    write_plan(paths, store, manifest.id)
    assert_portable(paths)
    return paths, store
