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
# ── Auto-generation ──────────────────────────────────────────────────────────

def auto_generate_memory(workspace_path: str) -> bool:
    """Scan workspace and generate initial project.md + features.md.
    Returns True if files were created, False if they already existed.
    """
    mem_dir = _memory_path(workspace_path)
    _ensure_dir(mem_dir)

    proj_file = _project_file(workspace_path)
    feat_file = _features_file(workspace_path)

    # Don't overwrite existing memory
    if os.path.isfile(proj_file) or os.path.isfile(feat_file):
        return False

    # Detect tech stack and project info
    stack = _detect_tech_stack(workspace_path)
    db = _detect_database(workspace_path)
    frontend = _detect_frontend(workspace_path)
    test_fw = _detect_test_framework(workspace_path)
    arch = _detect_architecture(workspace_path)

    # Write project.md
    proj_lines = [
        "# Project Overview",
        f"**Auto-generated**: {datetime.now().strftime('%Y-%m-%d')}",
        "",
        "## Tech Stack",
    ]
    if stack:
        for s in stack:
            proj_lines.append(f"- {s}")
    else:
        proj_lines.append("- _Not detected_")
    proj_lines.append("")

    if frontend:
        proj_lines.append("## Frontend")
        for f in frontend:
            proj_lines.append(f"- {f}")
        proj_lines.append("")

    if db:
        proj_lines.append("## Database")
        proj_lines.append(f"- {db}")
        proj_lines.append("")

    if test_fw:
        proj_lines.append("## Testing")
        proj_lines.append(f"- {test_fw}")
        proj_lines.append("")

    if arch:
        proj_lines.append("## Architecture")
        for a in arch[:5]:
            proj_lines.append(f"- {a}")
        proj_lines.append("")

    # Project type
    proj_lines.append("## Project Type")
    has_python = any("Python" in s for s in stack)
    has_node = any("Node.js" in s for s in stack)
    has_ai = any("LangChain" in s or "LangGraph" in s for s in stack)
    if has_ai:
        proj_lines.append("- AI agent orchestration system")
    if has_python:
        proj_lines.append("- Python application")
    if has_node:
        proj_lines.append("- Node.js application")
    if not has_python and not has_node and not has_ai:
        proj_lines.append("- _Auto-detection pending_")
    proj_lines.append("")

    proj_lines.append("## Goals")
    proj_lines.append("- _Auto-detected. Update as the project evolves._")
    proj_lines.append("")

    _write_file_safe(proj_file, "\n".join(proj_lines))

    # Write features.md (starts as template — orchestrator populates)
    feat_lines = [
        "# Features",
        f"**Auto-generated**: {datetime.now().strftime('%Y-%m-%d')}",
        "",
        "_Features are added here by the orchestrator as work is done._",
        "",
    ]
    _write_file_safe(feat_file, "\n".join(feat_lines))

    print(f"[MEMORY] Auto-generated at {mem_dir}")
    return True


# ── Tech detection ───────────────────────────────────────────────────────────

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

def load_project_memory(workspace_path: str) -> str:
    """Read memory files and return formatted string for Zone 2 injection."""
    mem_dir = _memory_path(workspace_path)
    if not os.path.isdir(mem_dir):
        return ""

    parts = []
    project = _read_file_safe(_project_file(workspace_path))
    if project:
        parts.append(project)

    features = _read_file_safe(_features_file(workspace_path))
    if features:
        lines = features.strip().split("\n")
        if len(lines) > 3:
            parts.append(features)

    if not parts:
        return ""
    return "\n\n---\n\n".join(parts)


# ── Writing ──────────────────────────────────────────────────────────────────

def update_feature(workspace_path: str, feature_name: str, description: str = "", file_refs: list[dict] | None = None) -> bool:
    """Add or update a feature entry in features.md.
    file_refs: [{"path": "file.py", "lines": "45-120", "purpose": "main logic"}, ...]
    Description parameter is unused — only heading + file refs are written.
    """
    if file_refs is None:
        file_refs = []
    feat_file = _features_file(workspace_path)
    _ensure_dir(_memory_path(workspace_path))

    existing = _read_file_safe(feat_file)
    lines = existing.split("\n") if existing else ["# Features", ""]

    entry_lines = [""]
    entry_lines.append(f"## {feature_name}")
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


_ALLOWED_PROJECT_SECTIONS = {
    "Tech Stack", "Frontend", "Database", "Testing",
    "Architecture", "Project Type", "Goals",
}


def update_project_meta(workspace_path: str, key: str, value: str) -> bool:
    """Update a section in project.md. Only known sections allowed."""
    if key not in _ALLOWED_PROJECT_SECTIONS:
        print(f"[MEMORY] Ignored unknown section key: '{key}' — not in allowed list")
        return False

    proj_file = _project_file(workspace_path)
    _ensure_dir(_memory_path(workspace_path))

    existing = _read_file_safe(proj_file)
    if not existing:
        existing = "# Project Overview\n"
    lines = existing.split("\n")

    heading = f"## {key}"
    new_section = [heading, value, ""]

    if heading in lines:
        start = lines.index(heading)
        end = len(lines)
        for i in range(start + 1, len(lines)):
            if lines[i].startswith("## "):
                end = i
                break
        lines = lines[:start] + new_section + lines[end:]
    else:
        lines = lines + [""] + new_section

    result = _write_file_safe(proj_file, "\n".join(lines))
    if result:
        print(f"[MEMORY] Updated project.{key}")
    return result


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
    """Ensure memory exists (auto-generate if needed), return formatted content."""
    if not workspace_path or not os.path.isdir(workspace_path):
        return ""
    auto_generate_memory(workspace_path)
    return load_project_memory(workspace_path)
