"""Shared state shape for the Orchestrator → Dev graph."""
from typing import TypedDict


class ITState(TypedDict, total=False):
    client_request: str
    requirements: str
    tech_spec: str
    code: str
    agent_report: str
    test_report: str
    error_count: int
    next_agent: str
    project_path: str
    chat_id: str
    tech_spec_updated: bool
    code_updated: bool
