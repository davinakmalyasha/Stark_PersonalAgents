"""AddOnTool long-term project-memory wrappers."""

def update_feature(feature_name: str = "", name: str = "", description: str = "", file_refs: list = None) -> str:
    """Add/update a feature entry in long-term memory (features.md). Thin wrapper over memory_manager."""
    from tools import shared_state, WORKSPACE
    try:
        from memory.memory_manager import update_feature as _mem_feature
        ws = shared_state.get("project_path") or WORKSPACE
        fname = feature_name or name
        if not fname:
            return "Error: update_feature requires 'name' (or 'feature_name')."
        ok = _mem_feature(ws, fname, description, file_refs or [])
        return f"[OK] Feature '{fname}' updated in long-term memory." if ok else f"[ERR] Could not update feature '{fname}'."
    except Exception as e:
        return f"Error updating feature: {e}"

def update_project_meta(key: str = "", value: str = "") -> str:
    """Add/update a project metadata section in long-term memory (project.md). Thin wrapper over memory_manager."""
    from tools import shared_state, WORKSPACE
    try:
        from memory.memory_manager import update_project_meta as _mem_meta
        ws = shared_state.get("project_path") or WORKSPACE
        if not key:
            return "Error: update_project_meta requires 'key'."
        ok = _mem_meta(ws, key, value)
        allowed = "Tech Stack, Frontend, Database, Testing, Architecture, Project Type, Goals"
        return f"[OK] Project metadata '{key}' updated." if ok else f"[ERR] Unknown metadata key '{key}' (allowed: {allowed})."
    except Exception as e:
        return f"Error updating project metadata: {e}"
