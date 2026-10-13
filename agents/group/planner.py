"""agents.group.planner — deciding *what shape of work* a request is (Phase 3).

``docs/group-projects/03-orchestrator-tools.md`` sections 1-2.

"Is this a group project?" is currently an invisible model decision. This
module makes it a **tool call with a rationale**, so the choice is visible,
auditable, and overridable (``03`` section 2.2):

    plan_execution(goal, horizon, reversible, needs_human)
      -> { shape: solo | group | scheduled, rationale, environment,
           plan_preview, blocking_questions }

Three things are load-bearing here:

- **The classifier is deterministic by default.** :func:`classify_shape` is a
  scored heuristic, not a model call. That makes the routing regression test
  (``03`` section 2.3) runnable offline and its negative cases assertable —
  which matters more than cleverness, because the expensive failure is
  wrapping a trivial job in a three-day project.
- **An LLM planner is injectable.** :func:`plan_execution` takes ``planner``;
  the app passes a real one, tests pass none and get the deterministic
  fallback. Same seam as :class:`~agents.group.runner.ItemRunner`.
- **`@task` skips detection, it does not add syntax.** It is a flag the
  orchestrator sees, not a second router that can disagree with the first
  (``03`` section 4).
"""
from __future__ import annotations

import re
from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any

SHAPES = ("solo", "group", "scheduled")

#: Objectives usually name a project; this bounds what one plan may create.
DEFAULT_MAX_MEMBERS = 3
DEFAULT_MAX_ITEMS = 12


class PlanError(Exception):
    """The request cannot be turned into a plan as stated."""


# ── the @task force path ────────────────────────────────────────────────────

_TASK_DIRECTIVE = re.compile(r"^\s*@task\b[:\s]*(?P<goal>.*)$", re.IGNORECASE | re.DOTALL)


def parse_directive(text: str) -> tuple[bool, str]:
    """``("is it a forced @task", "the goal")``.

    Deliberately a *prefix flag*, not a parser. Anything cleverer grows a
    second routing system that can disagree with :func:`classify_shape`.
    """
    match = _TASK_DIRECTIVE.match(text or "")
    if not match:
        return False, (text or "").strip()
    return True, match.group("goal").strip()


# ── shape classification ────────────────────────────────────────────────────

_SCHEDULED = re.compile(
    r"\b("
    r"every\s+(day|weekday|morning|evening|night|hour|week|month|"
    r"monday|tuesday|wednesday|thursday|friday|saturday|sunday|"
    r"\d+\s*(minutes?|hours?|days?|weeks?))"
    r"|daily|weekly|monthly|nightly|hourly"
    r"|each\s+(day|week|month|morning|monday|tuesday|wednesday|thursday|friday)"
    r"|on\s+a\s+schedule|per\s+day|cron"
    r")\b",
    re.IGNORECASE,
)

_GROUP_STAGES = re.compile(
    r"\b(then|after\s+that|afterwards|followed\s+by|end\s+to\s+end|"
    r"step\s+by\s+step|and\s+then|once\s+.{0,30}\s+is\s+done|"
    r"before\s+(we|you|i)\s+(ship|launch|publish))\b",
    re.IGNORECASE,
)

_GROUP_VERBS = re.compile(
    r"\b(reconcile|audit|migrate|overhaul|investigate|sort\s+out|clean\s*up|"
    r"figure\s+out\s+why|find\s+out\s+why|get\s+.{0,20}\s+done|"
    r"take\s+care\s+of|put\s+together|wrap\s+up|plan\s+out)\b",
    re.IGNORECASE,
)

_GROUP_DOMAINS = re.compile(
    r"\b(research\b.{0,60}\b(write|draft|summari[sz]e|report)|"
    r"design\b.{0,60}\b(build|implement)|"
    r"(build|implement)\b.{0,60}\b(test|review|ship|deploy)|"
    r"document\b.{0,60}\b(ship|release))\b",
    re.IGNORECASE,
)

_SOLO = re.compile(
    r"\b(rename|reformat|format|convert|delete|remove|move|copy|list|"
    r"show\s+me|what\s+(is|are)|explain|summari[sz]e\s+this|"
    r"fix\s+the\s+typo|bump|install|print|echo|check\s+the\s+version|"
    r"write\s+a\s+(python\s+|node\s+|bash\s+)?script)\b",
    re.IGNORECASE,
)

_SOLO_MARKERS = re.compile(
    r"\b(just|quick|quickly|one\s+file|this\s+file|a\s+single|simple|"
    r"small|tiny|one[- ]off)\b",
    re.IGNORECASE,
)

#: Weights, all in one place so the routing test can be reasoned about.
W_SCHEDULED = 10
W_STAGE = 3
W_GROUP_VERB = 3
W_DOMAIN = 3
W_LONG_HORIZON = 2
W_NEEDS_HUMAN = 3
W_IRREVERSIBLE = 2
W_SOLO_VERB = 3
W_SOLO_MARKER = 2
W_SHORT = 2
W_LONG = 1          # long and rambling leans away from solo

SHORT_GOAL_WORDS = 10
LONG_GOAL_WORDS = 40


@dataclass
class Score:
    solo: int = 0
    group: int = 0
    scheduled: int = 0
    reasons: list[str] = field(default_factory=list)

    def add(self, shape: str, weight: int, why: str) -> None:
        setattr(self, shape, getattr(self, shape) + weight)
        self.reasons.append(f"{shape} +{weight}: {why}")

    @property
    def winner(self) -> str:
        """Highest score; ties go to ``solo`` — the cheapest, safest default."""
        best = max(self.solo, self.group, self.scheduled)
        for shape in ("solo", "group", "scheduled"):
            if getattr(self, shape) == best:
                return shape
        return "solo"  # pragma: no cover - the loop always returns


def score_shape(goal: str, *, horizon: str = "hours", reversible: bool = True,
                needs_human: bool = False) -> Score:
    """Score every shape against the goal. Pure; no I/O, no model."""
    score = Score()
    text = goal or ""
    words = len(text.split())

    if _SCHEDULED.search(text):
        score.add("scheduled", W_SCHEDULED, "recurrence wording")
    if _GROUP_STAGES.search(text):
        score.add("group", W_STAGE, "explicitly sequenced stages")
    if _GROUP_VERBS.search(text):
        score.add("group", W_GROUP_VERB, "multi-stage verb")
    if _GROUP_DOMAINS.search(text):
        score.add("group", W_DOMAIN, "more than one kind of expertise")
    if str(horizon).lower() == "days":
        score.add("group", W_LONG_HORIZON, "horizon is days")
    if needs_human:
        score.add("group", W_NEEDS_HUMAN, "will need a human decision")
    if not reversible:
        score.add("group", W_IRREVERSIBLE, "not everything can be undone")

    if _SOLO.search(text):
        score.add("solo", W_SOLO_VERB, "single-purpose verb")
    if _SOLO_MARKERS.search(text):
        score.add("solo", W_SOLO_MARKER, "explicitly small in scope")
    if words and words <= SHORT_GOAL_WORDS:
        score.add("solo", W_SHORT, "short single-clause request")
    elif words >= LONG_GOAL_WORDS:
        score.add("group", W_LONG, "long, multi-clause request")
    return score


def classify_shape(goal: str, *, horizon: str = "hours", reversible: bool = True,
                   needs_human: bool = False) -> tuple[str, str]:
    """``(shape, rationale)``. Deterministic, auditable, overridable."""
    score = score_shape(goal, horizon=horizon, reversible=reversible,
                        needs_human=needs_human)
    shape = score.winner
    rationale = (
        f"{shape} (solo={score.solo} group={score.group} "
        f"scheduled={score.scheduled}) — " + "; ".join(score.reasons[:4])
    )
    return shape, rationale


# ── environment proposal ────────────────────────────────────────────────────

_WORD = re.compile(r"[a-z0-9_]+")


def _tokens(text: str) -> set[str]:
    return {w for w in _WORD.findall((text or "").lower()) if len(w) > 2}


def propose_environment(goal: str, *, agents: list[dict],
                        skills: list[dict] | None = None,
                        mcp: list[dict] | None = None,
                        max_members: int = DEFAULT_MAX_MEMBERS) -> dict:
    """Pick the members (and their skills/MCP) this goal seems to need.

    A keyword-overlap heuristic, deliberately simple and inspectable: the
    orchestrator's real choice is an LLM call, and this is the deterministic
    fallback plus the shape the LLM must fill in. Every entry carries a
    ``why`` because ``03`` section 2.2 requires the choice to be explainable.
    """
    goal_tokens = _tokens(goal)
    scored: list[tuple[int, dict]] = []
    for agent in agents or []:
        name = str(agent.get("name") or agent.get("key") or "")
        if not name:
            continue
        blob = " ".join(str(agent.get(f) or "") for f in
                        ("name", "key", "display", "description", "when_to_use"))
        overlap = len(goal_tokens & _tokens(blob))
        scored.append((overlap, agent))
    scored.sort(key=lambda pair: (-pair[0], str(pair[1].get("name") or "")))

    members: list[dict] = []
    for overlap, agent in scored[:max(0, max_members)]:
        name = str(agent.get("name") or agent.get("key") or "")
        members.append({
            "name": name,
            "derived_from": str(agent.get("key") or agent.get("name") or name),
            "model": agent.get("model") or "",
            "thinking": agent.get("thinking") or "high",
            "tools": list(agent.get("tools") or []),
            "skills": [],
            "mcp": [],
            "why": (f"shares {overlap} term(s) with the goal"
                    if overlap else "default member (no clear match)"),
        })

    skill_entries = [
        {"name": s.get("name"), "why": "declared available to this project"}
        for s in (skills or [])
    ]
    mcp_entries = [
        {"server": m.get("name"), "why": "needed by a proposed member"}
        for m in (mcp or [])
    ]
    return {
        "members": members,
        "skills": skill_entries,
        "mcp": mcp_entries,
        "reusable": [],
        "note": ("" if members else
                 "no members proposed; the orchestrator becomes the sole member"),
    }


def propose_plan(goal: str, environment: dict, *,
                 max_items: int = DEFAULT_MAX_ITEMS) -> list[dict]:
    """A minimal, valid plan. The deterministic floor, not the good version.

    Every item carries non-empty acceptance criteria because DM-06 makes an
    item without them undispatachable — a preview that produces one would be
    a preview the system cannot run.
    """
    members = environment.get("members") or []
    owner = members[0]["name"] if members else ""
    return [
        {
            "id": "m1",
            "title": "Understand",
            "owner_member": owner,
            "items": [{
                "id": "m1.i1",
                "title": "Establish what is actually being asked",
                "detail": f"Restate the goal, list assumptions, and identify what "
                          f"is unknown. Goal: {goal}",
                "acceptance": [{"kind": "check", "check": "file_exists",
                                "path": "context/findings.md"}],
            }],
        },
        {
            "id": "m2",
            "title": "Do the work",
            "depends_on": ["m1"],
            "owner_member": owner,
            "items": [{
                "id": "m2.i1",
                "title": "Produce the deliverable",
                "detail": "Carry out the goal and leave the result in "
                          "context/artifacts/.",
                "acceptance": [{"kind": "check", "check": "glob_nonempty",
                                "pattern": "context/artifacts/*"}],
            }],
        },
    ]


# ── the tool ────────────────────────────────────────────────────────────────


@dataclass
class PlanPreview:
    shape: str
    rationale: str
    environment: dict
    plan: list[dict]
    blocking_questions: list[str] = field(default_factory=list)
    est_cost_tokens: int = 0
    forced: bool = False

    def to_dict(self) -> dict:
        return {
            "shape": self.shape,
            "rationale": self.rationale,
            "environment": self.environment,
            "plan_preview": self.plan,
            "blocking_questions": list(self.blocking_questions),
            "est_cost_tokens": self.est_cost_tokens,
            "forced": self.forced,
        }


def plan_execution(goal: str, *,
                   horizon: str = "hours",
                   reversible: bool = True,
                   needs_human: bool = False,
                   constraints: str = "",
                   agents: list[dict] | None = None,
                   skills: list[dict] | None = None,
                   mcp: list[dict] | None = None,
                   planner: Callable[..., dict] | None = None) -> PlanPreview:
    """Decide the shape of the work and propose an environment + plan.

    ``planner`` is an optional LLM callable returning
    ``{shape, rationale, environment, plan_preview, blocking_questions}``.
    When absent the deterministic classifier and proposals are used — which is
    what the routing test asserts against, so the test needs no model.
    """
    forced, clean_goal = parse_directive(goal)
    if not clean_goal:
        raise PlanError("nothing to plan: the goal is empty")

    environment = propose_environment(clean_goal, agents=agents or [],
                                      skills=skills, mcp=mcp)
    deterministic_shape, deterministic_rationale = classify_shape(
        clean_goal, horizon=horizon, reversible=reversible, needs_human=needs_human)

    shape = deterministic_shape
    rationale = deterministic_rationale
    plan = []
    blocking: list[str] = []

    if planner is not None:
        proposal = planner(goal=clean_goal, horizon=horizon,
                           reversible=reversible, needs_human=needs_human,
                           constraints=constraints,
                           environment=environment) or {}
        candidate = str(proposal.get("shape") or "")
        if candidate in SHAPES:
            shape = candidate
            rationale = str(proposal.get("rationale") or rationale)
        if proposal.get("environment"):
            environment = proposal["environment"]
        plan = list(proposal.get("plan_preview") or [])
        blocking = list(proposal.get("blocking_questions") or [])

    if not plan:
        plan = propose_plan(clean_goal, environment) if shape == "group" else []

    if shape == "group" and not environment.get("members"):
        blocking.append(
            "no member was proposed — this would run as a single-agent project; "
            "confirm that is intended"
        )
    for item in _iter_items(plan):
        if not (item.get("acceptance") or []):
            raise PlanError(
                f"proposed item {item.get('id')!r} has no acceptance criteria; "
                f"it could never reach ready (DM-06)"
            )

    return PlanPreview(
        shape=shape,
        rationale=rationale,
        environment=environment,
        plan=plan,
        blocking_questions=blocking,
        est_cost_tokens=_estimate_tokens(shape, plan),
        forced=forced,
    )


def _iter_items(plan: list[dict]) -> Any:
    for milestone in plan or []:
        yield from (milestone.get("items") or [])


def _estimate_tokens(shape: str, plan: list[dict]) -> int:
    """A deliberately crude floor, not a forecast.

    The budget ceiling is enforced against *real* accumulated usage
    (``02`` section 6.1); this number only exists so a preview can be
    compared with another preview.
    """
    base = {"solo": 40_000, "group": 250_000, "scheduled": 10_000}.get(shape, 50_000)
    return base + 60_000 * sum(1 for _ in _iter_items(plan))


def requires_confirmation(preview: PlanPreview) -> bool:
    """A forced ``@task`` needs no confirmation; a detected one does."""
    return not preview.forced
