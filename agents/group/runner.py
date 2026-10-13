"""agents.group.runner — turning board items into work (Phase 2).

``docs/group-projects/02-runtime.md`` section 8.

The supervisor decides *when*; the runner does *how*. The supervisor never
imports an agent — it holds an :class:`ItemRunner` and calls four methods. That
seam is what lets the entire tick be tested with a fake, and what keeps the
supervisor free of any LLM dependency (RT-01).

Phases:

- 5 **dispatch** — supervisor claims the item, runner starts it
- 6 **collect**  — runner reports what finished; the supervisor records it
- 7 **verify**   — supervisor evaluates acceptance criteria and transitions

This module supplies:

- :class:`ItemRunner` — the interface, with inert defaults
- :class:`RunContext` — everything one run needs, resolved once
- :func:`build_prompt` — the six things a member is told (``02`` section 8)
- :class:`LocalRunner` — runs items in threads through the *materialised*
  member's own ``runtime`` entrypoint, with an injectable ``execute`` so tests
  never touch a model
"""
from __future__ import annotations

import importlib
import threading
import uuid
from collections.abc import Callable
from dataclasses import dataclass, field

#: Default parallel members. Deliberately small: every one is an LLM loop.
MAX_WORKERS = 4


@dataclass
class RunResult:
    """What one attempt produced. Harvested by the supervisor's phase 6."""

    item_id: str
    ok: bool
    report: str = ""
    error: str = ""
    cost_usd: float = 0.0
    cost_tokens: int = 0
    artifact: str = ""


@dataclass
class RunContext:
    """Everything one item run needs, resolved before it starts."""

    project_id: str
    project_root: str
    item: dict
    member: dict
    member_name: str
    chat_id: str
    autonomy: dict = field(default_factory=dict)


def build_prompt(ctx: RunContext) -> str:
    """The six things a member is told (``02`` section 8).

    The acceptance criteria are **inlined**, not referenced: a member that has
    to go and fetch its own success criteria will rationalise whatever it
    produced (open question D-7).
    """
    item = ctx.item
    lines = [
        f"# Work item {item['id']}",
        "",
        f"## {item.get('title') or '(untitled)'}",
        "",
        str(item.get("detail") or "").strip() or "(no further detail provided)",
        "",
        "## Done means",
    ]
    criteria = item.get("acceptance") or []
    if criteria:
        for index, criterion in enumerate(criteria, 1):
            from .criteria import describe
            lines.append(f"{index}. {describe(criterion)}")
    else:
        lines.append("(no acceptance criteria — this should not happen)")
    lines += [
        "",
        "## Project context",
        "- Read `context/decisions.md` and `context/findings.md` before you start.",
        "- Append what you *learn*, with sources, to `context/findings.md`.",
        "- Record decisions and their rejected alternatives in `context/decisions.md`.",
        f"- Your working directory is the project root: {ctx.project_root}",
        "",
        "## What you may do without asking",
    ]
    autonomy = ctx.autonomy or {}
    if autonomy:
        for key in ("project_folder", "external_read", "external_write", "destructive"):
            if key in autonomy:
                lines.append(f"- {key}: {autonomy[key]}")
    else:
        lines.append("- project_folder: autonomous (default); nothing outside it")
    lines += [
        "",
        "## How to finish",
        f"- Signal completion with `set_item_status` for item `{item['id']}`.",
        "- If something leaves the project folder, call `request_approval` first;",
        "  the item will park until a human answers, which is expected.",
        "- If the plan itself turns out to be wrong, say so and stop — do not",
        "  force the item through. It will be invalidated and re-planned.",
    ]
    return "\n".join(lines)


class ItemRunner:
    """What the supervisor needs from a runner. All methods are inert here."""

    def member_for(self, item: dict) -> str | None:
        """Which member should take this item (``None`` = any)."""
        return item.get("owner_member") or None

    def start(self, ctx: RunContext) -> None:
        """Begin the run. Must not block; the supervisor ticks on."""

    def collect(self, project_id: str) -> list[RunResult]:
        """Take every result that finished since the last call."""
        return []

    def cancel(self, project_id: str, item_id: str) -> None:
        """Best-effort cooperative cancel. The lease is the real backstop."""


class LocalRunner(ItemRunner):
    """Runs items in threads through the member's own runtime entrypoint.

    ``execute`` is injectable and defaults to :meth:`_execute_node`. Tests pass
    a fake so the tick can be exercised end to end with no model involved.

    In-flight state is process-local, exactly like ``delegation._DELEGATIONS``:
    a restart loses the thread, and the *lease* is what recovers the item. The
    member's conversation is not lost, because the transcript is persisted by
    the session store.
    """

    def __init__(self, *, execute: Callable[[RunContext], RunResult] | None = None,
                 max_workers: int = MAX_WORKERS,
                 log: Callable[[str], None] | None = None) -> None:
        self._execute = execute or self._execute_node
        self._max_workers = max_workers
        self._log = log or (lambda _msg: None)
        self._lock = threading.RLock()
        self._threads: dict[str, threading.Thread] = {}
        self._results: dict[str, list[RunResult]] = {}
        self._running: dict[str, str] = {}   # item_id -> project_id

    # ── interface ───────────────────────────────────────────────────────

    def start(self, ctx: RunContext) -> None:
        item_id = ctx.item["id"]
        with self._lock:
            if len(self._threads) >= self._max_workers:
                self._log(f"[RUNNER] at capacity, deferring {item_id}")
                return
            if item_id in self._running:
                return
            thread = threading.Thread(
                target=self._run, args=(ctx,), daemon=True,
                name=f"gp-{item_id}",
            )
            self._threads[item_id] = thread
            self._running[item_id] = ctx.project_id
            thread.start()

    def collect(self, project_id: str) -> list[RunResult]:
        with self._lock:
            pending = self._results.pop(project_id, [])
            for result in pending:
                self._running.pop(result.item_id, None)
                thread = self._threads.pop(result.item_id, None)
                if thread and thread.is_alive():
                    self._threads[result.item_id] = thread
        return pending

    def cancel(self, project_id: str, item_id: str) -> None:
        # Threads cannot be interrupted safely mid-LLM-call; the lease is the
        # real cancellation mechanism, and the run's result is ignored if the
        # item is no longer `doing` when it lands.
        with self._lock:
            self._running.pop(item_id, None)

    def active(self) -> int:
        with self._lock:
            return len(self._threads)

    # ── internals ───────────────────────────────────────────────────────

    def _run(self, ctx: RunContext) -> None:
        try:
            result = self._execute(ctx)
        except Exception as exc:  # noqa: BLE001 - a failed run is a result
            result = RunResult(item_id=ctx.item["id"], ok=False,
                               error=f"{type(exc).__name__}: {exc}")
        with self._lock:
            self._results.setdefault(ctx.project_id, []).append(result)

    def _execute_node(self, ctx: RunContext) -> RunResult:
        """Call the member's materialised ``runtime`` entrypoint.

        Mirrors ``delegation._call_node``: the registry entrypoint is a plain
        ``module:function`` taking a state dict and returning outputs. Reading
        ``runtime`` from the *materialised* agent, not the registry, is what
        makes the folder self-contained (``01`` section 5).
        """
        runtime = str(ctx.member.get("runtime") or "")
        if not runtime or ":" not in runtime:
            return RunResult(
                item_id=ctx.item["id"], ok=False,
                error=f"member {ctx.member_name!r} has no runtime entrypoint "
                      f"(derived_from={ctx.member.get('derived_from')!r})",
            )
        module_name, function_name = runtime.split(":", 1)
        module = importlib.import_module(module_name)
        node = getattr(module, function_name)
        state = {
            "client_request": build_prompt(ctx),
            "project_path": ctx.project_root,
            "chat_id": ctx.chat_id,
            "error_count": 0,
            "test_report": "",
        }
        output = node(state) or {}
        report = str(output.get("agent_report") or output.get("content") or "")
        return RunResult(
            item_id=ctx.item["id"],
            ok=not output.get("error"),
            report=report,
            error=str(output.get("error") or ""),
            cost_usd=float(output.get("cost_usd") or 0.0),
            cost_tokens=int(output.get("cost_tokens") or 0),
            artifact=str(output.get("artifact") or ""),
        )


def new_run_id() -> str:
    return f"run-{uuid.uuid4().hex[:8]}"


def resolve_members(manifest_members: list[dict],
                    agents: dict[str, dict]) -> dict[str, dict]:
    """Map member name -> materialised agent file.

    Raises if a member has no materialised agent: an unmaterialised member is
    a folder that cannot run itself (DM-07).
    """
    resolved: dict[str, dict] = {}
    for member in manifest_members:
        name = str(member.get("name") or "")
        agent = agents.get(name)
        if agent is None:
            raise KeyError(f"member {name!r} has no materialised agent (DM-07)")
        resolved[name] = agent
    return resolved
