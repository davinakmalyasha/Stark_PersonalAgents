"""
Agents Package
Provides orchestrator, developer, general, and subagent swarm management.
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

from .it_department_nodes_base import ITState
from .developer_prompt import _STATIC_SYSTEM_TEMPLATE as DEVELOPER_SYSTEM_PROMPT
from .developer_agent import (
    developer_node,
)
from .general_prompt import _GENERAL_SYSTEM_TEMPLATE as GENERAL_SYSTEM_PROMPT
from .general_agent import (
    general_node,
    run_general_direct,
)
from .orchestrator_agent import (
    orchestrator_node,
    _ORCHESTRATOR_SYSTEM_TEMPLATE,
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


__all__ = [
    "ITState",
    "DEVELOPER_SYSTEM_PROMPT",
    "GENERAL_SYSTEM_PROMPT",
    "developer_node",
    "general_node",
    "run_general_direct",
    "orchestrator_node",
    "_ORCHESTRATOR_SYSTEM_TEMPLATE",
    "start_continuable",
    "send_message",
    "interrupt",
    "report",
    "list_agents",
    "reset_manager",
    "current_child",
    "route_request",
    "stream_direct",
    "create_project",
    "find_project_dev",
    "list_projects",
    "find_project_by_workspace",
]
