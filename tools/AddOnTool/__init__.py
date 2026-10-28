"""AddOnTool — extracted add-on implementations.

The legacy tools.py facade imports these names back so TOOL_MAP, execute_tool,
and existing callers retain their original identities and signatures.
"""
from .browserTools import (
    browser_navigate,
    _browser_cmd,
    browser_extract,
    browser_screenshot,
    browser_click,
    browser_fill,
    browser_close,
    browser_act,
)

from .githubTools import (
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
)

from .groupChat import group_message
from .orchestrationTools import (
    send_message,
    list_agents,
    interrupt_agent,
    report,
    ask_user,
    create_project,
)

from .workspaceTools import (
    capture_workspace_delta,
    _REDACT_PATTERNS,
    _GENERIC_SECRET_PAT,
    redact_secrets,
)

from .subagentTools import (
    SubagentsRegistry,
    _SUBAGENTS_REGISTRY,
    _TASK_DEPTH,
    _MAX_TASK_DEPTH,
    _load_subagents,
    _classify_error,
    _handoff,
    task,
    call_subagent,
    subagent_catalog,
)

from .delegationTools import (
    delegate,
    delegation_status,
    job_message,
    job_cancel,
)

from .scheduleTools import schedule
from .noteTools import give_note
from .canvasTools import canvas_mode

from .dfdBridge import dfd_bridge

from .screenTools import look_at_screen, set_screen_share

__all__ = [
    'browser_navigate',
    '_browser_cmd',
    'browser_extract',
    'browser_screenshot',
    'browser_click',
    'browser_fill',
    'browser_close',
    'browser_act',
    '_gh_env',
    '_run_gh',
    '_gh_api',
    'github_create_repo',
    'github_create_issue',
    'github_list_issues',
    '_current_repo',
    'github_view_issue',
    'github_close_issue',
    'github_comment_issue',
    'github_list_pull_requests',
    'github_view_pull_request',
    'github_create_pull_request',
    'github_merge_pull_request',
    'github_search_code',
    'github_search_repositories',
    'github_get_file_contents',
    'github_list_commits',
    'github_create_branch',
    'github_list_workflow_runs',
    'github_view_workflow_run',
    'github_dispatch_workflow',
    'send_message',
    'group_message',
    'list_agents',
    'interrupt_agent',
    'report',
    'ask_user',
    'create_project',
    'capture_workspace_delta',
    '_REDACT_PATTERNS',
    '_GENERIC_SECRET_PAT',
    'redact_secrets',
    'SubagentsRegistry',
    '_SUBAGENTS_REGISTRY',
    '_TASK_DEPTH',
    '_MAX_TASK_DEPTH',
    '_load_subagents',
    '_classify_error',
    '_handoff',
    'task',
    'call_subagent',
    'subagent_catalog',
    'delegate',
    'delegation_status',
    'job_message',
    'job_cancel',
    'schedule',
    'give_note',
    'canvas_mode',
    'dfd_bridge',
    'look_at_screen',
    'set_screen_share',
]
