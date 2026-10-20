"""AU-05 — external writes are idempotent by key.

``docs/group-projects/04-autonomy-and-approvals.md`` section 6.

The failure this prevents is concrete: an item sends an email / opens a PR /
calls an MCP ``create_issue``, then the process dies before the item is marked
done. The lease expires, the item is requeued, and the member runs again. With
``max_attempts`` defaulting to 3, that is three copies of the same outward
action.

The ledger is the project's **event log**, not a table: ``write.reserved``
before the effect, ``write.done`` after it, ``write.skipped`` when a retry
finds the key already done. A reservation with no completion is the crash
case; the retry must ask a human rather than silently duplicate.

Only actions the envelope gated are deduped. Autonomous actions stay
repeatable (re-running a build after a fix is correct), and file writes are
naturally idempotent, so neither is touched.
"""
from __future__ import annotations

import os
import sys
import tempfile
import types
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.dirname(HERE)
if REPO_ROOT not in sys.path:
    sys.path.insert(0, REPO_ROOT)
if HERE not in sys.path:
    sys.path.insert(0, HERE)

import pytest
from _util import agents_shim, real_tools

ENVELOPE = {
    "project_folder": "autonomous",
    "external_read": "autonomous",
    "external_write": "needs_approval",
    "destructive": "blocked",
}

SKIPPED = "already performed; skipped"


@pytest.fixture(autouse=True)
def _real_tools_and_clean_contextvars():
    """The real ``tools`` package, and no ContextVar leaking between tests.

    ``_util.tools_stub()`` replaces ``sys.modules['tools']`` and never restores
    it, so every test that imports the runtime must call ``real_tools()``
    first. The ContextVar reset is the same discipline the runner uses.
    """
    real_tools()
    from tools import _runtime

    gate = _runtime.active_approval_gate.set(None)
    ledger = None
    ledger_var = getattr(_runtime, "active_write_ledger", None)
    if ledger_var is not None:
        ledger = ledger_var.set(None)
    try:
        yield
    finally:
        if ledger is not None:
            ledger_var.reset(ledger)
        _runtime.active_approval_gate.reset(gate)


class Rig:
    """A temp project with one claimed item, the group modules, and a store."""

    def __init__(self, envelope: dict | None = None) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self._shim = agents_shim(
            "agents.group.events", "agents.group.store",
            "agents.group.runner", "agents.group.policy",
            "agents.group.planner", "agents.group.tools",
        )
        modules = self._shim.__enter__()
        self.ev = modules["agents.group.events"]
        self.store_mod = modules["agents.group.store"]
        self.runner = modules["agents.group.runner"]
        self.policy = modules["agents.group.policy"]
        self.envelope = dict(envelope or ENVELOPE)
        self.pid = "idem-rig"
        self.item_id = "m1.i1"
        self.root = os.path.join(self._tmp.name, "project")
        os.makedirs(self.root, exist_ok=True)
        self.store = self.store_mod.ProjectStore(
            os.path.join(self._tmp.name, ".gp", "state.db")
        )
        self.store.append(
            self.pid, self.ev.PROJECT_CREATED, "human",
            data={
                "goal": "prove idempotency",
                "milestones": [{
                    "id": "m1", "title": "m1",
                    "items": [{"id": self.item_id, "title": "do it"}],
                }],
            },
        )
        self.store.append(
            self.pid, self.ev.ITEM_CLAIMED, "supervisor", item_id=self.item_id,
            data={"lease_owner": "member", "lease_expires": 9999999999},
        )

    def grant(self, shape: str, *, write_key: str | None = None) -> None:
        data = {"shape": shape}
        if write_key:
            data["write_key"] = write_key
        self.store.append(
            self.pid, self.ev.APPROVAL_GRANTED, "human",
            item_id=self.item_id, data=data,
        )

    def events(self, kind: str | None = None) -> list[dict]:
        events = self.store.events(self.pid)
        if kind is None:
            return events
        return [event for event in events if event["kind"] == kind]

    def shape(self, kind: str, target: str) -> str:
        return self.policy.shape_of(self.policy.Action(
            kind=kind, target=target, write=True, outside=True,
        ))

    def install(self, *, gate: bool = True, ledger: bool = True):
        from tools import _runtime

        gate_fn = None
        if gate:
            gate_fn = self.runner.build_gate(
                self.store, self.pid, self.item_id, self.root, self.envelope,
            )
        ledger_obj = None
        if ledger:
            ledger_obj = self.runner.build_ledger(
                self.store, self.pid, self.item_id, self.root, self.envelope,
            )
        gate_token = _runtime.active_approval_gate.set(gate_fn)
        ledger_token = _runtime.active_write_ledger.set(ledger_obj)
        return _runtime, gate_token, ledger_token

    def close(self) -> None:
        try:
            self.store.close()
        finally:
            self._shim.__exit__(None, None, None)
            self._tmp.cleanup()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()
        return False


def _reset(runtime, gate_token, ledger_token) -> None:
    runtime.active_write_ledger.reset(ledger_token)
    runtime.active_approval_gate.reset(gate_token)


class _FakeMcpClient:
    """Enough of ``_McpClient`` for ``McpManager.call``; no transport."""

    def __init__(self, calls: list, result: str = "issue created") -> None:
        self.cfg = {"approval": "prompt"}
        self.status = "connected"
        self.error = ""
        self._calls = calls
        self._result = result

    def call_tool(self, tool: str, args: dict, timeout) -> str:
        self._calls.append((tool, dict(args)))
        return self._result


def _mcp_manager(calls: list):
    from tools.mcp import mcp_bridge

    manager = mcp_bridge.McpManager()
    manager._clients["fake"] = _FakeMcpClient(calls)
    return mcp_bridge, manager


def _write_kinds(rig: Rig) -> list[str]:
    return [kind for kind in (event["kind"] for event in rig.events())
            if kind.startswith("write.")]


# ════════════════════════════════════════════════════════════════════════════
#  1. The dedupe — a retried gated action is skipped
# ════════════════════════════════════════════════════════════════════════════


def test_a_retried_gated_mcp_action_is_skipped():
    """The AU-05 contract, at the real MCP call site.

    The first attempt performs the effect and records it. The second attempt
    must return "already performed; skipped" and leave the effect count at one.
    """
    with Rig() as rig:
        shape = rig.shape("mcp", "fake/create_issue")
        rig.grant(shape)
        runtime, gate_token, ledger_token = rig.install()
        calls: list = []
        manager = _mcp_manager(calls)[1]
        try:
            first = manager.call(
                "mcp__fake__create_issue", {"title": "the same issue"})
            second = manager.call(
                "mcp__fake__create_issue", {"title": "the same issue"})
        finally:
            _reset(runtime, gate_token, ledger_token)

        assert first == "issue created"
        assert second == SKIPPED
        assert len(calls) == 1, f"the effect ran {len(calls)} times"

        kinds = [event["kind"] for event in rig.events()]
        assert rig.ev.WRITE_RESERVED in kinds
        assert rig.ev.WRITE_DONE in kinds
        assert rig.ev.WRITE_SKIPPED in kinds


def test_a_retried_gated_command_is_skipped():
    """The same dedupe at the shell call site.

    ``runCommand`` was not gated before AU-05, so this also proves the gate and
    the ledger were wired into the non-read-only command path.
    """
    with Rig() as rig:
        from tools.MainTool import runCommand

        rig.grant(rig.shape("command", "gh"))
        runtime, gate_token, ledger_token = rig.install()
        calls: list[str] = []

        def fake_execute(self, command, **kwargs):
            calls.append(command)
            return "[OK] Exit 0"

        ctx = types.SimpleNamespace(project_path=rig.root)
        try:
            with mock.patch.object(
                runCommand.LocalShellBackend, "execute", fake_execute,
            ):
                first = runCommand.run_command(
                    ctx=ctx, command="gh pr create --title the-same-pr")
                second = runCommand.run_command(
                    ctx=ctx, command="gh pr create --title the-same-pr")
        finally:
            _reset(runtime, gate_token, ledger_token)

        assert first.startswith("[OK]"), first
        assert second == SKIPPED
        assert calls == ["gh pr create --title the-same-pr"]
        assert rig.ev.WRITE_SKIPPED in [event["kind"] for event in rig.events()]


# ════════════════════════════════════════════════════════════════════════════
#  2. The crash case — reserved without done parks, never silently repeats
# ════════════════════════════════════════════════════════════════════════════


def test_a_crash_between_reserve_and_done_parks_for_a_human():
    """A reservation with no completion is genuine ambiguity.

    The effect may have happened before the process died, so retrying silently
    is the one thing not allowed. The retry parks with a detail a human can act
    on, and the effect is not called again.
    """
    with Rig() as rig:
        from tools import _runtime

        shape = rig.shape("mcp", "fake/create_issue")
        payload = {"title": "the same issue"}
        key = rig.runner.write_key(rig.item_id, shape, payload)
        _runtime_obj = _runtime
        ledger = rig.runner.build_ledger(
            rig.store, rig.pid, rig.item_id, rig.root, rig.envelope,
        )
        calls: list[str] = []

        def dies_after_the_call():
            calls.append("attempt")
            raise RuntimeError("process died after the external call")

        ledger_token = _runtime_obj.active_write_ledger.set(ledger)
        try:
            with pytest.raises(RuntimeError):
                _runtime_obj._perform_write(
                    "mcp", "fake/create_issue", payload, dies_after_the_call)

            with pytest.raises(_runtime_obj.ApprovalRequired):
                _runtime_obj._perform_write(
                    "mcp", "fake/create_issue", payload,
                    lambda: calls.append("must not happen"),
                )
        finally:
            _runtime_obj.active_write_ledger.reset(ledger_token)

        assert calls == ["attempt"], "the ambiguous write was repeated"
        asked = rig.events(rig.ev.APPROVAL_REQUESTED)[-1]
        assert "may or may not have been sent" in asked["data"]["detail"]
        assert asked["data"].get("write_key") == key
        assert rig.store.item(rig.item_id)["blocked_reason"] == rig.ev.AWAITING_APPROVAL

        # An explicit human grant after the reservation authorizes ONE retry.
        # Go through the real resolve_approval path: it must carry the key.
        from agents.group.tools import GroupToolkit

        toolkit = GroupToolkit(lambda _pid: (rig.store, None))
        resolved = toolkit.resolve_approval(rig.pid, item_id=rig.item_id, grant=True)
        assert resolved.ok, resolved.message
        grant = rig.events(rig.ev.APPROVAL_GRANTED)[-1]
        assert grant["data"].get("write_key") == key, (
            "the grant did not carry the write key, so the ledger would park forever"
        )

        ledger_token = _runtime_obj.active_write_ledger.set(ledger)
        try:
            result = _runtime_obj._perform_write(
                "mcp", "fake/create_issue", payload,
                lambda: calls.append("retry") or "sent",
            )
        finally:
            _runtime_obj.active_write_ledger.reset(ledger_token)

        assert result == "sent"
        assert calls == ["attempt", "retry"]
        assert rig.events(rig.ev.WRITE_DONE), "the retry was not recorded as done"


# ════════════════════════════════════════════════════════════════════════════
#  3. Autonomous actions are NOT deduped
# ════════════════════════════════════════════════════════════════════════════


def test_an_autonomous_action_is_not_deduped():
    """Re-running a build after a fix is correct, not a duplicate.

    The envelope says external writes are autonomous, so the ledger must stay
    out of the way: two identical calls, two effects, no write events.
    """
    with Rig({**ENVELOPE, "external_write": "autonomous"}) as rig:
        runtime, gate_token, ledger_token = rig.install()
        calls: list = []
        _mcp_bridge, manager = _mcp_manager(calls)
        try:
            manager.call("mcp__fake__create_issue", {"title": "same"})
            manager.call("mcp__fake__create_issue", {"title": "same"})
        finally:
            _reset(runtime, gate_token, ledger_token)

        assert len(calls) == 2, "an autonomous action was suppressed"
        assert _write_kinds(rig) == [], "an autonomous action wrote a ledger entry"


# ════════════════════════════════════════════════════════════════════════════
#  4. File writes are NOT deduped
# ════════════════════════════════════════════════════════════════════════════


def test_a_file_write_is_not_deduped():
    """Writing the same path twice is naturally idempotent.

    A ledger key would have to contain the resolved path (which breaks
    portability), and suppressing the second write would be surprising. So file
    writes must not consult the ledger at all.
    """
    with Rig() as rig:
        from tools import _runtime
        from tools.MainTool import editFile

        class ExplodingLedger:
            def __getattr__(self, name):
                raise AssertionError(f"a file write consulted the ledger: {name}")

        target = os.path.join(rig.root, "context", "artifacts", "note.txt")
        ctx = types.SimpleNamespace(project_path=rig.root)
        token = _runtime.active_write_ledger.set(ExplodingLedger())
        try:
            first = editFile._write_file(target, "one", ctx)
            second = editFile._write_file(target, "two", ctx)
        finally:
            _runtime.active_write_ledger.reset(token)

        assert first.startswith("[OK]"), first
        assert second.startswith("[OK]"), second
        with open(target, encoding="utf-8") as handle:
            assert handle.read() == "two"
        assert _write_kinds(rig) == []


# ════════════════════════════════════════════════════════════════════════════
#  5. No ledger in force — the call sites are exactly as they were
# ════════════════════════════════════════════════════════════════════════════


def test_without_a_ledger_mcp_still_uses_the_interactive_confirm():
    """Every ordinary agent's path. It must not have moved.

    With no gate and no ledger, ``_gate_mcp_call`` still asks through the
    running chat and the call still executes; nothing is recorded.
    """
    with Rig() as rig:
        calls: list = []
        confirmed: list = []
        mcp_bridge, manager = _mcp_manager(calls)

        with mock.patch.object(
            mcp_bridge, "_confirm",
            lambda server, tool, args: confirmed.append((server, tool)) or True,
        ):
            result = manager.call(
                "mcp__fake__create_issue", {"title": "ordinary agent"})

        assert result == "issue created"
        assert confirmed == [("fake", "create_issue")]
        assert len(calls) == 1
        assert _write_kinds(rig) == []


def test_without_a_ledger_run_command_is_unchanged():
    """The shell path with no group project in force.

    A normal command executes; a destructive one is still refused before any
    backend or ledger is touched.
    """
    with Rig() as rig:
        from tools.MainTool import runCommand

        calls: list[str] = []

        def fake_execute(self, command, **kwargs):
            calls.append(command)
            return "[OK] Exit 0"

        ctx = types.SimpleNamespace(project_path=rig.root)
        with mock.patch.object(
            runCommand.LocalShellBackend, "execute", fake_execute,
        ):
            result = runCommand.run_command(
                ctx=ctx, command="gh pr create --title ordinary")

            with pytest.raises(Exception) as refused:
                runCommand.run_command(ctx=ctx, command="rm -rf /")

        assert result.startswith("[OK]"), result
        assert calls == ["gh pr create --title ordinary"]
        assert "blocked for safety" in str(refused.value)
        assert _write_kinds(rig) == []


# ════════════════════════════════════════════════════════════════════════════
#  6. Replay — write.* must not raise UnknownEventKind
# ════════════════════════════════════════════════════════════════════════════


def test_write_events_replay_without_unknown_event_kind():
    """D-13 is frozen, so AU-05's three kinds are an explicit amendment.

    A log containing them must rebuild like any other log. If the kinds are
    added to ``ALL_KINDS`` but not ``REDUCED_KINDS`` (or the reducer lacks an
    arm), this is the test that fails.
    """
    with Rig() as rig:
        for kind in (rig.ev.WRITE_RESERVED, rig.ev.WRITE_DONE,
                     rig.ev.WRITE_SKIPPED):
            rig.store.append(
                rig.pid, kind, "member", item_id=rig.item_id,
                data={"key": "m1.i1:mcp:write:fake/create_issue:abc"},
            )

        rig.store.rebuild(rig.pid)

        kinds = [event["kind"] for event in rig.events()]
        for kind in (rig.ev.WRITE_RESERVED, rig.ev.WRITE_DONE,
                     rig.ev.WRITE_SKIPPED):
            assert kind in kinds
            assert kind in rig.ev.ALL_KINDS
            assert kind in rig.ev.REDUCED_KINDS


# ════════════════════════════════════════════════════════════════════════════
#  7. Portability — no path in a key or a command shape
# ════════════════════════════════════════════════════════════════════════════


def test_a_command_key_and_shape_are_path_free():
    """``check_portability()`` does not scan ``.gp/``, so a path here would
    slip in silently. The command string lives only inside the hash; the shape
    is the command roots."""
    with Rig() as rig:
        from tools.MainTool import runCommand
        from tools.MainTool._cmdpolicy import check_command

        command = (
            f'gh pr create --title idempotency '
            f'--body "see {rig.root}\\notes.txt"'
        )
        decision = check_command(command)
        assert decision.allowed, decision.reason
        target = runCommand._command_target(decision)
        key = rig.runner.write_key(
            rig.item_id, f"command:write:{target}", command)

        assert target == "gh"
        assert rig.root not in target
        assert rig.root not in key
        assert "notes.txt" not in key
        assert os.path.abspath(rig.root) not in key


def test_a_destructive_command_is_refused_before_the_ledger():
    """``04`` §4.1: destructive ops are NEVER automated.

    Even with a ledger in force, ``check_command`` runs first and the backend
    is never reached.
    """
    with Rig() as rig:
        from tools import _runtime
        from tools.MainTool import runCommand

        class ExplodingLedger:
            def __getattr__(self, name):
                raise AssertionError(f"the ledger ran before check_command: {name}")

        calls: list[str] = []

        def fake_execute(self, command, **kwargs):
            calls.append(command)
            return "[OK] should not run"

        ctx = types.SimpleNamespace(project_path=rig.root)
        token = _runtime.active_write_ledger.set(ExplodingLedger())
        try:
            with mock.patch.object(
                runCommand.LocalShellBackend, "execute", fake_execute,
            ), pytest.raises(Exception) as refused:
                runCommand.run_command(ctx=ctx, command="rm -rf /")
        finally:
            _runtime.active_write_ledger.reset(token)

        assert "blocked for safety" in str(refused.value)
        assert calls == []
        assert _write_kinds(rig) == []
