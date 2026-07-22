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


def get_developer_memory_context(project_path: str, request: str) -> str:
    """Load relevant context from .agent-memory/ for the Dev subagent."""
    if not project_path or not os.path.isdir(project_path):
        return ""

    mem_dir = os.path.join(project_path, MEMORY_DIR)
    if not os.path.isdir(mem_dir):
        return ""

    parts = []

    # Load project overview (tech stack, goals, architecture)
    project_md = _read_file_safe(os.path.join(mem_dir, "project.md"))
    if project_md:
        # Strip the heading, keep the content
        lines = project_md.strip().split("\n")
        # Only include if it has real content (not just auto-generated template)
        if any(not line.startswith("**Auto-generated") and not line.startswith("#") and line.strip() for line in lines):
            parts.append(project_md)

    # Load features relevant to the request
    features_md = _read_file_safe(os.path.join(mem_dir, "features.md"))
    if features_md and request:
        req_lower = request.lower()
        words = [w.strip() for w in req_lower.split() if len(w) > 2]
        if words:
            sections = features_md.split("## ")
            relevant = [s for s in sections if any(w in s.lower() for w in words)]
            if relevant:
                parts.append("### Relevant Features:\n" + "\n".join(f"## {s}" for s in relevant))

    if not parts:
        return ""

    return (
        "\n\n=========================================\n"
        "PROJECT MEMORY\n"
        "=========================================\n\n"
        + "\n\n---\n\n".join(parts)
        + "\n\n========================================="
    )
