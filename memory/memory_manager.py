"""
Long-term project memory. Persistent across sessions.
Auto-generates from codebase scan on first use.
Orchestrator reads at session start, updates as it discovers new info.
"""
import os
import json
import re
from datetime import datetime

MEMORY_DIR = ".agent-memory"


# ── Path helpers ─────────────────────────────────────────────────────────────

def _ensure_dir(path: str) -> None:
    os.makedirs(path, exist_ok=True)


def _memory_path(workspace_path: str) -> str:
    return os.path.join(workspace_path, MEMORY_DIR)


def _project_file(workspace_path: str) -> str:
    return os.path.join(_memory_path(workspace_path), "project.md")


def _features_file(workspace_path: str) -> str:
    return os.path.join(_memory_path(workspace_path), "features.md")


CODE_MAP_START = "<!-- CODE-MAP:START (auto-generated — do not edit) -->"
CODE_MAP_END = "<!-- CODE-MAP:END -->"


def _split_map_blocks(map_text: str) -> list[tuple[str, str]]:
    """Split a generated map into (file path, block text) pairs.

    Each block starts with a `<path>: ` line. Used to diff fresh generations
    against the persisted section for append-only updates.
    """
    blocks = []
    cur_path, cur_lines = None, []
    path_re = re.compile(r"^[^\s:]+(?:/[^\s:]+)*:\s")
    for line in map_text.split("\n"):
        if path_re.match(line):
            if cur_path:
                blocks.append((cur_path, "\n".join(cur_lines)))
            cur_path = line.split(":", 1)[0].strip()
            cur_lines = [line]
        else:
            cur_lines.append(line)
    if cur_path:
        blocks.append((cur_path, "\n".join(cur_lines)))
    return blocks


def refresh_code_map(workspace_path: str) -> bool:
    """Update the `## Code Map` section inside features.md — APPEND-ONLY.

    The map is the byte-stable source of truth injected into the cached
    system messages of Dev. Existing file blocks are NEVER
    rewritten or removed (that would shift every later byte and break the
    prefix cache) — only brand-new files get appended at the BOTTOM, behind
    a `[added: <utc>]` marker. Edited/deleted files keep their old block
    (harmless stale pointers — the agents read the real files).
    """
    if not workspace_path or not os.path.isdir(workspace_path):
        return False
    _ensure_dir(_memory_path(workspace_path))
    try:
        from memory.repo_map_generator import RepoMapGenerator
        generator = RepoMapGenerator(workspace_path)
        code_map = generator.generate_map()
    except Exception as e:
        print(f"[MEMORY] Code map generation failed: {e}")
        return False
    if not code_map or code_map.startswith("("):
        return False

    stamp = datetime.utcnow().strftime("%Y-%m-%d %H:%M UTC")
    feat_file = _features_file(workspace_path)
    existing = _read_file_safe(feat_file) or "# Features\n"

    if CODE_MAP_START in existing and CODE_MAP_END in existing:
        old = existing[existing.index(CODE_MAP_START):existing.index(CODE_MAP_END)]
        old_paths = {p for p, _ in _split_map_blocks(old)}
        new_blocks = _split_map_blocks(code_map)
        to_append = [b for p, b in new_blocks if p not in old_paths]
        if not to_append:
            return False
        delta = "\n\n[added: " + stamp + "]\n\n" + "\n\n".join(to_append)
        end = existing.index(CODE_MAP_END)
        updated = existing[:end] + delta + existing[end:]
    else:
        block = (
            f"{CODE_MAP_START}\n"
            "## Code Map (auto-generated, append-only — do not edit)\n"
            "Compact symbol index. OLDEST entries at the top, NEWEST at the "
            "bottom. Blocks are never rewritten — entries for edited or deleted "
            "files may be stale; read the real file. New files are appended "
            "below with an [added:] marker.\n\n"
            f"[generated: {stamp}]\n\n"
            f"{code_map}\n"
            f"{CODE_MAP_END}\n"
        )
        updated = existing.rstrip() + "\n\n" + block

    if _write_file_safe(feat_file, updated):
        print(f"[MEMORY] Refreshed code map in features.md ({len(code_map)} chars)")
        return True
    return False
# ── Auto-generation ──────────────────────────────────────────────────────────

# ── Project identity card ────────────────────────────────────────────────────
#
# project.md is the write-once IDENTITY CARD: name / description / purpose /
# stack / version + a status. It adapts to reality ONLY by APPENDING corrected
# `stack:`/`version:` lines at the bottom behind an [updated:] marker (same
# append-only mechanics as the code map) — old lines are never rewritten; the
# last occurrence wins.
#
# status: draft → stable (harness-decided, one-way):
#   * draft   — the project is young; the identity may still change. The card
#     is NOT injected anywhere and NO drift appends happen (all early
#     stack/purpose churn stays invisible to the API — zero cache impact).
#   * stable  — the project is structurally committed (manifest + ≥5 source
#     files + detectable stack). The card is injected into the CACHED stable
#     zone of every code-touching agent; reality drift appends below.
# Agents never decide this — the harness checks the heuristic at each
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
    'can no longer be re-purposed' line — once crossed, never demoted."""
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
    status: "draft" (default — dormant, not injected) or "stable" (e.g. the
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


def _project_identity_for_injection(workspace_path: str) -> str:
    """The identity card content, but ONLY when its status is `stable`
    (draft cards are dormant — never injected into any agent context).
    Read-only; creation/promotion happen in ensure_project_identity /
    refresh_project_identity."""
    if not workspace_path or not os.path.isdir(workspace_path):
        return ""
    content = _read_file_safe(_project_file(workspace_path))
    if not content or not content.strip():
        return ""
    if _card_last_status(content) != "stable":
        return ""
    return content.strip()


def load_cached_project_identity(workspace_path: str) -> str:
    """Read-only: the identity card for stable-zone injection, or "" when it
    is missing or still `draft`. Never generates side effects."""
    return _project_identity_for_injection(workspace_path)


def refresh_project_identity(workspace_path: str) -> bool:
    """Adapt the card to reality — APPEND-ONLY, like the code map.

    While `draft` the card is DORMANT: nothing is appended (it is not
    injected anywhere, so drift lines would be invisible churn). When the
    harness heuristic sees a structurally committed project (manifest +
    ≥5 source files + detectable stack) it appends `[identity stabilized:]`
    + `status: stable` ONCE — from then on the card is injected into the
    cached stable zone and reality drift appends `[updated:]` corrections
    at the BOTTOM. Old lines are never rewritten — the prefix stays
    byte-identical (cache-safe); last occurrence wins.
    name/description/purpose are identity — never auto-updated.
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


def auto_generate_memory(workspace_path: str) -> bool:
    """Scan workspace and generate initial project.md + features.md.
    Returns True if files were created, False if they already existed.
    """
    mem_dir = _memory_path(workspace_path)
    _ensure_dir(mem_dir)

    proj_file = _project_file(workspace_path)
    feat_file = _features_file(workspace_path)

    # Don't overwrite existing memory
    if os.path.isfile(feat_file):
        return False

    # project.md — the write-once identity card (name/description/purpose/
    # stack/version), seeded from detection ONCE, then frozen. It adapts to
    # reality only via append-only [updated:] drift corrections.
    ensure_project_identity(workspace_path)

    # Write features.md (starts as template — orchestrator populates)
    feat_lines = [
        "# Features",
        f"**Auto-generated**: {datetime.now().strftime('%Y-%m-%d')}",
        "",
        "_Features are added here by the orchestrator as work is done._",
        "",
    ]
    _write_file_safe(feat_file, "\n".join(feat_lines))

    # Seed the Aider-style code map immediately so features.md is useful
    # on a fresh project instead of an empty template.
    refresh_code_map(workspace_path)

    print(f"[MEMORY] Auto-generated at {mem_dir}")
    return True


# ── Tech detection ───────────────────────────────────────────────────────────

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


# ── Reading ──────────────────────────────────────────────────────────────────

def _strip_code_map_section(features_md: str) -> str:
    """Remove the Code Map section from features.md content.

    Used by load_project_memory: the orchestrator dispatches tasks, it does
    NOT navigate code — the map is injected (cached) into Dev
    system messages instead, and would be dead weight here.
    """
    if CODE_MAP_START in features_md and CODE_MAP_END in features_md:
        start = features_md.index(CODE_MAP_START)
        end = features_md.index(CODE_MAP_END) + len(CODE_MAP_END)
        return features_md[:start] + features_md[end:]
    return features_md


def load_cached_code_map(workspace_path: str) -> str:
    """Return the persisted, byte-stable Code Map section (features.md), or "".

    Read-only — never generates side effects. Generation/append happens in
    refresh_code_map (eval attempt start / orchestrator session / daemon).
    The bytes are stable across sessions, so the map cache-hits once warm.
    """
    if not workspace_path or not os.path.isdir(workspace_path):
        return ""
    existing = _read_file_safe(_features_file(workspace_path))
    if not existing or CODE_MAP_START not in existing or CODE_MAP_END not in existing:
        return ""
    start = existing.index(CODE_MAP_START) + len(CODE_MAP_START)
    end = existing.index(CODE_MAP_END)
    content = existing[start:end].strip()
    if not content or content.startswith("("):
        return ""
    return content


def load_project_memory(workspace_path: str) -> str:
    """Read memory files and return formatted string for Zone 2 injection."""
    mem_dir = _memory_path(workspace_path)
    if not os.path.isdir(mem_dir):
        return ""

    parts = []
    # The identity card is included only when stable — draft cards are
    # dormant (the orchestrator sees no card until the project is committed).
    project = _project_identity_for_injection(workspace_path)
    if project:
        parts.append(project)

    features = _read_file_safe(_features_file(workspace_path))
    if features:
        # The Code Map section is NOT included — the orchestrator dispatches
        # tasks and never navigates code; the map is injected (cached) into
        # the code-touching subagents' system messages instead.
        features = _strip_code_map_section(features)
        lines = features.strip().split("\n")
        if len(lines) > 3:
            parts.append(features)

    # MEMORY.md is retired — its "LLM lessons from chat logs" role is replaced
    # by the verified failure→fix pattern library (error_patterns.py), which
    # only stores evidence-backed lessons (hidden-test-verified diffs).

    if not parts:
        return ""
    return "\n\n---\n\n".join(parts)


# ── Writing ──────────────────────────────────────────────────────────────────

def update_feature(workspace_path: str, feature_name: str, description: str = "", file_refs: list[dict] | None = None) -> bool:
    """Add or update a feature entry in features.md.
    file_refs: [{"path": "file.py", "lines": "45-120", "purpose": "main logic"}, ...]
    Description is persisted as the entry's summary line when provided.
    """
    if file_refs is None:
        file_refs = []
    feat_file = _features_file(workspace_path)
    _ensure_dir(_memory_path(workspace_path))

    existing = _read_file_safe(feat_file)
    lines = existing.split("\n") if existing else ["# Features", ""]

    entry_lines = [""]
    entry_lines.append(f"## {feature_name}")
    if description and description.strip():
        entry_lines.append(description.strip())
        entry_lines.append("")
    for ref in file_refs:
        path = ref.get("path", "")
        lr = ref.get("lines", "")
        purpose = ref.get("purpose", "")
        if lr:
            entry_lines.append(f"- `{path}:{lr}` — {purpose}")
        else:
            entry_lines.append(f"- `{path}` — {purpose}")
    entry_lines.append("")

    # Replace existing feature or append
    heading = f"## {feature_name}"
    if heading in lines:
        start = lines.index(heading)
        end = len(lines)
        for i in range(start + 1, len(lines)):
            if lines[i].startswith("## "):
                end = i
                break
        lines = lines[:start] + entry_lines[1:] + lines[end:]
    else:
        lines = lines + entry_lines[1:]

    result = _write_file_safe(feat_file, "\n".join(lines))
    if result:
        print(f"[MEMORY] Updated feature: {feature_name}")
    return result


def update_project_meta(workspace_path: str, key: str, value: str) -> bool:
    """Agent-facing project metadata tool — REROUTED to features.md.

    project.md is the write-once identity card injected into the cached
    stable zone; agents must never rewrite it (any change would break cache
    identity for every code-touching agent). Session knowledge belongs in
    features.md.
    """
    return update_feature(workspace_path, key, str(value)[:400])


# ── Lessons journal (lessons.md) ─────────────────────────────────────────────

def _lessons_file(workspace_path: str) -> str:
    return os.path.join(_memory_path(workspace_path), "lessons.md")


def append_lesson(workspace_path: str, status: str, title: str, body: str, max_entries: int = 20) -> bool:
    """Append a failure/blocker lesson to lessons.md (deduped, bounded).

    status: BLOCKED / FAILED / SAVED (SAVED = agent-initiated via save_memory).
    Dedup: identical (title + body head) entries are skipped — a recurring
    failure must not stack duplicate lessons.
    Keeps the newest max_entries, oldest dropped.
    """
    if not workspace_path:
        return False
    try:
        _ensure_dir(_memory_path(workspace_path))
        fpath = _lessons_file(workspace_path)
        from datetime import datetime
        stamp = datetime.now().strftime("%Y-%m-%d %H:%M")
        import hashlib
        dedup_key = hashlib.sha256(f"{title}|{body[:300]}".encode("utf-8", "replace")).hexdigest()[:12]

        existing = _read_file_safe(fpath) or "# Lessons\n"
        if dedup_key in existing:
            return True  # already recorded — skip duplicates

        entry = (
            f"\n## {stamp} [{status}] {title[:120]}\n"
            f"<!-- lesson-id: {dedup_key} -->\n"
            f"{body.strip()[:600]}\n"
        )
        updated = existing + entry

        # Bound the journal to the newest max_entries (drop oldest blocks)
        blocks = [b for b in updated.split("## ") if b.strip()]
        if len(blocks) > max_entries + 1:  # +1 for the "# Lessons" header
            updated = "# Lessons" + "".join(f"\n## {b}" for b in blocks[-max_entries:])

        if _write_file_safe(fpath, updated):
            print(f"[MEMORY] Lesson appended: [{status}] {title[:60]}")
            return True
    except Exception as e:
        print(f"[MEMORY] Lesson append failed: {e}")
    return False


# ── File helpers ─────────────────────────────────────────────────────────────

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


# ── Convenience ──────────────────────────────────────────────────────────────

def ensure_memory(workspace_path: str) -> str:
    """Ensure memory exists (auto-generate if needed), refresh the code map
    and project identity (append-only), and return formatted content."""
    if not workspace_path or not os.path.isdir(workspace_path):
        return ""
    auto_generate_memory(workspace_path)
    refresh_code_map(workspace_path)  # keep the symbol index fresh each session
    refresh_project_identity(workspace_path)  # rare append-only reality drift
    return load_project_memory(workspace_path)


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
