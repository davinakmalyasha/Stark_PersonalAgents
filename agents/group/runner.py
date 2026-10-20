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

import hashlib
import importlib
import json
import os
import threading
import uuid
from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any

from . import events as ev

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
    #: The project's store. Needed by the approval gate, which parks the item
    #: and reads back the shapes a human has already approved.
    store: Any = None


# ── the approval gate (Phase 5) ─────────────────────────────────────────────
#
# ``04-autonomy-and-approvals.md`` §3–§5. The envelope used to be a paragraph
# in the member's prompt: something the model was asked to respect. This turns
# it into the thing that decides, and it is the difference between "refused"
# and "asked" — before it, a member that needed to act outside its folder got
# an error and there was no path to consent at all.
#
# All the policy is in :mod:`agents.group.policy`, which is pure. This module
# supplies only the facts policy cannot know (where the project is, what its
# envelope says, what a human has already approved) and records the question.


def _norm(path: str) -> str:
    return os.path.normcase(os.path.normpath(os.path.abspath(path)))


def path_is_inside(root: str, path: str) -> bool:
    """True when ``path`` is the project folder or something under it."""
    if not root or not path:
        return False
    try:
        root_n, path_n = _norm(root), _norm(path)
    except Exception:  # noqa: BLE001 - an unresolvable path is not "inside"
        return False
    return path_n == root_n or path_n.startswith(root_n + os.sep)


def _resolve(root: str, target: str) -> str:
    """Resolve a tool's path argument the way the tool will.

    Relative paths are relative to the project folder, because that is the
    member's working directory in every way that matters.
    """
    if not target:
        return _norm(root)
    if not os.path.isabs(target):
        target = os.path.join(root, target)
    return _norm(target)


def granted_shapes(store: Any, project_id: str) -> set[str]:
    """Every action shape a human has approved for this project (AU-08).

    Read once per run: a grant only ever arrives while no run is in flight,
    because granting hands the item back to dispatch.
    """
    shapes: set[str] = set()
    if store is None:
        return shapes
    try:
        for event in store.events(project_id):
            if event.get("kind") != ev.APPROVAL_GRANTED:
                continue
            shape = (event.get("data") or {}).get("shape")
            if shape:
                shapes.add(str(shape))
    except Exception:  # noqa: BLE001 - unreadable history grants nothing
        return set()
    return shapes


def requested_shape(store: Any, project_id: str, item_id: str) -> str | None:
    """The shape of the question a member asked about this item, if any.

    ``resolve_approval`` carries this onto the grant event, which is what lets
    ``granted_shapes`` recognise the action when the item is re-dispatched —
    without it the member would ask the same question forever.
    """
    if store is None:
        return None
    try:
        events = store.events(project_id)
    except Exception:  # noqa: BLE001
        return None
    for event in reversed(events):
        if event.get("item_id") != item_id:
            continue
        if event.get("kind") != ev.APPROVAL_REQUESTED:
            continue
        shape = (event.get("data") or {}).get("shape")
        if shape:
            return str(shape)
    return None


def requested_write_key(store: Any, project_id: str, item_id: str) -> str | None:
    """The write key of the latest question about this item, if it had one.

    Only an *ambiguous* write asks with a key (``write.reserved`` with no
    ``write.done``). Carrying it onto the grant is what lets the ledger see
    that a human explicitly authorized **one** retry, instead of parking the
    same ambiguity forever.
    """
    if store is None:
        return None
    try:
        events = store.events(project_id)
    except Exception:  # noqa: BLE001
        return None
    for event in reversed(events):
        if event.get("item_id") != item_id:
            continue
        if event.get("kind") != ev.APPROVAL_REQUESTED:
            continue
        key = (event.get("data") or {}).get("write_key")
        return str(key) if key else None
    return None


def _action_for(kind: str, target: str, project_root: str):
    """The :class:`policy.Action` behind one call-site request.

    Shared by the gate and the ledger so a grant and a dedupe key can never
    disagree about what the action was.
    """
    from . import policy

    kind = str(kind or "fs")
    if kind == "fs":
        resolved = _resolve(project_root, target)
        outside = not path_is_inside(project_root, resolved)
        dangerous = outside and policy.looks_like_credential(resolved)
    else:
        # Not a path, so there is nothing to resolve and nothing to check
        # against the credential markers. An MCP tool or a shell command
        # reaches outward by definition — `server/tool` and a command root are
        # not "inside" any folder.
        resolved = str(target or "")
        outside = True
        dangerous = False
    return policy.Action(
        kind=kind,
        target=resolved,
        # `_sanitize_path`, the MCP bridge and run_command are not told whether
        # the caller means to read or write, so this assumes the write key.
        # Being asked about too much is the safe direction to be wrong in.
        write=True,
        outside=outside,
        dangerous=dangerous,
    )


def build_gate(store: Any, project_id: str, item_id: str, project_root: str,
               autonomy: dict | None):
    """The project envelope as a callable ``tools/`` can ask.

    ``04`` §5, bullet 4: the envelope arrives as **data**. Nothing under
    ``tools/`` imports this, or anything else in the group package — it calls a
    function that takes two strings and returns a tier name.

    When the answer is ``needs_approval`` the item is parked **here**, before
    the caller raises, so the question exists on the board by the time the
    member's run ends.
    """
    from . import policy

    grants = granted_shapes(store, project_id)

    def gate(kind: str, target: str) -> str | None:
        action = _action_for(kind, target, project_root)
        tier = policy.classify(action, envelope=autonomy, grants=grants)
        if tier == policy.NEEDS_APPROVAL:
            _park(
                store, project_id, item_id, policy.shape_of(action),
                f"wants to write outside the project folder: {action.target}",
            )
        return tier

    return gate


def _park(store: Any, project_id: str, item_id: str, shape: str, detail: str,
          *, write_key: str | None = None) -> None:
    """Record the question. Idempotent — one item, one open question.

    ``write_key`` is only set for the ambiguous case (a reservation with no
    completion). It rides on the grant so the ledger can tell "a human
    explicitly authorized one retry" from "the original approval is still in
    force".
    """
    if store is None:
        return
    try:
        item = store.item(item_id)
        if item is not None and item.get("blocked_reason") == ev.AWAITING_APPROVAL:
            return  # already asked; do not ask again
        data = {"detail": detail, "shape": shape}
        if write_key:
            data["write_key"] = write_key
        store.append(
            project_id, ev.APPROVAL_REQUESTED, "member", item_id=item_id,
            data=data,
        )
    except Exception:  # noqa: BLE001 - a failed park must not allow the action
        return


# ── the idempotency ledger (AU-05) ──────────────────────────────────────────
#
# ``04`` §6. A retried item must not repeat an external effect. The ledger is
# the EVENT LOG, not a table: a write is reserved before it happens, marked
# done after it succeeds, and skipped when a retry finds the key already done.
#
# Only actions the envelope GATED are deduped. An autonomous action is the
# project's own business and repeating it is often correct (re-running a build
# after a fix must not be suppressed). File writes are deliberately excluded:
# writing the same path twice is naturally idempotent, and a key containing a
# resolved path would break portability (``06`` §7).

FRESH = "fresh"
DONE = "done"
UNKNOWN = "unknown"


def payload_sha(payload: Any) -> str:
    """The content identity of one write, independent of attempt or path.

    MCP arguments are canonicalised as sorted-key JSON; a command is its own
    string. The digest is the only place the payload appears, so a command
    containing an absolute path cannot leak one into the key.
    """
    if isinstance(payload, str):
        text = payload
    else:
        text = json.dumps(
            payload, sort_keys=True, separators=(",", ":"), default=str,
        )
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def write_key(item_id: str, shape: str, payload: Any) -> str:
    """``item_id:shape:payload_sha`` — AU-05's deterministic identity.

    ``item_id`` is included on purpose: two items legitimately sending the same
    email are two events, not a duplicate.
    """
    return f"{item_id}:{shape}:{payload_sha(payload)}"


class WriteLedger:
    """The events-as-truth ledger for one item.

    ``shape_for(kind, target)`` returns ``None`` for actions the envelope did
    not gate, and the stable shape otherwise. That is the whole "dedupe only
    what was gated" rule; everything else is a scan of the project's events.
    """

    def __init__(self, store: Any, project_id: str, item_id: str,
                 shape_for: Callable[[str, str], str | None]) -> None:
        self._store = store
        self._project_id = project_id
        self._item_id = item_id
        self._shape_for = shape_for
        self._lock = threading.Lock()

    def _identity(self, kind: str, target: str,
                  payload: Any) -> tuple[str, str] | None:
        shape = self._shape_for(kind, target)
        if not shape:
            return None
        return shape, write_key(self._item_id, shape, payload)

    def _state(self, key: str) -> str:
        done = False
        last_reserved = 0
        last_grant = 0
        for event in self._store.events(self._project_id):
            if event.get("item_id") != self._item_id:
                continue
            kind = event.get("kind")
            data = event.get("data") or {}
            if kind == ev.APPROVAL_GRANTED and data.get("write_key") == key:
                last_grant = int(event.get("seq") or 0)
                continue
            if data.get("key") != key:
                continue
            if kind in (ev.WRITE_DONE, ev.WRITE_SKIPPED):
                done = True
            elif kind == ev.WRITE_RESERVED:
                last_reserved = int(event.get("seq") or 0)
        if done:
            return DONE
        if last_reserved > last_grant:
            # The crash case. Do not guess: the effect may have happened.
            return UNKNOWN
        return FRESH

    def begin(self, kind: str, target: str, payload: Any) -> str | None:
        """Reserve a fresh write, or report ``done`` / ``unknown``.

        ``None`` means the envelope did not gate this action, so the caller
        behaves exactly as it always did.
        """
        identity = self._identity(kind, target, payload)
        if identity is None:
            return None
        shape, key = identity
        with self._lock:
            state = self._state(key)
            if state == FRESH:
                self._store.append(
                    self._project_id, ev.WRITE_RESERVED, "member",
                    item_id=self._item_id, data={"key": key, "shape": shape},
                )
            return state

    def finish(self, kind: str, target: str, payload: Any) -> None:
        identity = self._identity(kind, target, payload)
        if identity is None:
            return
        _shape, key = identity
        with self._lock:
            self._store.append(
                self._project_id, ev.WRITE_DONE, "member",
                item_id=self._item_id, data={"key": key},
            )

    def skipped(self, kind: str, target: str, payload: Any) -> None:
        identity = self._identity(kind, target, payload)
        if identity is None:
            return
        _shape, key = identity
        with self._lock:
            self._store.append(
                self._project_id, ev.WRITE_SKIPPED, "member",
                item_id=self._item_id, data={"key": key},
            )

    def unknown(self, kind: str, target: str, payload: Any) -> None:
        identity = self._identity(kind, target, payload)
        if identity is None:
            return
        shape, key = identity
        _park(
            self._store, self._project_id, self._item_id, shape,
            f"may or may not have been sent: {shape} ({key}). "
            f"Check the destination before approving a retry.",
            write_key=key,
        )


def build_ledger(store: Any, project_id: str, item_id: str, project_root: str,
                 autonomy: dict | None) -> WriteLedger | None:
    """The AU-05 ledger for one item run, or ``None`` when it cannot apply."""
    if store is None or not item_id:
        return None
    from . import policy

    def shape_for(kind: str, target: str) -> str | None:
        action = _action_for(kind, target, project_root)
        if action.kind == "fs":
            # Writing the same path twice is idempotent, and a key would carry
            # a resolved path (``06`` §7). Never dedupe file writes.
            return None
        # Classify WITHOUT the grants: an action is deduped because the
        # envelope gated it, even when an approval has since made it
        # autonomous. An action the envelope made autonomous from the start is
        # not deduped.
        if policy.classify(action, envelope=autonomy, grants=()) != policy.NEEDS_APPROVAL:
            return None
        return policy.shape_of(action)

    return WriteLedger(store, project_id, str(item_id), shape_for)


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
        "## How to finish",
        # The envelope used to be restated here as four bullets. It is enforced
        # now (``04`` §5), so listing it only invited the model to reason about
        # policy instead of acting — and an instruction to call
        # `request_approval` named a tool members do not have, which is worse
        # than saying nothing.
        "- Do the work, then stop. The acceptance criteria above decide whether",
        "  this item is done; nothing you report changes that.",
        "- If a previous attempt paused for the owner's approval, do the same",
        "  thing again — the pause is not a failure, and the answer is recorded.",
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

        The run is **scoped** on ``state_sync`` for the same reason delegation
        scopes its jobs (``delegation.py:622-627``): the agent stack publishes
        token usage into the current state, so without a scope the run both
        pollutes the root state and loses its own cost — which would leave the
        budget ceiling permanently reading zero.
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

        usage: dict = {}
        token = None
        allow_token = None
        gate_token = None
        ledger_token = None
        try:
            from sync.state_sync import active_state, new_scoped_state, safe_get_state
            from tools._runtime import (
                active_allowed_tools,
                active_approval_gate,
                active_write_ledger,
            )

            # The member's materialised allowlist is the real ceiling on its
            # tools (``04`` §5). Until this ran, ``tools/bindings.json`` was
            # written at creation and never opened again — so a project that
            # withheld a tool had that decision ignored at run time, which made
            # the whole `never` tier decorative (D-1, ``06`` §3).
            #
            # Set here, inside the member's own thread, because ContextVars do
            # not cross the boundary `start()` opened.
            from . import materialize as mat
            allow = mat.read_member_allow(ctx.project_root, ctx.member_name)
            if allow is not None:
                allow_token = active_allowed_tools.set(frozenset(allow))

            # And the envelope, so a member that needs to act outside its
            # folder is ASKED rather than refused (``04`` §3-§5).
            gate_token = active_approval_gate.set(build_gate(
                ctx.store, ctx.project_id, str(ctx.item.get("id") or ""),
                ctx.project_root, ctx.autonomy))

            # And the idempotency ledger (AU-05), so a retried item that
            # already performed a gated external write does not repeat it
            # (``04`` §6). It is set here, inside the member's own thread,
            # because ContextVars do not cross the boundary `start()` opened.
            ledger_token = active_write_ledger.set(build_ledger(
                ctx.store, ctx.project_id, str(ctx.item.get("id") or ""),
                ctx.project_root, ctx.autonomy))

            token = active_state.set(new_scoped_state({
                "chat_id": ctx.chat_id,
                "project_path": ctx.project_root,
            }))
            output = node(state) or {}
            # Read usage BEFORE the scope resets, or the numbers are gone.
            usage = dict((safe_get_state() or {}).get("token_usage") or {})
        except Exception as exc:  # noqa: BLE001 - a failed run is a result
            output = {"error": f"{type(exc).__name__}: {exc}"}
        finally:
            if ledger_token is not None:
                try:
                    from tools._runtime import active_write_ledger as _ledger
                    _ledger.reset(ledger_token)
                except Exception:  # noqa: BLE001, S110 - scope cleanup only
                    pass
            if gate_token is not None:
                try:
                    from tools._runtime import active_approval_gate as _gate
                    _gate.reset(gate_token)
                except Exception:  # noqa: BLE001, S110 - scope cleanup only
                    pass
            if allow_token is not None:
                try:
                    from tools._runtime import active_allowed_tools as _allow
                    _allow.reset(allow_token)
                except Exception:  # noqa: BLE001, S110 - scope cleanup only
                    pass
            if token is not None:
                try:
                    from sync.state_sync import active_state as _active
                    _active.reset(token)
                except Exception:  # noqa: BLE001, S110 - scope cleanup only
                    pass

        report = str(output.get("agent_report") or output.get("content") or "")
        tokens_in = int(usage.get("total_input_tokens") or 0)
        tokens_out = int(usage.get("total_output_tokens") or 0)
        return RunResult(
            item_id=ctx.item["id"],
            ok=not output.get("error"),
            report=report,
            error=str(output.get("error") or ""),
            cost_usd=float(usage.get("total_cost") or 0.0),
            cost_tokens=tokens_in + tokens_out,
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
