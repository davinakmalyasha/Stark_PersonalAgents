"""agents.group — durable group projects (Phases 1-2).

Public surface:

- :mod:`agents.group.events` — the frozen event vocabulary (decision D-13)
- :class:`agents.group.store.ProjectStore` — events (truth) + projections
- :class:`agents.group.supervisor.Supervisor` — the deterministic tick
- :mod:`agents.group.manifest` — ``PROJECT.md`` parse / render
- :mod:`agents.group.project` — the folder, the portability contract, creation
- :mod:`agents.group.materialize` — writing the capability set into a folder
- :mod:`agents.group.criteria` — acceptance-criteria evaluation
- :class:`agents.group.runner.LocalRunner` — running items through members

Nothing here calls an LLM except ``runner.LocalRunner._execute_node``, which
is the one seam that does. See ``docs/group-projects/`` for the design.
"""
from . import criteria as criteria
from . import events as events
from . import manifest as manifest
from . import materialize as materialize
from . import planner as planner
from . import project as project
from . import runner as runner
from . import tools as tools
from .planner import PlanPreview, classify_shape, parse_directive, plan_execution
from .runner import ItemRunner, LocalRunner, RunContext, RunResult, build_prompt
from .store import ProjectStore, RebuildMismatch, State, db_path, evaluate_completion
from .supervisor import OpRejected, ProjectContext, Supervisor, recover_all_projects
from .tools import GroupToolkit, ToolResult

__all__ = [
    "GroupToolkit",
    "ItemRunner",
    "LocalRunner",
    "OpRejected",
    "PlanPreview",
    "ProjectContext",
    "ProjectStore",
    "RebuildMismatch",
    "RunContext",
    "RunResult",
    "State",
    "Supervisor",
    "ToolResult",
    "build_prompt",
    "classify_shape",
    "criteria",
    "db_path",
    "evaluate_completion",
    "events",
    "manifest",
    "materialize",
    "parse_directive",
    "plan_execution",
    "planner",
    "project",
    "recover_all_projects",
    "runner",
    "tools",
]
