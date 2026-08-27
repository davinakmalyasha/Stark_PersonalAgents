"""
Unified tools package — single source of truth for all tools, registries, and schemas.
"""
from __future__ import annotations

import sys
import os
import re
import json
import time
import tempfile
import subprocess
import glob as globmod
import threading
import collections
from typing import Optional, Any
from contextvars import ContextVar

# 1. Immediate schema and registry exports (before any cross-module imports)
from ._schemas import (
    TOOL_DEFINITIONS,
    get_native_tools,
    get_native_tool_names,
    get_compact_tool_docs,
    format_compact_tool_help,
    _TOOL_DOCS,
    _TOOL_REQUIRED_ARGS,
)
from ._registry import (
    ToolDefinition,
    ToolExecutionResult,
    ToolRegistry,
    active_tool_scope,
    get_registry,
    reset_registry,
    ensure_tool_registry,
    execute_tool,
    execute_tool_detailed,
    pre_validate_tool_args,
    TOOL_MAP,
    _tool_registry,
    _tool_registry_lock,
    _tool_cache_pre_hook,
    _tool_result_post_hook,
    validate_tool_definitions,
    _legacy_execute_tool,
    _current_scope,
    _current_chat,
)

# 2. Tool cluster implementations
from .DocTool import create_docx, create_xlsx, read_pdf
from .HouseTool import (
    house_get_state,
    house_get_device,
    house_set_device,
    house_run_scene,
    house_get_history,
)
from .MainTool import (
    _clean_terminal_output, _fmt_args, _log_tool,
    _apply_diff, apply_search_replace, apply_unified_diff, _edit_snippet,
    edit_file, _write_file, read_file, search_code, run_command,
    update_plan, get_plan_for_context,
)
from .AddOnTool import (
    browser_navigate,
    _browser_cmd,
    browser_extract,
    browser_screenshot,
    browser_click,
    browser_fill,
    browser_close,
    browser_act,
    _gh_env,
    _run_gh,
    _gh_api,
    github_create_repo,
    github_create_issue,
    github_list_issues,
    _current_repo,
    github_view_issue,
    github_close_issue,
    github_comment_issue,
    github_list_pull_requests,
    github_view_pull_request,
    github_create_pull_request,
    github_merge_pull_request,
    github_search_code,
    github_search_repositories,
    github_get_file_contents,
    github_list_commits,
    github_create_branch,
    github_list_workflow_runs,
    github_view_workflow_run,
    github_dispatch_workflow,
    _GATE_CHECKS,
    _GATE_PROFILE_TEMPLATES,
    _repo_default_branch,
    _detect_stack,
    github_apply_gate,
    _TEMPLATE_CATEGORIES,
    _TEMPLATES_DIR,
    task,
    call_subagent,
    subagent_catalog,
    _ASYNC_TASKS,
    start_async_task,
    check_async_task,
    list_async_tasks,
    async_task,
    ask_user,
    send_message,
    list_agents,
    interrupt_agent,
    report,
    apply_template,
    template_list,
    template,
    build_template_map,
    template_new,
    create_project,
    resume_project,
    _SUBAGENTS_REGISTRY,
    _TASK_DEPTH,
    _load_subagents,
    delegate,
    delegation_status,
)

# Shared state and caching utilities
from sync.state_sync import shared_state, safe_append_live_log

# Tool Caching
from ._runtime import (
    _TOOL_CACHE_TTL,
    _FILE_CACHE,
    _RUN_OUTPUT_CACHE,
    _TOOL_RESPONSE_CACHE,
    _TOOL_MTIME_MAP,
    WORKSPACE,
    active_permissions,
    enforce_fs_permission,
    _increment_cache_version,
    auto_offload_result,
    _tool_cap,
    _make_tool_cache_key,
    _check_tool_cache,
    _set_tool_cache,
    _invalidate_tool_cache_all,
    _invalidate_file_cache,
    WorkspaceFallbackBackend,
    get_vfs_router,
    _sanitize_path_fallback,
    _sanitize_path,
    _check_and_request_approval,
)

# Helpers
_HARNESS_ROOT = os.environ.get("HARNESS_ROOT", os.path.dirname(os.path.abspath(__file__)))


def _create_budgeted_preview(text: str, head_lines: int = 50, tail_lines: int = 50) -> str:
    lines = text.splitlines()
    total = len(lines)
    if total <= head_lines + tail_lines:
        return text
    head = "\n".join(lines[:head_lines])
    tail = "\n".join(lines[-tail_lines:])
    omitted = total - head_lines - tail_lines
    return f"{head}\n\n... [{omitted} lines omitted] ...\n\n{tail}"


def redact_secrets(text: str) -> str:
    patterns = [
        r'(?i)(api[_-]?key|secret|token|password|auth|bearer)\s*[:=]\s*["\']?([a-zA-Z0-9_\-\.]{8,})["\']?',
        r'ghp_[a-zA-Z0-9]{36}',
        r'gho_[a-zA-Z0-9]{36}',
        r'glpat-[a-zA-Z0-9\-_]{20,}',
        r'xox[baprs]-[0-9a-zA-Z]{10,48}',
        r'sk-[a-zA-Z0-9]{20,}',
    ]
    redacted = text
    for p in patterns:
        redacted = re.sub(p, r'\1: [REDACTED]', redacted)
    return redacted


# Backward compatibility alias: tools_pkg -> tools
sys.modules['tools_pkg'] = sys.modules[__name__]
sys.modules['tools_pkg._schemas'] = sys.modules.get('tools._schemas')
sys.modules['tools_pkg._registry'] = sys.modules.get('tools._registry')
