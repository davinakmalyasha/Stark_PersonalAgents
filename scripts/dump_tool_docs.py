"""Generate docs/tools/<tool>.md — per-tool contract for migration safety.

Reads the canonical single source of truth (tools/_schemas.py: TOOL_DEFINITIONS,
_SCOPE_MAP, _TOOL_DOCS, _TOOL_REQUIRED_ARGS) plus _parallel.PARALLEL_SAFE and the
registry pipeline (cache/redact/offload) and writes one markdown file per tool.

Usage: venv/Scripts/python scripts/dump_tool_docs.py
Output: docs/tools/<name>.md
"""
import os
import sys
import json

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from tools._schemas import (
    TOOL_DEFINITIONS,
    _TOOL_DOCS,
    _TOOL_REQUIRED_ARGS,
    _SCOPE_MAP,
)
from tools._parallel import PARALLEL_SAFE

OUT_DIR = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "docs", "tools")

# Tools migrated to accept typed WorkspaceDeps via the registry (ctx keyword).
# Registry passes deps only when the signature exposes a ctx/deps/run_ctx param.
_MIGRATED_TOOLS = {
    "read_file",
    "search_code",
    "edit_file",
    "run_command",
    "template",
}

# Native pydantic-ai 2.36 equivalent for our registry pipeline (Track B).
_NATIVE_NOTES = {
    "read_file": "Tool.from_schema(TOOL_DEFINITIONS) | cache via Hooks.before_tool_execute | FilteredToolset scope",
    "search_code": "Tool.from_schema | cache via Hooks | FilteredToolset",
    "edit_file": "Tool.from_schema | approval via Hooks.before_tool_execute (plan-mode + core-config veto)",
    "run_command": "Tool.from_schema | redact via Hooks.after_tool_execute | offload via after_tool_execute",
    "template": "Tool.from_schema | cache via Hooks | FilteredToolset",
    "default": "Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)",
}


def _param_lines(params: dict) -> str:
    props = (params or {}).get("properties", {})
    required = set((params or {}).get("required", []))
    lines = []
    for name, meta in props.items():
        req = "required" if name in required else "optional"
        desc = (meta or {}).get("description", "")
        lines.append(f"  - `{name}` ({meta.get('type', 'any')}, {req}): {desc}")
    if not lines:
        return "  - (none)"
    return "\n".join(lines)


def _scopes_for(name: str) -> str:
    scopes = [s for s, tools in _SCOPE_MAP.items() if name in tools]
    return ", ".join(scopes) if scopes else "(not exposed to any agent scope)"


def main() -> None:
    os.makedirs(OUT_DIR, exist_ok=True)
    by_name = {d["name"]: d for d in TOOL_DEFINITIONS}
    written = []
    for d in TOOL_DEFINITIONS:
        name = d["name"]
        params = d.get("parameters") or {}
        lines = [
            f"# {name}",
            "",
            f"- **Description:** {d.get('description', '(none)')}",
            f"- **Exposed to model:** {not d.get('exposed', False)}",
            f"- **Scopes (from _SCOPE_MAP):** {_scopes_for(name)}",
            f"- **Parallel-safe (PARALLEL_SAFE):** {name in PARALLEL_SAFE}",
            f"- **Compact doc:** {_TOOL_DOCS.get(name, '(none)')}",
            f"- **Required args (validated):** {', '.join(sorted(_TOOL_REQUIRED_ARGS.get(name, []))) or '(none)'}",
            f"- **Deps-migrated:** {name in _MIGRATED_TOOLS} (accepts ctx=WorkspaceDeps via registry shim)",
            f"- **Native equivalent (pydantic-ai 2.36):** {_NATIVE_NOTES.get(name, _NATIVE_NOTES['default'])}",
            f"- **Pipeline:** {'direct shim call (no hooks)' if name in _MIGRATED_TOOLS else 'thin registry shim: direct execute -> ToolExecutionResult'}",
            "",
            "## Parameters (JSON Schema)",
            "",
            _param_lines(params),
            "",
            "## How to recreate",
            "",
            "1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).",
            "2. `_TOOL_DOCS` one-liner.",
            "3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.",
            "4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).",
            "5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.",
            "6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.",
            "",
        ]
        path = os.path.join(OUT_DIR, f"{name}.md")
        with open(path, "w", encoding="utf-8") as f:
            f.write("\n".join(lines))
        written.append(name)

    # Also write the scope map itself for reference.
    scope_path = os.path.join(OUT_DIR, "_SCOPE_MAP.md")
    with open(scope_path, "w", encoding="utf-8") as f:
        f.write("# _SCOPE_MAP (tools/_schemas.py)\n\n")
        for scope, tools in _SCOPE_MAP.items():
            f.write(f"## {scope}\n\n")
            for t in tools:
                f.write(f"- {t}\n")
            f.write("\n")
    written.append("_SCOPE_MAP")

    print(f"Wrote {len(written)} docs to {OUT_DIR}: {', '.join(sorted(written))}")


if __name__ == "__main__":
    main()
