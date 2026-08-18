"""Shared state type for the agent graph.

Legacy department roles (BA/SA/DevOps/Analytics) and the VFS/swarm helpers are
gone. This module only provides ``ITState`` — the shared graph state shape used
by ``developer_agent`` and ``orchestrator_agent``.
"""
from typing import TypedDict


class ITState(TypedDict, total=False):
    client_request: str
    requirements: str
    tech_spec: str
    code: str
    agent_report: str
    test_report: str
    devops_config: str
    analytics_report: str
    error_count: int
    next_agent: str
    project_path: str
    chat_id: str
    agents_plan: str
    active_tasks: list[str]
    requirements_updated: bool
    tech_spec_updated: bool
    code_updated: bool
    remaining_steps: int