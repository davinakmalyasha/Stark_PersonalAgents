"""agents.group — durable group projects (Phase 1).

Public surface:

- :mod:`agents.group.events` — the frozen event vocabulary (decision D-13)
- :class:`agents.group.store.ProjectStore` — events (truth) + projections
- :class:`agents.group.supervisor.Supervisor` — the deterministic tick

Nothing here calls an LLM. See ``docs/group-projects/`` for the design.
"""
from . import events as events
from .store import ProjectStore, RebuildMismatch, State, db_path, evaluate_completion
from .supervisor import OpRejected, Supervisor, recover_all_projects

__all__ = [
    "OpRejected",
    "ProjectStore",
    "RebuildMismatch",
    "State",
    "Supervisor",
    "db_path",
    "evaluate_completion",
    "events",
    "recover_all_projects",
]
