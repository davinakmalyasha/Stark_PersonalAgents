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
                      "title": "", "status": "unknown"}
            try:
                record["title"] = manifest_mod.load(paths.manifest).title
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
