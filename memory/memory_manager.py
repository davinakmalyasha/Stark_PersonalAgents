"""Stable project identity and generated repository-map cache.

This module deliberately does not create or update conversation-derived
project artifacts or session journals. ``project.md`` is a small identity card
and ``repository_map.md`` is generated navigation data.
"""
import os
import json
import re
import time
from datetime import datetime

MEMORY_DIR = ".agent-memory"


# â”€â”€ Path helpers â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

def _ensure_dir(path: str) -> None:
    os.makedirs(path, exist_ok=True)


def _memory_path(workspace_path: str) -> str:
    return os.path.join(workspace_path, MEMORY_DIR)


def _project_file(workspace_path: str) -> str:
    return os.path.join(_memory_path(workspace_path), "project.md")


def _repository_map_file(workspace_path: str) -> str:
    """Return the dedicated generated repository-map path.

     Repository navigation is derived cache data, separate from the stable
     identity card, so refreshes never rewrite identity data.
    """
    return os.path.join(_memory_path(workspace_path), "repository_map.md")


def _repository_map_state_file(workspace_path: str) -> str:
    """Return the private topology state for the append-only map stream."""
    return os.path.join(_memory_path(workspace_path), "repository_map_state.json")


PROJECT_MAP_DELTA_MAX_CHARS = int(os.environ.get("PROJECT_MAP_DELTA_MAX_CHARS", "1800"))
PROJECT_MAP_FULL_MAX_CHARS = int(os.environ.get("PROJECT_MAP_FULL_MAX_CHARS", "10000"))


def _valid_map_header() -> str:
    """Format-versioned header; pre-v2 caches lack symbols + the richer
    delta grammar, so they must regenerate instead of being served."""
    try:
        from memory.repo_map_generator import MAP_HEADER
        return MAP_HEADER
    except Exception:
        return "projectMap:"


def _is_valid_map(content: str) -> bool:
    if not content or not content.startswith(_valid_map_header()):
        return False
    return (
        content.count("projectMap:") == 1
        and "projectMap+:" not in content
        and "Updates:" not in content
    )


def _cap_full_map(content: str) -> str:
    """Bound the full-map block injected on a FRESH task/transcript.

    The delta stream is already capped (PROJECT_MAP_DELTA_MAX_CHARS); the full
    snapshot was not, so a large workspace dumped its entire symbol map into
    the first request of every fresh run (~the fixed cache-miss entry fee).
    Symbols stay reachable via read_file/grep — lazy load beats pre-stuffing.
    """
    if content and len(content) > PROJECT_MAP_FULL_MAX_CHARS:
        cut = content[:PROJECT_MAP_FULL_MAX_CHARS].rsplit("\n", 1)[0]
        return cut + "\n# [projectMap truncated; use read_file/grep for missing symbols]\n"
    return content


def _load_repository_map_state(workspace_path: str) -> dict:
    raw = _read_file_safe(_repository_map_state_file(workspace_path))
    if not raw:
        return {}
    try:
        value = json.loads(raw)
        return value if isinstance(value, dict) else {}
    except Exception:
        return {}


def _save_repository_map_state(workspace_path: str, state: dict) -> bool:
    return _write_file_safe(
        _repository_map_state_file(workspace_path),
        json.dumps(state, ensure_ascii=False, separators=(",", ":")) + "\n",
    )


def _topology_changes(previous: dict, current: dict) -> dict[str, list[str]]:
    """Compare file presence and outgoing relationships, not file bodies."""
    old_paths = set(previous)
    new_paths = set(current)

    def targets(value: object) -> set[str]:
        return set(value) if isinstance(value, (list, tuple, set)) else set()

    return {
        "added": sorted(new_paths - old_paths),
        "removed": sorted(old_paths - new_paths),
        "modified": sorted(
            path for path in old_paths & new_paths
            if targets(previous.get(path)) != targets(current.get(path))
        ),
    }


def _snapshot_changes(previous: dict, current: dict) -> dict:
    """Diff two full snapshots (topology + top-level symbols).

    ``previous``/``current`` are state-shaped dicts (``topology``,
    ``symbols``). Body-only edits never appear: symbols are compared as
    name sets, so a docstring/format change does not emit a delta. Legacy
    state without a ``symbols`` map suppresses symbol diffs for one cycle
    instead of flooding the delta with every existing symbol.
    """
    old_topology = previous.get("topology") if isinstance(previous.get("topology"), dict) else {}
    new_topology = current.get("topology") if isinstance(current.get("topology"), dict) else {}
    old_symbols = previous.get("symbols") if isinstance(previous.get("symbols"), dict) else {}

    def targets(value: object) -> set[str]:
        return set(value) if isinstance(value, (list, tuple, set)) else set()

    old_paths = set(old_topology)
    new_paths = set(new_topology)
    changes: dict = {
        "added": sorted(new_paths - old_paths),
        "removed": sorted(old_paths - new_paths),
        "modified": sorted(
            path for path in old_paths & new_paths
            if targets(old_topology.get(path)) != targets(new_topology.get(path))
        ),
        "symbol_added": {},
        "symbol_removed": {},
    }
    if not old_symbols:
        return changes
    new_symbols = current.get("symbols") if isinstance(current.get("symbols"), dict) else {}
    for path in old_paths & new_paths:
        old_names = targets(old_symbols.get(path))
        new_names = targets(new_symbols.get(path))
        added = sorted(new_names - old_names)
        removed = sorted(old_names - new_names)
        if added:
            changes["symbol_added"][path] = added
        if removed:
            changes["symbol_removed"][path] = removed
    return changes


def _changes_count(changes: dict) -> int:
    total = len(changes.get("added") or []) + len(changes.get("removed") or [])
    total += len(changes.get("modified") or [])
    for key in ("symbol_added", "symbol_removed"):
        value = changes.get(key)
        if isinstance(value, dict):
            total += sum(len(v or []) for v in value.values())
    return total


def _save_map_state(workspace_path: str, state: dict, current: dict,
                    full_at: float, dirty: bool = False) -> bool:
    """Persist the snapshot triple plus the watcher flag (the dirty fast
    path can only be trusted while the watcher is actually running)."""
    return _save_repository_map_state(workspace_path, {
        "topology": current.get("topology") or {},
        "symbols": current.get("symbols") or {},
        "hashes": current.get("hashes") or {},
        "full_at": full_at,
        "last_refresh": time.time(),
        "dirty": dirty,
        "watcher_active": bool((state or {}).get("watcher_active")),
    })


def refresh_code_map_context(
    workspace_path: str,
    force_full: bool = False,
    include_cached_full: bool = False,
    include_summaries: bool = False,
) -> dict:
    """Refresh the map and return the smallest context event for the caller.

    The on-disk cache contains only the last canonical full ``projectMap``.
    Continuing transcripts receive immutable ``projectMap+`` events for small
    topology changes; those events are not appended to the canonical file.
    The transcript is the source of truth for deltas until the next single
    checkpoint refreshes the full map.

    This function returns one of three events:

    ``full``  - the current full snapshot (initial/invalid/forced refresh)
    ``delta`` - only the new compact ``projectMap+`` relationship lines
    ``none``  - no new map context is needed

    ``include_cached_full`` is used when there is no existing transcript
    anchor, such as a fresh Dev run.  If the filesystem topology changed
    since the cached snapshot, a fresh run gets a new full snapshot rather
    than an unanchored delta.

    ``include_summaries`` is used ONLY by the compaction re-anchor: the
    canonical cache is still written from the deterministic map, but the
    returned full snapshot overlays hash-matched LLM one-liners from the
    summary sidecar. Fresh runs never see it.
    """
    if not workspace_path or not os.path.isdir(workspace_path):
        return {"kind": "none", "content": "", "changed": False}
    _ensure_dir(_memory_path(workspace_path))

    # Phase 7: watcher dirty-flag fast path — only trusted while the file
    # watcher is actually running (tools/watcher.py sets watcher_active when
    # it marks the state dirty). Without it, an external edit would go
    # unnoticed, so the snapshot below always runs.
    map_file = _repository_map_file(workspace_path)
    existing = _read_file_safe(map_file) or ""
    state = _load_repository_map_state(workspace_path)
    valid_map_marker = _is_valid_map(existing)
    if (
        not force_full
        and valid_map_marker
        and not state.get("dirty")
        and isinstance(state.get("topology"), dict)
        and state.get("watcher_active")
    ):
        if include_cached_full:
            return {"kind": "full", "content": _cap_full_map(existing.strip()), "changed": False}
        return {"kind": "none", "content": "", "changed": False}

    try:
        from memory.repo_map_generator import RepoMapGenerator
        generator = RepoMapGenerator(workspace_path)
        current = generator.snapshot(previous=state)
    except Exception as e:
        print(f"[MEMORY] Project map snapshot failed: {e}")
        return {"kind": "none", "content": "", "changed": False}

    has_topology_state = isinstance(state.get("topology"), dict)
    changes = _snapshot_changes(state, current)
    changed_count = _changes_count(changes)
    now = time.time()
    try:
        full_at = float(state.get("full_at", 0) or 0)
    except (TypeError, ValueError):
        full_at = 0
    # A full rebuild is allowed only for an explicit checkpoint/initial
    # refresh, an invalid/missing cache, or a fresh transcript whose cached
    # topology no longer matches the filesystem. There is intentionally no
    # elapsed-time refresh: an hourly rewrite would create an avoidable cache
    # boundary while the conversation is otherwise unchanged.
    full_due = (
        force_full
        or not valid_map_marker
        or not has_topology_state
        or (include_cached_full and bool(changed_count))
    )

    if not changed_count and not full_due:
        if include_cached_full and valid_map_marker:
            return {
                "kind": "full",
                "content": _cap_full_map(existing.strip()),
                "changed": False,
            }
        return {"kind": "none", "content": "", "changed": False}

    # Continuing sessions stay append-only even for larger topology changes.
    # The delta is bounded so an unusually large change cannot become a
    # second hidden full-map refresh. The next checkpoint will regenerate the
    # canonical snapshot from the current filesystem.
    if not full_due:
        try:
            delta = generator.generate_delta(changes, current=current, previous=state).strip()
        except Exception as e:
            print(f"[MEMORY] Project map delta failed: {e}")
            return {"kind": "none", "content": "", "changed": False}
        if not delta:
            return {"kind": "none", "content": "", "changed": False}
        if len(delta) > PROJECT_MAP_DELTA_MAX_CHARS:
            delta = (
                delta[:PROJECT_MAP_DELTA_MAX_CHARS].rsplit("\n", 1)[0]
                + "\n[delta clipped; full snapshot at next checkpoint]"
            )
        _save_map_state(workspace_path, state, current, full_at)
        print(f"[MEMORY] projectMap+ event emitted ({len(delta)} chars)")
        return {
            "kind": "delta",
            "content": "projectMap+:\n" + delta,
            "changed": True,
        }

    try:
        full_map = generator.generate_map()
    except Exception as e:
        print(f"[MEMORY] Project map full rebuild failed: {e}")
        return {"kind": "none", "content": "", "changed": False}
    if not full_map or not full_map.startswith("projectMap:"):
        return {"kind": "none", "content": "", "changed": False}
    updated = full_map.strip() + "\n"
    wrote = _write_file_safe(map_file, updated)
    injection_text = full_map.strip()
    if wrote and include_summaries:
        try:
            enriched = generator.generate_map(include_summaries=True)
            if enriched and enriched.startswith("projectMap:"):
                injection_text = enriched.strip()
        except Exception:
            pass
    if wrote:
        _save_map_state(workspace_path, state, current, now)
        print(f"[MEMORY] projectMap fully rebuilt ({len(updated)} chars)")
    return {
        "kind": "full" if wrote else "none",
        "content": _cap_full_map(injection_text) if wrote else "",
        "changed": bool(wrote),
    }


def force_delta_refresh(workspace_path: str) -> str:
    """Emit a projectMap+ delta after a mutating tool call (live map).

    Runs between model requests inside a single AgentRun so the agent sees
    topology changes without waiting for an outer iteration/resume. Reads
    the persisted topology state, rescans ONCE, persists the new state and
    returns the capped delta block text ('' when nothing changed). The
    caller wraps it as <current-context>; re-sent blocks are provider
    cache-hits after their first appearance.
    """
    if not workspace_path or not os.path.isdir(workspace_path):
        return ""
    try:
        from memory.repo_map_generator import RepoMapGenerator
        st = _load_repository_map_state(workspace_path) or {}
        gen = RepoMapGenerator(workspace_path)
        cur = gen.snapshot(previous=st)
        changes = _snapshot_changes(st, cur)
        if not _changes_count(changes):
            return ""
        delta = gen.generate_delta(changes, current=cur, previous=st).strip()
        if not delta:
            return ""
        if len(delta) > PROJECT_MAP_DELTA_MAX_CHARS:
            delta = (delta[:PROJECT_MAP_DELTA_MAX_CHARS].rsplit("\n", 1)[0]
                     + "\n[delta clipped; full snapshot at next checkpoint]")
        try:
            full_at = float(st.get("full_at", 0) or 0)
        except (TypeError, ValueError):
            full_at = 0
        _save_map_state(workspace_path, st, cur, full_at)
        return "projectMap+:\n" + delta
    except Exception:
        return ""


def fresh_full_map(workspace_path: str, include_summaries: bool = False) -> str:
    """Force a current full projectMap snapshot (compaction re-injection).

    Compaction must re-anchor the LATEST map, not the stale copy embedded in
    old history (edits since then only existed as deltas). One full rebuild;
    returns bare 'projectMap:...' text or '' on failure. ``include_summaries``
    overlays hash-matched LLM one-liners on the returned snapshot only — the
    canonical cache file stays deterministic for fresh runs.
    """
    if not workspace_path or not os.path.isdir(workspace_path):
        return ""
    try:
        ev = refresh_code_map_context(workspace_path, force_full=True,
                                      include_cached_full=False,
                                      include_summaries=include_summaries)
        content = str(ev.get("content") or "")
        return content if content.startswith("projectMap:") else ""
    except Exception:
        return ""


def refresh_code_map(workspace_path: str, force_full: bool = False) -> bool:
    """Maintain the canonical ``projectMap`` cache.

    This compatibility wrapper preserves the historical boolean API.  Dev
    uses :func:`refresh_code_map_context` when it needs the cache-efficient
    full-versus-delta injection event.
    """
    # The compatibility/cache-initialization path has no transcript anchor,
    # so topology changes must become a full snapshot here. Continuing Dev
    # transcripts use refresh_code_map_context directly and receive deltas.
    result = refresh_code_map_context(
        workspace_path,
        force_full=force_full,
        include_cached_full=True,
    )
    return bool(result.get("changed"))


# â”€â”€ Project identity card â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
#
# project.md is the LLM-AUTHORED project doc (CLAUDE.md/AGENTS.md style),
# not derived state. It holds two compact bullet sections:
#
#   ## Identity - project, purpose, stack, goals. Authored by the Developer on
#     the first task of a project that has none; updated only on a real pivot.
#   ## Design   - direction, theme, palette, typography, concept. Owned by the
#     UI specialist (design lock / redesign).
#
# The harness only reads/parses/projects this file; the compaction LLM may
# refresh it during a checkpoint and the harness writes exactly what the LLM
# returned (validated). No status gate, no drift appends, no auto-detection.

_MEMORY_SECTION_NAMES = ("Identity", "Design")


def _parse_memory_sections(content: str) -> dict:
    """Parse `## Identity` / `## Design` bullet sections (tolerant).

    Only `- key: value` lines inside the two known sections are kept; a later
    occurrence of a section replaces an earlier one; everything else is
    ignored. Returns ``{section_name: ["key: value", ...]}``.
    """
    out: dict = {}
    current = ""
    for raw in (content or "").splitlines():
        s = raw.strip()
        if s.startswith("##"):
            title = s.lstrip("#").strip()
            current = title if title in _MEMORY_SECTION_NAMES else ""
            if current:
                out[current] = []
            continue
        if current and s.startswith("-"):
            body = s.lstrip("-").strip()
            if ":" in body:
                out[current].append(body)
    return out


def load_project_memory(workspace_path: str) -> str:
    """Compact projection of the authored project doc, or "" when absent.

    Read-only. Injected as its own context message below the system prompt and
    re-anchored fresh at every compaction checkpoint.
    """
    if not workspace_path or not os.path.isdir(workspace_path):
        return ""
    content = _read_file_safe(_project_file(workspace_path))
    if not content or not content.strip():
        return ""
    sections = _parse_memory_sections(content)
    parts: list = []
    for name in _MEMORY_SECTION_NAMES:
        bullets = sections.get(name) or []
        if bullets:
            parts.append(f"## {name}\n" + "\n".join(f"- {b}" for b in bullets))
    return "\n\n".join(parts)


def read_project_doc(workspace_path: str) -> str:
    """Raw project.md content (for the compaction prompt). Read-only."""
    if not workspace_path or not os.path.isdir(workspace_path):
        return ""
    return str(_read_file_safe(_project_file(workspace_path)) or "").strip()


def update_project_memory(workspace_path: str, updated: str) -> bool:
    """Persist LLM-authored section updates (compaction sync).

    Accepts a block containing `## Identity` and/or `## Design` bullet lines;
    sections absent from ``updated`` are preserved from the existing file.
    Invalid input (no known section) returns False and leaves the file
    untouched. Atomic write (tmp + replace).
    """
    if not workspace_path or not os.path.isdir(workspace_path):
        return False
    new_sections = _parse_memory_sections(updated)
    if not new_sections:
        return False
    existing = _parse_memory_sections(_read_file_safe(_project_file(workspace_path)))
    merged: list = ["# Project", ""]
    for name in _MEMORY_SECTION_NAMES:
        bullets = new_sections.get(name) or existing.get(name) or []
        if not bullets:
            continue
        merged.append(f"## {name}")
        merged.extend(f"- {b}" for b in bullets)
        merged.append("")
    text = "\n".join(merged).rstrip() + "\n"
    try:
        _ensure_dir(_memory_path(workspace_path))
        target = _project_file(workspace_path)
        tmp = target + ".tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            f.write(text)
        os.replace(tmp, target)
        print(f"[MEMORY] project.md updated ({len(text)} chars)")
        return True
    except Exception as e:
        print(f"[MEMORY] project.md update failed: {e}")
        return False


def _detect_stack_summary(workspace_path: str) -> str:
    """One-line primary stack summary, detected once at creation."""
    stack = _detect_tech_stack(workspace_path) if os.path.isdir(workspace_path) else []
    seen, out = set(), []
    for s in stack:
        if s not in seen:
            seen.add(s)
            out.append(s)
    return ", ".join(out[:8]) or "unknown"


def ensure_project_cache(workspace_path: str) -> bool:
    """Ensure only the repository map cache exists.

    Authored project memory (project.md) is created by the agents themselves,
    never by the harness; this initializer deliberately touches only the
    generated map.
    """
    _ensure_dir(_memory_path(workspace_path))
    before_map = os.path.isfile(_repository_map_file(workspace_path))
    refresh_code_map(workspace_path)
    return not before_map


# â”€â”€ Tech detection â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

def _detect_dependencies(workspace_path: str) -> list[str]:
    """Deterministically extract dependencies, npm scripts and entry points from
    package.json / requirements.txt (top-N only, keeps the memory block small)."""
    out: list[str] = []
    try:
        pkg_path = os.path.join(workspace_path, "package.json")
        if os.path.isfile(pkg_path):
            import json as _json
            with open(pkg_path, "r", encoding="utf-8") as _f:
                data = _json.load(_f)
            deps = data.get("dependencies") or {}
            dev = data.get("devDependencies") or {}
            for name in list(deps)[:10]:
                out.append(f"{name}@{deps[name]}")
            for name in list(dev)[:5]:
                out.append(f"dev: {name}@{dev[name]}")
            scripts = data.get("scripts") or {}
            for name, cmd in list(scripts.items())[:6]:
                out.append(f"script: {name} -> {str(cmd)[:60]}")
            if data.get("main"):
                out.append(f"entry: {data['main']}")
    except Exception:
        pass
    try:
        req_path = os.path.join(workspace_path, "requirements.txt")
        if os.path.isfile(req_path):
            with open(req_path, "r", encoding="utf-8", errors="replace") as _f:
                for line in _f:
                    line = line.strip()
                    if line and not line.startswith("#"):
                        out.append(line)
                    if len(out) > 20:
                        break
    except Exception:
        pass
    return out[:20]

def _read_file_lines(path: str) -> list[str]:
    try:
        with open(path, "r", encoding="utf-8") as f:
            return f.readlines()
    except Exception:
        return []


def _detect_tech_stack(workspace_path: str) -> list[str]:
    stack = []

    # Python
    req_file = os.path.join(workspace_path, "requirements.txt")
    if os.path.isfile(req_file):
        stack.append("Python")
        content = _read_file_lines(req_file)
        for line in content:
            ll = line.lower()
            if "fastapi" in ll and "FastAPI" not in stack:
                stack.append("FastAPI")
            if "flask" in ll and "Flask" not in stack:
                stack.append("Flask")
            if "django" in ll and "Django" not in stack:
                stack.append("Django")
            if "langchain" in ll and "LangChain" not in stack:
                stack.append("LangChain")
            if "langgraph" in ll and "LangGraph" not in stack:
                stack.append("LangGraph")
            if "pydantic" in ll and "Pydantic" not in stack:
                stack.append("Pydantic")
            if "sqlalchemy" in ll and "SQLAlchemy" not in stack:
                stack.append("SQLAlchemy")

    # Node.js
    pkg_file = os.path.join(workspace_path, "package.json")
    if os.path.isfile(pkg_file):
        stack.append("Node.js")
        try:
            with open(pkg_file, "r", encoding="utf-8") as f:
                pkg = json.load(f)
            deps = {**pkg.get("dependencies", {}), **pkg.get("devDependencies", {})}
            for dep in deps:
                dl = dep.lower()
                if "typescript" in dl and "TypeScript" not in stack:
                    stack.append("TypeScript")
                if "tailwind" in dl and "Tailwind CSS" not in stack:
                    stack.append("Tailwind CSS")
        except Exception:
            pass

    # PHP
    if os.path.isfile(os.path.join(workspace_path, "composer.json")):
        stack.append("PHP")

    # Go
    if os.path.isfile(os.path.join(workspace_path, "go.mod")):
        stack.append("Go")

    # Rust
    if os.path.isfile(os.path.join(workspace_path, "Cargo.toml")):
        stack.append("Rust")

    # Ruby
    if os.path.isfile(os.path.join(workspace_path, "Gemfile")):
        stack.append("Ruby")

    return stack


def _detect_database(workspace_path: str) -> str | None:
    patterns = [
        (r"sqlite|sqlite3", "SQLite"),
        (r"postgresql|postgres|psycopg", "PostgreSQL"),
        (r"mysql|mariadb|pymysql", "MySQL / MariaDB"),
        (r"mongodb|pymongo|mongoose", "MongoDB"),
        (r"redis", "Redis"),
        (r"supabase|firebase", "Supabase / Firebase"),
        (r"prisma", "Prisma ORM"),
    ]
    # Only check top-level files, not recursive (avoids node_modules, .git, venv etc.)
    try:
        for entry in os.listdir(workspace_path):
            fpath = os.path.join(workspace_path, entry)
            if os.path.isfile(fpath) and entry in (".env", ".env.example", "config.py", "database.php"):
                content = open(fpath, encoding="utf-8").read().lower()
                for pattern, db_name in patterns:
                    if re.search(pattern, content):
                        return db_name
    except Exception:
        pass
    return None


def _detect_frontend(workspace_path: str) -> list[str] | None:
    frontend = []
    pkg_file = os.path.join(workspace_path, "package.json")
    if os.path.isfile(pkg_file):
        try:
            with open(pkg_file, "r", encoding="utf-8") as f:
                pkg = json.load(f)
            deps = {**pkg.get("dependencies", {}), **pkg.get("devDependencies", {})}
            for dep in deps:
                dl = dep.lower()
                if "react" in dl and "React" not in frontend:
                    frontend.append("React")
                    if "next" in dl:
                        frontend.append("Next.js")
                if "vue" in dl and "Vue.js" not in frontend:
                    frontend.append("Vue.js")
                if "angular" in dl and "Angular" not in frontend:
                    frontend.append("Angular")
                if "svelte" in dl and "Svelte" not in frontend:
                    frontend.append("Svelte")
                if "vite" in dl and "Vite" not in frontend:
                    frontend.append("Vite")
                if "tailwind" in dl and "Tailwind CSS" not in frontend:
                    frontend.append("Tailwind CSS")
        except Exception:
            pass

    if os.path.isdir(os.path.join(workspace_path, "frontend")) and not frontend:
        frontend.append("Frontend SPA")

    return frontend if frontend else None


def _detect_test_framework(workspace_path: str) -> str | None:
    if os.path.isfile(os.path.join(workspace_path, "pytest.ini")):
        return "pytest"
    content = _read_file_lines(os.path.join(workspace_path, "pyproject.toml"))
    if any("pytest" in line for line in content):
        return "pytest"
    pkg_file = os.path.join(workspace_path, "package.json")
    if os.path.isfile(pkg_file):
        try:
            with open(pkg_file, "r", encoding="utf-8") as f:
                pkg = json.load(f)
            for dep in {**pkg.get("dependencies", {}), **pkg.get("devDependencies", {})}:
                dl = dep.lower()
                if "jest" in dl:
                    return "Jest"
                if "vitest" in dl:
                    return "Vitest"
                if "playwright" in dl:
                    return "Playwright"
        except Exception:
            pass
    return None


def _detect_architecture(workspace_path: str) -> list[str]:
    arch = []
    dirs = set()
    try:
        for entry in os.listdir(workspace_path):
            if os.path.isdir(os.path.join(workspace_path, entry)):
                dirs.add(entry.lower())
    except Exception:
        pass

    if "src" in dirs:
        arch.append("Source in `src/` directory")
    if "frontend" in dirs or "client" in dirs:
        arch.append("Separate frontend")
    if "backend" in dirs or "api" in dirs:
        arch.append("Separate backend")
    if "routes" in dirs:
        arch.append("Route-based API")
    if "services" in dirs:
        arch.append("Service layer")
    if "agents" in dirs or ".deep_agents" in dirs:
        arch.append("Multi-agent system")
    if "tests" in dirs or "test" in dirs:
        arch.append("Dedicated test directory")

    return arch


# â”€â”€ Reading â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

def load_cached_code_map(workspace_path: str) -> str:
    """Return the dedicated repository map, or an empty string.

    Read-only â€” never generates side effects. Invalid or legacy map files are
    ignored so they cannot enter a prompt.
    """
    if not workspace_path or not os.path.isdir(workspace_path):
        return ""
    content = (_read_file_safe(_repository_map_file(workspace_path)) or "").strip()
    if not _is_valid_map(content):
        return ""
    return _cap_full_map(content)


# â”€â”€ File helpers â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

def _read_file_safe(path: str) -> str | None:
    try:
        with open(path, "r", encoding="utf-8") as f:
            return f.read()
    except Exception:
        return None


def _write_file_safe(path: str, content: str) -> bool:
    try:
        with open(path, "w", encoding="utf-8") as f:
            f.write(content)
        return True
    except Exception as e:
        print(f"[MEMORY] Error writing {path}: {e}")
        return False


def is_valid_build_request(text: str) -> bool:
    """Determine whether user input is an actionable build/task request."""
    if not text or not isinstance(text, str):
        return False
    stripped = text.strip()
    if len(stripped) < 3:
        return False
    greetings = {"hi", "hello", "hey", "hola", "halo", "test", "ping", "p"}
    if stripped.lower() in greetings:
        return False
    return True

