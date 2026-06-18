"""
Memory context for the Dev subagent.
Reads from .agent-memory/ (markdown files) instead of old workspace_memory.json.
"""
import os

MEMORY_DIR = ".agent-memory"


def _read_file_safe(path: str) -> str | None:
    try:
        with open(path, "r", encoding="utf-8") as f:
            return f.read()
    except Exception:
        return None


def get_developer_memory_context(project_path: str, request: str, include_code_map_only: bool = False) -> str:
    """Load relevant context from .agent-memory/ for the Dev subagent.

    The code map is deliberately NOT included here — the Dev's
    `_build_dynamic_context` generates a FRESH repo map and injects it on every
    session (fresh or fixing), so any copy from features.md would be duplicate
    cache-miss input. This function returns curated memory only: project
    overview + top-K request-matched feature sections.
    `include_code_map_only=True` (repair/fix mode) returns "" — the fresh map
    injected by `_build_dynamic_context` covers navigation.
    Total block is capped at 6K chars.
    """
    if not project_path or not os.path.isdir(project_path):
        return ""

    mem_dir = os.path.join(project_path, MEMORY_DIR)
    if not os.path.isdir(mem_dir):
        return ""

    # Repair/fix mode: the fresh per-session map is the only navigation layer.
    if include_code_map_only:
        return ""

    # NOTE: project.md (the identity card) is deliberately NOT included here —
    # it is injected into the CACHED stable zone of every code-touching agent,
    # so a volatile copy here would be pure duplicate input.

    parts = []

    features_md = _read_file_safe(os.path.join(mem_dir, "features.md"))

    # Top-K request-matched feature sections (never the code map section itself)
    if features_md and request:
        req_lower = request.lower()
        words = [w.strip() for w in req_lower.split() if len(w) > 2]
        if words:
            relevant = []
            for s in features_md.split("## "):
                if not s.strip():
                    continue
                first_line = s.lower().splitlines()[0]
                if "code map" in first_line:
                    continue  # the code map is injected fresh per session
                if any(w in s.lower() for w in words):
                    relevant.append(s)
            if relevant:
                parts.append("### Relevant Features:\n" + "\n".join(f"## {s}" for s in relevant[:3]))

    if not parts:
        return ""

    block = (
        "\n\n=========================================\n"
        "PROJECT MEMORY\n"
        "=========================================\n\n"
        + "\n\n---\n\n".join(parts)
        + "\n\n========================================="
    )
    return block[:6000]
