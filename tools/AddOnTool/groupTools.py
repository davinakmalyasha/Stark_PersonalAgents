"""tools.AddOnTool.groupTools — the orchestrator's group-project surface.

Phase 4 wiring for ``docs/group-projects/03-orchestrator-tools.md``. The real
logic lives in ``agents/group/`` and ``server/store/group_registry.py``; this
module is the thin, model-facing bridge, matching the house convention that a
tool returns a string the model can read (``[OK]`` / ``[ERR]``).

Design notes that matter:

- **``group_update`` is ONE tool, not sixteen.** The ops are separate; the tool
  is not. Every operation would otherwise cost a slot in the orchestrator's
  tool list and a paragraph of description, and the model picks worse from a
  long list of near-identical names.
- **``create_group_project`` takes structured arguments, not markdown.** A
  model writing a valid PROJECT.md — fenced YAML, quoted prompts, milestone
  DAG — in one tool call is a model that will sometimes get it wrong. The tool
  renders the manifest from arguments instead.
- Reads (``group_board``, ``group_item_detail``, ``group_events``) never queue,
  so asking "what is left?" answers immediately (``03`` §3.2).
"""
from __future__ import annotations

import json
import os

_TOOL_ERR = "[ERR]"


def _registry():
    """Lazy: keeps `tools` importable without a project root on disk."""
    from server.store import group_registry
    return group_registry


def _harness_root() -> str:
    return os.environ.get("HARNESS_ROOT") or os.path.dirname(
        os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def _catalogue_agents() -> list[dict]:
    """What agents a project could derive from — for plan_execution's proposal."""
    out: list[dict] = []
    try:
        from agents.workspace_registry import all_specs
        for spec in all_specs():
            if getattr(spec, "internal", False) or not getattr(spec, "delegatable", False):
                continue
            out.append({
                "name": spec.display,
                "key": spec.key,
                "display": spec.display,
                "description": spec.description,
                "tools": list(spec.tools),
            })
    except Exception:  # noqa: BLE001, S110 - no registry means no proposals
        pass
    return out


def _catalogue_skills() -> list[dict]:
    try:
        from tools.skills import discover_skills
        found = discover_skills(workspace_path=_registry().root(),
                                harness_root=_harness_root())
        return [{"name": name, "description": (v or {}).get("description") or ""}
                for name, v in sorted(found.items())]
    except Exception:  # noqa: BLE001
        return []


def _catalogue_mcp() -> list[dict]:
    try:
        return [{"name": name} for name in (_registry().mcp_catalogue().get("servers") or {})]
    except Exception:  # noqa: BLE001
        return []


# ════════════════════════════════════════════════════════════════════════════
#  plan_execution — the routing decision
# ════════════════════════════════════════════════════════════════════════════


def plan_execution(goal: str, horizon: str = "hours", reversible: bool = True,
                   needs_human: bool = False, constraints: str = "") -> str:
    """Decide the SHAPE of a request before starting anything.

    Returns ``solo``, ``group`` or ``scheduled`` plus a rationale, a proposed
    environment, and a plan preview. Always call this BEFORE
    ``create_group_project``.
    """
    try:
        toolkit = _registry().toolkit()
        preview = toolkit.plan_execution(
            goal, horizon=horizon, reversible=reversible,
            needs_human=needs_human, constraints=constraints,
            agents=_catalogue_agents(), skills=_catalogue_skills(),
            mcp=_catalogue_mcp(),
        )
    except Exception as exc:  # noqa: BLE001 - surfaced, not swallowed
        return f"{_TOOL_ERR} plan_execution failed: {type(exc).__name__}: {exc}"

    shape = preview.get("shape")
    lines = [f"[PLAN] shape={shape}", f"rationale: {preview.get('rationale')}"]
    env = preview.get("environment") or {}
    if env.get("members"):
        lines.append("members:")
        for member in env["members"]:
            lines.append(f"  - {member.get('name')} (from {member.get('derived_from')}) "
                         f"— {member.get('why')}")
    if preview.get("blocking_questions"):
        lines.append("needs confirmation:")
        for question in preview["blocking_questions"]:
            lines.append(f"  ? {question}")
    plan = preview.get("plan_preview") or []
    if plan:
        lines.append("plan preview:")
        for milestone in plan:
            lines.append(f"  {milestone.get('id')}: {milestone.get('title')}"
                         + (f" (after {', '.join(milestone['depends_on'])})"
                            if milestone.get("depends_on") else ""))
            for item in milestone.get("items") or []:
                lines.append(f"    - {item.get('id')}: {item.get('title')} "
                             f"[{len(item.get('acceptance') or [])} criteria]")
    lines.append(f"est tokens: {preview.get('est_cost_tokens')}")
    if shape == "group":
        lines.append("→ call create_group_project with a plan you are happy with")
    elif shape == "scheduled":
        lines.append("→ use the task scheduler (recurring), not a group project")
    else:
        lines.append("→ just do it; this does not need a project")
    return "\n".join(lines)


# ════════════════════════════════════════════════════════════════════════════
#  create_group_project
# ════════════════════════════════════════════════════════════════════════════


def _render_manifest(goal: str, milestones: list, members: list,
                     autonomy: dict, completion: dict,
                     budget_usd, budget_tokens, deadline,
                     skills: list, mcp: list, deps: dict,
                     project_id: str) -> str:
    """Turn structured arguments into PROJECT.md text."""
    import yaml

    slug = project_id or "-".join(
        (goal or "project").lower().split()[:4]).strip("-") or "project"
    slug = "".join(ch for ch in slug if ch.isalnum() or ch in "-_")[:48]

    data = {
        "schema": 1,
        "id": slug,
        "owner": "localhost",
        "goal": goal,
    }
    if budget_usd is not None:
        data["budget_usd"] = budget_usd
    if budget_tokens is not None:
        data["budget_tokens"] = budget_tokens
    if deadline:
        data["deadline"] = deadline
    data["completion"] = completion or {
        "kind": "all_milestones_done",
        "also_require": ["no_item_status_failed"],
    }
    data["autonomy"] = autonomy or {
        "project_folder": "autonomous",
        "external_read": "autonomous",
        "external_write": "needs_approval",
        "destructive": "blocked",
    }
    if members:
        data["members"] = members
    data["environment"] = {"skills": list(skills or []),
                           "mcp": list(mcp or []),
                           "deps": dict(deps or {})}
    data["milestones"] = milestones

    body = yaml.safe_dump(data, sort_keys=False, allow_unicode=True,
                          default_flow_style=False).rstrip()
    return (
        f"# {goal}\n\n"
        f"## Why\n{goal}\n\n"
        f"Authored by the orchestrator at creation. Edit freely — the prose is\n"
        f"yours, the block below is what the harness reads.\n\n"
        f"---\n```yaml\n{body}\n```\n"
    )


def create_group_project(goal: str, milestones: list, members: list | None = None,
                         autonomy: dict | None = None,
                         completion: dict | None = None,
                         budget_usd: float | None = None,
                         budget_tokens: int | None = None,
                         deadline: str | None = None,
                         skills: list | None = None,
                         mcp: list | None = None,
                         deps: dict | None = None,
                         project_id: str = "",
                         ctx: object = None) -> str:
    """Start a durable project that keeps working until it is done.

    Only after ``plan_execution`` returned ``shape=group`` and the human agreed
    the preview. Effects are queued; read ``group_board`` to follow progress.

    ``ctx`` is the run's typed ``WorkspaceDeps``, injected by the tool layer
    (``llm/native_tools.py`` and ``tools/_registry.py`` both bind any parameter
    named ``ctx``/``deps``/``run_ctx``/``context``). It is how this tool learns
    which workspace asked — the model never supplies it.
    """
    if not goal or not str(goal).strip():
        return f"{_TOOL_ERR} create_group_project needs a goal"
    if not milestones:
        return (f"{_TOOL_ERR} create_group_project needs at least one milestone. "
                f"Use plan_execution first to get a preview.")

    try:
        from agents.group.manifest import parse
        registry = _registry()
        text = _render_manifest(goal, milestones, members or [], autonomy,
                                completion, budget_usd, budget_tokens, deadline,
                                skills, mcp, deps, project_id)
        manifest = parse(text)
        manifest.validate()
        paths, _store = registry.create(registry.root(), manifest)
    except Exception as exc:  # noqa: BLE001 - surfaced to the model
        return f"{_TOOL_ERR} create_group_project failed: {type(exc).__name__}: {exc}"

    # Remember which workspace asked, in the HARNESS — never in the folder.
    # The portability contract (``01`` §14) forbids writing a root into a
    # project, and a project that loses this still resumes perfectly.
    origin = ""
    try:
        origin = registry.workspace_id_for_path(
            str(getattr(ctx, "project_path", "") or "")) or ""
        if origin:
            registry.set_origin_workspace(manifest.id, origin)
    except Exception:  # noqa: BLE001 - an unrecorded origin is not a failed create
        origin = ""

    return (f"[OK] created project {manifest.id}\n"
            f"path: {paths.project_dir}\n"
            f"milestones: {len(manifest.milestones)} · members: {len(manifest.members)}\n"
            + (f"origin workspace: {origin}\n" if origin else "")
            + f"the supervisor will start dispatching within one tick (~15s).\n"
            f"follow with group_board(project_id=\"{manifest.id}\")")


# ════════════════════════════════════════════════════════════════════════════
#  reads — these never queue
# ════════════════════════════════════════════════════════════════════════════


def group_board(project_id: str) -> str:
    """Current state of a project: milestones, items, spend, what is waiting."""
    try:
        return str(_registry().toolkit().board(project_id))
    except Exception as exc:  # noqa: BLE001
        return f"{_TOOL_ERR} group_board failed: {type(exc).__name__}: {exc}"


def group_item_detail(project_id: str, item_id: str) -> str:
    """One item in full: status, attempts, result, and its acceptance criteria."""
    try:
        return str(_registry().toolkit().item_detail(project_id, item_id=item_id))
    except Exception as exc:  # noqa: BLE001
        return f"{_TOOL_ERR} group_item_detail failed: {type(exc).__name__}: {exc}"


def group_events(project_id: str, since: int = 0, limit: int = 50) -> str:
    """The project's event log — what actually happened, in order."""
    try:
        return str(_registry().toolkit().events(project_id, since=since, limit=limit))
    except Exception as exc:  # noqa: BLE001
        return f"{_TOOL_ERR} group_events failed: {type(exc).__name__}: {exc}"


# ════════════════════════════════════════════════════════════════════════════
#  group_update — the single mutation entry point
# ════════════════════════════════════════════════════════════════════════════

_OPS = (
    "add_item", "add_milestone", "invalidate_item", "cancel_item",
    "add_member", "remove_member", "attach_tool",
    "attach_mcp", "detach_mcp", "attach_skill", "detach_skill",
    "pause", "resume", "close",
)


def group_update(project_id: str, op: str, params: dict | None = None) -> str:
    """Change a running project: items, members, MCP, skills, tools, lifecycle.

    Queued — the supervisor applies it on the next tick, and exactly one event
    is written. Read the result back with ``group_board``.
    """
    name = str(op or "").strip()
    if name not in _OPS:
        return f"{_TOOL_ERR} unknown op {name!r}; one of: {', '.join(_OPS)}"
    payload = dict(params or {})
    if name == "add_item" and not str(payload.get("item_id") or "").strip():
        # Caught here as well as in the supervisor so the model gets a usable
        # message instead of an op that bounces on the next tick.
        return (f"{_TOOL_ERR} add_item needs an item_id "
                f"(e.g. 'm2.i1') — without it the item cannot be addressed")
    try:
        toolkit = _registry().toolkit()
        method = getattr(toolkit, name, None)
        if method is None:
            return f"{_TOOL_ERR} op {name!r} has no implementation"
        result = method(project_id, **payload)
    except TypeError as exc:
        return f"{_TOOL_ERR} {name}: bad params ({exc})"
    except Exception as exc:  # noqa: BLE001
        return f"{_TOOL_ERR} {name} failed: {type(exc).__name__}: {exc}"
    if isinstance(result, str):
        return result
    return str(result)


# ── introspection for the registry test ─────────────────────────────────────

def list_ops() -> str:
    return json.dumps(list(_OPS))


GROUP_TOOLS = (
    "plan_execution", "create_group_project", "group_board",
    "group_item_detail", "group_events", "group_update",
)
