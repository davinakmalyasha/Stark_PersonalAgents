"""
DEPRECATED: Old workspace_memory.json persistence.
Replaced by .agent-memory/ markdown files.
Kept as stub to prevent import errors. Returns empty data.
"""
import os
import copy

WORKSPACE_DIR = r"d:\MyProject\LangChain"

DEFAULT_STRUCTURE = {
    "past_requests": [],
    "lessons_learned": [],
    "global": {"preferences": {}},
    "it_department": {"technologies": [], "db_configs": {}, "file_blueprints": {}},
    "design_department": {"themes": {}, "ui_flow_specs": {}},
    "security": {"encryption_specs": {}, "permission_rules": {}},
}


def load_memory(workspace_path: str = "") -> dict:
    """Stub: returns empty structure. Memory is now in .agent-memory/."""
    return copy.deepcopy(DEFAULT_STRUCTURE)


def save_memory(data: dict, workspace_path: str = "") -> None:
    """Stub: no-op. Memory is now in .agent-memory/."""
    pass


def load_global_lessons() -> list:
    """Stub: returns empty list."""
    return []


def save_global_lessons(lessons: list) -> None:
    """Stub: no-op."""
    pass


def apply_segment_update(segment: dict, updates) -> None:
    """Stub: no-op."""
    pass
