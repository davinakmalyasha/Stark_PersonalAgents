"""
Agent error taxonomy (B8).

Typed error classes + a string classifier so every failure mode in the agent
loops (orchestrator, Dev, generic subagent loop) has ONE consistent handler
policy (requery / escalate / salvage) instead of scattered nudge counters.

Usage:
    err_cls = classify_tool_error(result_or_exception_text)
    action = handler_policy(err_cls)   # "requery" | "escalate" | "salvage" | "stop"
"""
from __future__ import annotations


class AgentError(Exception):
    """Base class for harness-level agent errors."""


class AgentFormatError(AgentError):
    """Model output could not be parsed (bad tool JSON, missing args)."""


class AgentBlockedError(AgentError):
    """Tool call blocked by allow-list / permission rules."""


class AgentPolicyError(AgentError):
    """Command/tool blocked by the mechanical policy engine."""


class AgentTimeoutError(AgentError):
    """Command or LLM call exceeded its time budget."""


class AgentContextOverflowError(AgentError):
    """Conversation exceeded the model context window."""


class AgentCostExceededError(AgentError):
    """Task exceeded its configured cost ceiling."""


_FORMAT_HINTS = (
    "did not parse", "missing arguments", "no arguments", "invalid json",
    "malformed", "failed to parse", "not a valid tool call", "unparseable",
    "could not be executed", "could not parse",
)

_BLOCKED_HINTS = (
    "blocked", "permission denied", "not in subagent permissions",
    "is not available", "denied", "not allowed",
)

_POLICY_HINTS = (
    "blocked for safety", "dangerous pattern", "command execution blocked",
)

_TIMEOUT_HINTS = (
    "timeout", "timed out", "too long to complete", "exceeded",
)

_OVERFLOW_HINTS = (
    "context length", "context window", "maximum context", "max context",
    "context is too long", "too many tokens", "context overflow",
    "token limit", "input is too long",
)

_COST_HINTS = (
    "cost limit", "cost ceiling", "budget exceeded", "insufficient balance",
    "balance", "billing",
)


def classify_tool_error(text: str) -> type[AgentError]:
    """Map raw error text/exception strings to a typed AgentError subclass."""
    low = (text or "").lower()
    if any(k in low for k in _OVERFLOW_HINTS):
        return AgentContextOverflowError
    if any(k in low for k in _COST_HINTS):
        return AgentCostExceededError
    if any(k in low for k in _POLICY_HINTS):
        return AgentPolicyError
    if any(k in low for k in _BLOCKED_HINTS):
        return AgentBlockedError
    if any(k in low for k in _TIMEOUT_HINTS):
        return AgentTimeoutError
    if any(k in low for k in _FORMAT_HINTS):
        return AgentFormatError
    return AgentError


def handler_policy(err_cls: type[AgentError]) -> str:
    """Return the harness action for an error class.

    requery  — privately re-ask the model with the error template
    escalate — stop this path, surface to the parent/user
    salvage  — package partial work (diff + failure trail) and exit
    stop     — terminate the loop
    """
    if err_cls is AgentFormatError or err_cls is AgentTimeoutError:
        return "requery"
    if err_cls is AgentContextOverflowError:
        return "salvage"
    if err_cls is AgentBlockedError or err_cls is AgentPolicyError:
        return "escalate"
    if err_cls is AgentCostExceededError:
        return "stop"
    return "escalate"
