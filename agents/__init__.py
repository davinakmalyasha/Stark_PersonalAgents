"""
Agents Package
Provides orchestrator, developer, free agent, and subagent swarm management.
"""
import os
import sys

_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if _ROOT not in sys.path:
    sys.path.insert(0, _ROOT)

_DESKTOP = os.path.join(_ROOT, "Desktop")
if os.path.isdir(_DESKTOP) and _DESKTOP not in sys.path:
    sys.path.insert(0, _DESKTOP)

_VOICE = os.path.join(_ROOT, "Voice")
if os.path.isdir(_VOICE) and _VOICE not in sys.path:
    sys.path.insert(0, _VOICE)

_TESTING = os.path.join(_ROOT, "Testing_Log", "Testing")
if os.path.isdir(_TESTING) and _TESTING not in sys.path:
    sys.path.insert(0, _TESTING)

from .agent_errors import (
    AgentError,
    AgentFormatError,
    AgentBlockedError,
    AgentPolicyError,
    AgentTimeoutError,
    AgentContextOverflowError,
    AgentCostExceededError,
    classify_tool_error,
    handler_policy,
)
from .it_department_nodes_base import ITState
from .developer_prompt import _STATIC_SYSTEM_TEMPLATE as DEVELOPER_SYSTEM_PROMPT
from .developer_agent import (
    developer_node,
    parse_all_tool_calls,
    _parse_tool_call,
    get_harness_profile,
    looks_like_tool_call,
)
from .orchestrator_agent import (
    orchestrator_node,
    _ORCHESTRATOR_SYSTEM_TEMPLATE,
)
from .free_agent import (
    free_agent_node,
    run_freeagent_direct,
    chat,
    build_general_profile_prompt,
)
from .subagent_manager import (
    start_continuable,
    send_message,
    interrupt,
    report,
    list_agents,
    reset_manager,
    current_child,
)
from .project_router import (
    route_request,
    stream_direct,
    create_project,
    find_project_dev,
    list_projects,
    find_project_by_workspace,
)

from .subagent_swarm import (
    load_subagent_history,
    save_subagent_history,
    clear_subagent_history,
    get_subagent_history_path,
)

__all__ = [
    "AgentError",
    "AgentFormatError",
    "AgentBlockedError",
    "AgentPolicyError",
    "AgentTimeoutError",
    "AgentContextOverflowError",
    "AgentCostExceededError",
    "classify_tool_error",
    "handler_policy",
    "ITState",
    "DEVELOPER_SYSTEM_PROMPT",
    "developer_node",
    "parse_all_tool_calls",
    "_parse_tool_call",
    "looks_like_tool_call",
    "get_harness_profile",
    "orchestrator_node",
    "_ORCHESTRATOR_SYSTEM_TEMPLATE",
    "free_agent_node",
    "run_freeagent_direct",
    "chat",
    "build_general_profile_prompt",
    "start_continuable",
    "send_message",
    "interrupt",
    "report",
    "list_agents",
    "reset_manager",
    "current_child",
    "load_subagent_history",
    "save_subagent_history",
    "clear_subagent_history",
    "get_subagent_history_path",
    "route_request",
    "stream_direct",
    "create_project",
    "find_project_dev",
    "list_projects",
    "find_project_by_workspace",
]
