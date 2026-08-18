"""
Memory context for the Dev subagent.
Reads from .agent-memory/ (markdown files) instead of old workspace_memory.json.
"""
import os
import re

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

    # Lessons journal — newest first, top 3, compacted to one line each.
    # Injected so future sessions learn past blockers without re-triggering them.
    lessons_md = _read_file_safe(os.path.join(mem_dir, "lessons.md"))
    if lessons_md:
        lesson_blocks = [b for b in lessons_md.split("## ") if b.strip()]
        lesson_blocks = [b for b in lesson_blocks if "[BLOCKED]" in b or "[FAILED]" in b][:3]
        if lesson_blocks:
            compact = []
            for b in lesson_blocks:
                lines = [l.strip() for l in b.splitlines() if l.strip()]
                header = lines[0][:120] if lines else "lesson"
                first_line = next((l for l in lines[1:] if not l.startswith("<!--") and not l.startswith("Error")), "")
                compact.append(f"- **{header}**{': ' + first_line[:220] if first_line else ''}")
            parts.append("### Project Lessons (past blockers — do not repeat)\n" + "\n".join(compact))

    # Top-K request-matched feature sections (never the code map section itself)
    if features_md and request:
        req_lower = request.lower()
        words = [w.strip() for w in req_lower.split() if len(w) > 2]
        if words:
            # Common outcome/date words — an echo summary restates the task's
            # DISTINCTIVE terms, not these; filtering them sharpens the overlap.
            _STOP = {"session", "implemented", "fixed", "fix", "pass", "passed", "passes",
                     "tests", "test", "all", "now", "with", "and", "the", "this", "that",
                     "work", "done", "complete", "completed", "updated", "added", "was", "are"}
            _tokens = lambda t: {w for w in re.findall(r"[a-z0-9]{3,}", t)
                                 if w not in _STOP and not w.isdigit()}
            req_tokens = _tokens(req_lower)
            relevant = []
            for s in features_md.split("## "):
                if not s.strip():
                    continue
                first_line = s.lower().splitlines()[0]
                if "code map" in first_line:
                    continue  # the code map is injected fresh per session
                if any(w in s.lower() for w in words):
                    # Echo-skip: a "## Session" entry that is a near-summary of
                    # THIS request (the orchestrator's just-written entry echoes
                    # the current task) carries zero new information. Only
                    # session-named sections with high DISTINCTIVE-token overlap
                    # are skipped; real feature memory is never affected.
                    try:
                        if first_line.startswith("session"):
                            sec_tokens = _tokens(s.lower())
                            if sec_tokens and req_tokens and \
                                    len(sec_tokens & req_tokens) / len(sec_tokens) > 0.4:
                                continue
                    except Exception:
                        pass
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
