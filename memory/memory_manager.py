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


def refresh_code_map_context(
    workspace_path: str,
    force_full: bool = False,
    include_cached_full: bool = False,
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
    """
    if not workspace_path or not os.path.isdir(workspace_path):
        return {"kind": "none", "content": "", "changed": False}
    _ensure_dir(_memory_path(workspace_path))

    # Phase 7: watcher dirty-flag fast path â€” when the file watcher already
    # marked the repo-map state dirty on external change (tools/watcher.py),
    # skip the topology snapshot entirely for unchanged workspaces. The flag
    # is cleared on every successful refresh below.
    map_file = _repository_map_file(workspace_path)
    existing = _read_file_safe(map_file) or ""
    state = _load_repository_map_state(workspace_path)
    legacy_updates_marker = "Updates:" in existing or "projectMap+:" in existing
    valid_map_marker = (
        existing.startswith("projectMap:")
        and existing.count("projectMap:") == 1
        and not legacy_updates_marker
    )
    if (
        not force_full
        and valid_map_marker
        and not state.get("dirty")
        and isinstance(state.get("topology"), dict)
    ):
        if include_cached_full:
            return {"kind": "full", "content": existing.strip(), "changed": False}
        return {"kind": "none", "content": "", "changed": False}

    try:
        from memory.repo_map_generator import RepoMapGenerator
        generator = RepoMapGenerator(workspace_path)
        current_topology = generator.topology_snapshot()
    except Exception as e:
        print(f"[MEMORY] Project map snapshot failed: {e}")
        return {"kind": "none", "content": "", "changed": False}

    state_topology = state.get("topology")
    has_topology_state = isinstance(state_topology, dict)
    previous_topology = state_topology if has_topology_state else {}
    changes = _topology_changes(previous_topology, current_topology)
    changed_count = sum(len(v) for v in changes.values())
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
                "content": existing.strip(),
                "changed": False,
            }
        return {"kind": "none", "content": "", "changed": False}

    # Continuing sessions stay append-only even for larger topology changes.
    # The delta is bounded so an unusually large change cannot become a
    # second hidden full-map refresh. The next checkpoint will regenerate the
    # canonical snapshot from the current filesystem.
    if not full_due:
        try:
            delta = generator.generate_delta(changes).strip()
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
        _save_repository_map_state(workspace_path, {
            "topology": current_topology,
            "full_at": full_at,
            "last_refresh": now,
            "dirty": False,
        })
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
    if wrote:
        _save_repository_map_state(workspace_path, {
            "topology": current_topology,
            "full_at": now,
            "last_refresh": now,
            "dirty": False,
        })
        print(f"[MEMORY] projectMap fully rebuilt ({len(updated)} chars)")
    return {
        "kind": "full" if wrote else "none",
        "content": full_map.strip() if wrote else "",
        "changed": bool(wrote),
    }


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
# project.md is the write-once IDENTITY CARD: name / description / purpose /
# stack / version + a status. It adapts to reality ONLY by APPENDING corrected
# \stack:\/\ersion:\ lines at the bottom behind an [updated:] marker (same
# append-only mechanics as the code map) â€” old lines are never rewritten; the
# last occurrence wins.
#
# status: draft â†’ stable (harness-decided, one-way):
#   * draft   â€” the project is young; the identity may still change. The card
#     is NOT injected anywhere and NO drift appends happen (all early
#     stack/purpose churn stays invisible to the API â€” zero cache impact).
#   * stable  â€” the project is structurally committed (manifest + â‰¥5 source
#     files + detectable stack). The card is injected into the CACHED stable
#     zone of every code-touching agent; reality drift appends below.
# Agents never decide this â€” the harness checks the heuristic at each
# session; the eval stamps cards stable at creation (a task IS a built
# project).

_MANIFEST_NAMES = ("package.json", "requirements.txt", "pyproject.toml",
                   "go.mod", "composer.json", "Cargo.toml")
_CARD_SOURCE_EXTS = (".py", ".php", ".ts", ".tsx", ".js", ".jsx", ".dart")
_CARD_IGNORED_DIRS = {"node_modules", "vendor", ".git", "venv", ".venv",
                      "__pycache__", ".deep_agents", "dist", "build",
                      ".pytest_cache", ".vscode", ".idea", "scratch"}


def _project_is_developed(workspace_path: str) -> bool:
    """Deterministic structural-commitment check: a manifest exists AND at
    least 5 real source files exist AND the stack is detectable. This is the
    'can no longer be re-purposed' line â€” once crossed, never demoted."""
    try:
        if not any(os.path.isfile(os.path.join(workspace_path, m)) for m in _MANIFEST_NAMES):
            return False
        src_count = 0
        for root, dirs, files in os.walk(workspace_path):
            dirs[:] = [d for d in dirs if d not in _CARD_IGNORED_DIRS]
            for fn in files:
                if fn.lower().endswith(_CARD_SOURCE_EXTS):
                    src_count += 1
                    if src_count >= 5:
                        break
            if src_count >= 5:
                break
        if src_count < 5:
            return False
        return _detect_stack_summary(workspace_path) != "unknown"
    except Exception:
        return False


def _card_last_status(card: str) -> str:
    """Last `status:` line in the card ("" if none)."""
    last = ""
    for ln in card.split("\n"):
        s = ln.strip()
        if s.lower().startswith("status:"):
            last = s[len("status:"):].strip().lower()
    return last


def _detect_stack_summary(workspace_path: str) -> str:
    """One-line primary stack summary, detected once at creation."""
    stack = _detect_tech_stack(workspace_path) if os.path.isdir(workspace_path) else []
    seen, out = set(), []
    for s in stack:
        if s not in seen:
            seen.add(s)
            out.append(s)
    return ", ".join(out[:8]) or "unknown"


def _detect_version_summary(workspace_path: str) -> str:
    """One-line baseline version hint: engines / requires-python / unpinned."""
    try:
        pkg = os.path.join(workspace_path, "package.json")
        if os.path.isfile(pkg):
            import json as _json
            with open(pkg, "r", encoding="utf-8") as f:
                data = _json.load(f)
            eng = data.get("engines") or {}
            if eng.get("node"):
                return f"Node {eng['node']}"
            if eng.get("python"):
                return f"Python {eng['python']}"
    except Exception:
        pass
    try:
        pyproj = os.path.join(workspace_path, "pyproject.toml")
        if os.path.isfile(pyproj):
            with open(pyproj, "r", encoding="utf-8", errors="replace") as f:
                for ln in f:
                    s = ln.strip()
                    if s.startswith("requires-python"):
                        v = s.split("=", 1)[1].strip().strip('"')
                        return f"Python {v}"
    except Exception:
        pass
    return "unpinned"


def ensure_project_identity(workspace_path: str, name: str = "", description: str = "",
                            purpose: str = "", status: str = "draft") -> bool:
    """Create the write-once project identity card. Never touches an existing
    card (one exception: the old rich auto-generated template with a
    timestamp is migrated to the slim card exactly once).
    status: "draft" (default â€” dormant, not injected) or "stable" (e.g. the
    eval, where a task workspace IS a built project)."""
    if not workspace_path or not os.path.isdir(workspace_path):
        return False
    _ensure_dir(_memory_path(workspace_path))
    proj_file = _project_file(workspace_path)
    existing = _read_file_safe(proj_file)
    if existing and existing.strip():
        if "**Auto-generated**" not in existing:
            return False  # write-once: never rewrite a real card
        # One-time migration of the legacy auto-generated template
    name = (name or "").strip().replace("\n", " ") or os.path.basename(os.path.normpath(workspace_path))
    description = (description or "").strip().replace("\n", " ") or "No description provided."
    purpose = (purpose or "").strip().replace("\n", " ") or "No purpose specified."
    st = "stable" if str(status).lower() == "stable" else "draft"
    card = (
        "# Project\n"
        f"name: {name}\n"
        f"description: {description}\n"
        f"purpose: {purpose}\n"
        f"stack: {_detect_stack_summary(workspace_path)}\n"
        f"version: {_detect_version_summary(workspace_path)}\n"
        f"status: {st}\n"
    )
    if _write_file_safe(proj_file, card):
        print(f"[MEMORY] Project identity written once ({len(card)} chars, status={st})")
        return True
    return False


def _format_compact_project_identity(content: str, workspace_path: str = "") -> str:
    """Project raw card content into a clean, compact 4-line bullet format.

    - Fixed four-line order: project, purpose, stack, runtime
    - No markdown headings, timestamps, [updated] markers, or status lines
    - Last occurrence wins for each fact
    """
    proj_name = ""
    purpose = ""
    description = ""
    stack = ""
    runtime = ""

    # Injected identity must be immutable: drift appends ([updated:] /
    # [identity stabilized:]) describe reality churn but may never change
    # the SystemMessage bytes, or every resume would invalidate the whole
    # cached prefix. The projection freezes at the write-once head; the
    # full file keeps its audit trail and still drives the stable-gate,
    # which reads status from the complete file elsewhere.
    for marker in ("[updated:", "[identity stabilized:"):
        content = content.split(marker, 1)[0]

    for line in content.splitlines():
        s = line.strip()
        low = s.lower()
        if low.startswith("name:"):
            proj_name = s[len("name:"):].strip()
        elif low.startswith("project:"):
            proj_name = s[len("project:"):].strip()
        elif low.startswith("purpose:"):
            purpose = s[len("purpose:"):].strip()
        elif low.startswith("description:"):
            description = s[len("description:"):].strip()
        elif low.startswith("stack:"):
            stack = s[len("stack:"):].strip()
        elif low.startswith("version:"):
            runtime = s[len("version:"):].strip()
        elif low.startswith("runtime:"):
            runtime = s[len("runtime:"):].strip()

    final_proj = proj_name or (os.path.basename(os.path.normpath(workspace_path)) if workspace_path else "Project")
    final_purpose = purpose or description or "General development"
    final_stack = stack or (_detect_stack_summary(workspace_path) if workspace_path else "unknown")
    final_runtime = runtime or (_detect_version_summary(workspace_path) if workspace_path else "unpinned")

    return (
        f"- project: {final_proj}\n"
        f"- purpose: {final_purpose}\n"
        f"- stack: {final_stack}\n"
        f"- runtime: {final_runtime}"
    )


def _project_identity_for_injection(workspace_path: str) -> str:
    """The identity card content projected into compact format, ONLY when
    status is `stable` (draft cards are dormant â€” never injected).
    Read-only; creation/promotion happen in ensure_project_identity /
    refresh_project_identity."""
    if not workspace_path or not os.path.isdir(workspace_path):
        return ""
    content = _read_file_safe(_project_file(workspace_path))
    if not content or not content.strip():
        return ""
    if _card_last_status(content) != "stable":
        return ""
    return _format_compact_project_identity(content, workspace_path)


def load_cached_project_identity(workspace_path: str) -> str:
    """Read-only: the identity card for stable-zone injection, or "" when it
    is missing or still `draft`. Never generates side effects."""
    return _project_identity_for_injection(workspace_path)


def refresh_project_identity(workspace_path: str) -> bool:
    """Adapt the card to reality â€” APPEND-ONLY, like the code map.

    While `draft` the card is DORMANT: nothing is appended (it is not
    injected anywhere, so drift lines would be invisible churn). When the
    harness heuristic sees a structurally committed project (manifest +
    â‰¥5 source files + detectable stack) it appends `[identity stabilized:]`
    + `status: stable` ONCE â€” from then on the card is injected into the
    cached stable zone and reality drift appends `[updated:]` corrections
    at the BOTTOM. Old lines are never rewritten â€” the prefix stays
    byte-identical (cache-safe); last occurrence wins.
    name/description/purpose are identity â€” never auto-updated.
    """
    if not workspace_path or not os.path.isdir(workspace_path):
        return False
    proj_file = _project_file(workspace_path)
    existing = _read_file_safe(proj_file)
    if not existing or not existing.strip():
        return False
    stamp = datetime.utcnow().strftime("%Y-%m-%d %H:%M UTC")

    if _card_last_status(existing) != "stable":
        # Draft card: promote once when the project becomes structurally
        # committed; otherwise stay dormant (no drift appends).
        if _project_is_developed(workspace_path):
            updated = (existing.rstrip()
                       + "\n\n[identity stabilized: " + stamp + "]\n"
                       + "status: stable\n")
            if _write_file_safe(proj_file, updated):
                print("[MEMORY] Project identity stabilized (status: stable)")
                return True
        return False

    cur_stack = _detect_stack_summary(workspace_path)
    cur_version = _detect_version_summary(workspace_path)
    last_stack = last_version = ""
    for ln in existing.split("\n"):
        s = ln.strip()
        low = s.lower()
        if low.startswith("stack:"):
            last_stack = s[len("stack:"):].strip()
        elif low.startswith("version:"):
            last_version = s[len("version:"):].strip()
    delta = []
    if cur_stack and cur_stack != last_stack:
        delta.append(f"stack: {cur_stack}")
    if cur_version and cur_version != last_version:
        delta.append(f"version: {cur_version}")
    if not delta:
        return False
    updated = existing.rstrip() + "\n\n[updated: " + stamp + "]\n" + "\n".join(delta) + "\n"
    if _write_file_safe(proj_file, updated):
        print(f"[MEMORY] Project identity adapted ({', '.join(delta)})")
        return True
    return False


def ensure_project_cache(workspace_path: str) -> bool:
    """Ensure only the identity card and repository map cache exist.

    This is intentionally the sole automatic project-context initializer. It
    never creates feature, lesson, session, or greeting files.
    """
    mem_dir = _memory_path(workspace_path)
    _ensure_dir(mem_dir)

    before_identity = os.path.isfile(_project_file(workspace_path))
    ensure_project_identity(workspace_path)
    refresh_code_map(workspace_path)
    return not before_identity


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
    if (
        not content
        or not content.startswith("projectMap:")
        or content.count("projectMap:") != 1
        or "projectMap+:" in content
        or "Updates:" in content
    ):
        return ""
    return content


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

