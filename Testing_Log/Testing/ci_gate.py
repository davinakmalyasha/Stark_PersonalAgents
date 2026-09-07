"""ci_gate.py — REMOVED.

The harness auto-push has been deleted. Stub retained for import compat.
"""
from __future__ import annotations


def push_if_changed(ws: str, message: str, max_msg: int = 60) -> dict:  # noqa: ARG001
    return {"pushed": False, "sha": "", "repo": None, "branch": "", "note": "auto-push removed", "preview": ""}


def after_run(workspace_path: str, prompt: str = "", chat_id: str = "") -> None:  # noqa: ARG001
    return None


def resolve_levels(ws: str) -> dict | None:  # noqa: ARG001
    return None
