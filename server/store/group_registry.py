"""Group project registry — find projects on disk and open them (Phase 4).

```docs/group-projects/05-integration-map.md`` §11 and ``06`` §2.

**The folder is the registry.** A project is a directory with a
``PROJECT.md`` in it, so this module scans ``STARK_PROJECTS_ROOT`` rather than
maintaining an index file. There is no second source of truth to drift, and a
project copied in from another machine is discovered on the next scan.

It also builds the two things the supervisor cannot build for itself:

- a :class:`~agents.group.supervisor.ProjectContext`, assembled from the
  manifest, the *materialised* agents (the folder owns them, not the registry),
  and the machine-level catalogues,
- the injection points those need — ``spec_lookup``, ``skill_lookup``,
  ``mcp_catalogue``, and an optional ``judge`` for ``llm`` criteria.

Stores are cached per project: opening a SQLite connection on every call would
be wasteful, and the supervisor ticks every 15 seconds.
"""
from __future__ import annotations

import json
import os
import threading
from typing import Any

from agents.group import manifest as manifest_mod
from agents.group import materialize as materialize_mod
from agents.group.project import ProjectPaths, paths_for
from agents.group.store import ProjectStore
from agents.group.supervisor import ProjectContext


#: Where project folders live. A cloud host points this at a persistent
#: volume; nothing else changes (``05`` §4.3).
def projects_root() -> str:
    return os.environ.get("STARK_PROJECTS_ROOT") or r"D:\StarkProjects"


def _harness_root() -> str:
    return os.environ.get("HARNESS_ROOT") or os.path.dirname(
        os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


_lock = threading.RLock()
_stores: dict[str, ProjectStore] = {}
_root_override: str | None = None


def set_projects_root(path: str | None) -> None:
    """Point the registry somewhere else (tests, or a second root)."""
    global _root_override
    with _lock:
        _root_override = path
        invalidate()


def root() -> str:
    with _lock:
        return _root_override or projects_root()


# ── origin workspace ────────────────────────────────────────────────────────
#
# Which workspace asked for a project. Recorded **here, not in the folder**,
# and that is the whole point:
#
# - The portability contract (``01`` §14, enforced by ``check_portability``)
#   requires a folder to load and resume at any root, and ``06`` §7 names
#   "an absolute path sneaks in" as a failure mode.
# - A workspace *id* is better than a path, but it is still a harness
#   reference that cannot travel with the folder.
#
# So the folder stays pure. Copy it to another machine and it still resumes —
# it merely loses its workspace badge. That is the correct trade, and the
# alternative (a fat folder that moves) is the one that actually hurts.

_ORIGINS_FILENAME = "group_origins.json"


def _origins_path() -> str:
    return os.path.join(_harness_root(), ".deep_agents", _ORIGINS_FILENAME)


def _load_origins() -> dict:
    try:
        with open(_origins_path(), encoding="utf-8") as handle:
            data = json.load(handle)
    except (OSError, ValueError):
        return {}
    return data if isinstance(data, dict) else {}


def origin_workspace(project_id: str) -> str | None:
    """The workspace id this project was created from, or ``None`` for global."""
    if not project_id:
        return None
    with _lock:
        value = _load_origins().get(project_id)
    return str(value) if value else None


def set_origin_workspace(project_id: str, workspace_id: str | None) -> None:
    """Remember a project's origin. Atomic — a crash mid-write cannot corrupt."""
    if not project_id or not workspace_id:
        return
    with _lock:
        origins = _load_origins()
        if origins.get(project_id) == workspace_id:
            return
        origins[project_id] = workspace_id
        path = _origins_path()
        os.makedirs(os.path.dirname(path), exist_ok=True)
        tmp = f"{path}.tmp"
        with open(tmp, "w", encoding="utf-8", newline="\n") as handle:
            json.dump(origins, handle, indent=2, sort_keys=True)
        os.replace(tmp, path)


def workspace_id_for_path(workspace_path: str) -> str | None:
    """Resolve a workspace root to its id.

    Imported lazily in both directions: this module must stay importable
    without a workspace registry on disk, and the workspace registry must not
    have to import us.
    """
    if not workspace_path:
        return None
    try:
        from server.store.workspace_registry import _resolve_workspace_id
        return _resolve_workspace_id(workspace_path)
    except Exception:  # noqa: BLE001 - an unresolvable path is simply "no origin"
        return None


def workspace_kind(workspace_id: str) -> str:
    """``workspaceKind`` for an id ("project", "general", ...) or ``""``."""
    if not workspace_id:
        return ""
    try:
        from server.store.workspace_registry import get_workspace
        return str((get_workspace(workspace_id) or {}).get("workspaceKind") or "")
    except Exception:  # noqa: BLE001 - an unknown kind is treated as unscoped
        return ""


# ── discovery ───────────────────────────────────────────────────────────────


def project_paths(project_id: str) -> ProjectPaths:
    return paths_for(root(), project_id)


def list_project_ids() -> list[str]:
    """Every directory under the root that looks like a project."""
    base = root()
    if not os.path.isdir(base):
        return []
    found = []
    for name in sorted(os.listdir(base)):
        candidate = os.path.join(base, name)
        if os.path.isfile(os.path.join(candidate, "PROJECT.md")):
            found.append(name)
    return found


def list_projects() -> list[dict]:
    """Summary rows for the API. Never raises for one bad project."""
    rows = []
    for project_id in list_project_ids():
        try:
            paths = project_paths(project_id)
            record = {"id": project_id, "path": paths.project_dir,
                      "title": "", "status": "unknown",
                      "origin_workspace_id": origin_workspace(project_id),
                      "shell_enabled": False}
            try:
                manifest = manifest_mod.load(paths.manifest)
                record["title"] = manifest.title
                # A member holding a shell can reach anywhere, so every other
                # tier becomes advisory once one is opted in (``06`` §3). That
                # is a legitimate choice, but it should be visible where the
                # project is looked at, not only inside PROJECT.md.
                record["shell_enabled"] = (
                    "run_command"
                    not in materialize_mod.never_tools(manifest.autonomy)
                )
            except Exception:  # noqa: BLE001, S110 - broken manifest is data
                pass
            try:
                project = open_store(project_id).project(project_id)
                if project:
                    record["status"] = project.get("status") or "unknown"
                    record["spent_usd"] = project.get("spent_usd")
            except Exception:  # noqa: BLE001, S110 - an unreadable store is data
                pass
            rows.append(record)
        except Exception:  # noqa: BLE001, S112 - skip the bad row, keep the list
            continue
    return rows


def exists(project_id: str) -> bool:
    return os.path.isfile(project_paths(project_id).manifest)


def visible_projects(rows: list[dict], *, workspace_id: str | None,
                     scoped: bool) -> list[dict]:
    """Which of ``rows`` a workspace should see. Pure, so it is testable.

    ``scoped`` is the caller's decision about the workspace *kind*; this
    function only applies it, so the rule has exactly one home.
    """
    if not scoped or not workspace_id:
        return list(rows)
    return [row for row in rows if row.get("origin_workspace_id") == workspace_id]


def list_projects_for(workspace_id: str | None) -> list[dict]:
    """The project rows one workspace should see.

    A **coding workspace** (``workspaceKind == "project"``) sees only projects
    created from its own folder. Everything else — General, House, Personal,
    DFD — is not a code folder and sees them all.

    A project with **no** recorded origin is global: it appears in every
    unscoped workspace and in no coding one. That is why projects predating
    this field do not silently vanish from the board — they show up in
    General, where you would look for them.
    """
    rows = list_projects()
    return visible_projects(
        rows, workspace_id=workspace_id,
        scoped=workspace_kind(workspace_id or "") == "project",
    )


# ── stores ──────────────────────────────────────────────────────────────────


def open_store(project_id: str) -> ProjectStore:
    with _lock:
        store = _stores.get(project_id)
        if store is not None:
            return store
    paths = project_paths(project_id)
    if not os.path.isfile(paths.state_db):
        raise FileNotFoundError(f"no state.db for project {project_id!r}")
    store = ProjectStore(paths.state_db)
    with _lock:
        existing = _stores.get(project_id)
        if existing is not None:
            store.close()
            return existing
        _stores[project_id] = store
    return store


def invalidate(project_id: str | None = None) -> None:
    """Drop cached stores. Used by ``set_projects_root`` and by tests."""
    with _lock:
        if project_id is None:
            for store in _stores.values():
                store.close()
            _stores.clear()
        else:
            store = _stores.pop(project_id, None)
            if store is not None:
                store.close()


# ── lookups the context needs ───────────────────────────────────────────────

_skills_cache: dict[str, dict] = {}


def spec_lookup(key: str) -> Any:
    """Registry spec by key. Imported lazily so a cold import stays cheap."""
    try:
        from agents.workspace_registry import get_spec
        return get_spec(key)
    except Exception:  # noqa: BLE001 - a missing registry is "no spec", not a crash
        return None


def profile_lookup(key: str) -> Any:
    """Specialist profile by name (``agents/specialists/<key>.json``)."""
    path = os.path.join(_harness_root(), "agents", "specialists", f"{key}.json")
    if not os.path.isfile(path):
        return None
    try:
        with open(path, encoding="utf-8") as handle:
            return json.load(handle)
    except (OSError, ValueError):
        return None


def skill_lookup(name: str) -> dict | None:
    """One skill by name, discovered once and cached."""
    with _lock:
        cache = _skills_cache.get("all")
        if cache is None:
            try:
                from tools.skills import discover_skills
                cache = discover_skills(workspace_path=root(),
                                        harness_root=_harness_root(),
                                        include_hidden=True)
            except Exception:  # noqa: BLE001
                cache = {}
            _skills_cache["all"] = cache
        return cache.get(name)


def mcp_catalogue() -> dict:
    """The machine-level record of what is *installed* (not what is attached)."""
    path = os.path.join(_harness_root(), ".deep_agents", "mcp_servers.json")
    if not os.path.isfile(path):
        return {}
    try:
        with open(path, encoding="utf-8") as handle:
            return json.load(handle)
    except (OSError, ValueError):
        return {}


# ── resolve ─────────────────────────────────────────────────────────────────


def members_for(paths: ProjectPaths) -> dict[str, dict]:
    """The project's OWN materialised agents — never the live registry."""
    return materialize_mod.read_agents(paths.agents_dir)


def context_for(project_id: str) -> ProjectContext | None:
    """Build the supervisor's view of one project. ``None`` when unknown."""
    if not exists(project_id):
        return None
    paths = project_paths(project_id)
    try:
        manifest = manifest_mod.load(paths.manifest)
    except Exception:  # noqa: BLE001 - an unreadable manifest means "not resolvable"
        return None
    return ProjectContext(
        project_id=project_id,
        root=paths.project_dir,
        members=members_for(paths),
        autonomy=manifest.autonomy,
        manifest_members=manifest.members,
        paths=paths,
        spec_lookup=spec_lookup,
        profile_lookup=profile_lookup,
        skill_lookup=skill_lookup,
        mcp_catalogue=mcp_catalogue(),
    )


def resolve(project_id: str) -> tuple[ProjectPaths, ProjectStore, ProjectContext] | None:
    """``(paths, store, context)`` — what :class:`GroupToolkit` needs."""
    context = context_for(project_id)
    if context is None:
        return None
    try:
        store = open_store(project_id)
    except FileNotFoundError:
        return None
    return project_paths(project_id), store, context


def resolver(project_id: str) -> tuple[ProjectStore, ProjectContext] | None:
    """Adapter for ``GroupToolkit(resolve=...)``."""
    found = resolve(project_id)
    if found is None:
        return None
    _paths, store, context = found
    return store, context


# ── create ──────────────────────────────────────────────────────────────────


def create(project_id_or_manifest: Any, manifest: Any = None, **options: Any) -> Any:
    """Adapter for ``GroupToolkit(create=...)``.

    ``GroupToolkit.create_group_project`` calls ``create(root, manifest,
    **options)``; this binds the registry's root and lookups so callers do not
    have to pass them on every creation.
    """
    from agents.group.project import create_group_project

    if manifest is None:
        raise TypeError("create() needs (root, manifest)")
    root_dir = str(project_id_or_manifest)
    options.setdefault("spec_lookup", spec_lookup)
    options.setdefault("profile_lookup", profile_lookup)
    options.setdefault("skill_lookup", skill_lookup)
    options.setdefault("mcp_catalogue", mcp_catalogue())
    paths, store = create_group_project(root_dir, manifest, **options)
    with _lock:
        _stores[manifest.id] = store
    return paths, store


def toolkit() -> Any:
    """A :class:`GroupToolkit` bound to this registry."""
    from agents.group.tools import GroupToolkit
    return GroupToolkit(resolver, create=create)
