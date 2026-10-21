"""agents.jobs — harness-level durable job state (Phase 0).

Public surface: :class:`JobStore` and :func:`get_store`.
See ``docs/group-projects/07-phase-0-implementation.md``.
"""
from .store import (
    TERMINAL,
    JobStore,
    db_path,
    get_store,
    reset_store_for_tests,
)

__all__ = [
    "TERMINAL",
    "JobStore",
    "db_path",
    "get_store",
    "reset_store_for_tests",
]
