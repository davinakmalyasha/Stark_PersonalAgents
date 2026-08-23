"""MainTool — core agent workbench (file read/write/edit, run command, search).

Cross-cutting helpers live in _shared.py; per-tool files hold each tool's own
helpers. tools.py re-imports these so TOOL_MAP keeps resolving them by name.
"""
from ._shared import _clean_terminal_output, _fmt_args, _log_tool
from .editFile import (_apply_diff, apply_search_replace, apply_unified_diff,
                       _edit_snippet, edit_file, _write_file)
from .readFile import read_file
from .searchCode import search_code
from .runCommand import run_command
from .planTools import update_plan, get_plan_for_context

__all__ = [
    "_clean_terminal_output", "_fmt_args", "_log_tool",
    "_apply_diff", "apply_search_replace", "apply_unified_diff", "_edit_snippet",
    "edit_file", "_write_file",
    "read_file",
    "search_code",
    "run_command",
    "update_plan",
    "get_plan_for_context",
]
