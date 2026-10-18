"""Phase 5 tests — the autonomy envelope, enforced.

``docs/group-projects/04-autonomy-and-approvals.md``.

The envelope used to be a paragraph in the member's prompt: something the model
was asked to respect. These tests are about the difference between that and the
envelope deciding — and about the difference between **refused** and **asked**,
which is the whole feature. Before this, a member that needed to act outside
its folder got an error and there was no path to consent at all.

Two layers, tested separately because they fail differently:

- :mod:`agents.group.policy` is pure. If the rules are wrong, they are wrong
  here, in a test that takes milliseconds.
- the gate in ``tools/_runtime.py`` is the wiring. If it is wrong, the rules
  never run — which is exactly how ``tools/bindings.json`` sat unread for four
  phases while its docstring claimed the opposite.
"""
from __future__ import annotations

import os
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.dirname(HERE)
if REPO_ROOT not in sys.path:
    sys.path.insert(0, REPO_ROOT)
if HERE not in sys.path:
    sys.path.insert(0, HERE)

from _util import agents_shim

ENVELOPE = {
    "project_folder": "autonomous",
    "external_read": "autonomous",
    "external_write": "needs_approval",
    "destructive": "blocked",
}


def _policy():
    """Load the pure module WITHOUT executing ``agents/__init__.py``.

    ``policy`` imports nothing but ``dataclasses``, so pulling the whole agent
    stack to test it costs ~45s and proves nothing. Same reason every other
    suite here uses the shim.
    """
    with agents_shim("agents.group.policy"):
        from agents.group import policy
        return policy


def _action(outside=True, *, write=True, dangerous=False, target="X"):
    P = _policy()
    return P.Action(kind="fs", target=target, write=write,
                    outside=outside, dangerous=dangerous)


# ════════════════════════════════════════════════════════════════════════════
#  The rules — pure
# ════════════════════════════════════════════════════════════════════════════


def test_inside_the_project_folder_is_the_free_zone():
    P = _policy()
    assert P.classify(_action(outside=False), envelope=ENVELOPE) == P.AUTONOMOUS
    # Even when the envelope is empty: the folder is the project's own business.
    assert P.classify(_action(outside=False), envelope={}) == P.AUTONOMOUS


def test_outside_follows_the_envelope():
    P = _policy()
    assert P.classify(_action(), envelope=ENVELOPE) == P.NEEDS_APPROVAL
    assert P.classify(_action(), envelope={
        **ENVELOPE, "external_write": "autonomous"}) == P.AUTONOMOUS
    assert P.classify(_action(), envelope={
        **ENVELOPE, "external_write": "blocked"}) == P.BLOCKED


def test_an_unnamed_action_is_default_deny():
    """AU-01. An enumerated allowlist is a list of things someone thought of;
    default-deny catches the ones they did not."""
    P = _policy()
    assert P.classify(_action(), envelope={}) == P.NEEDS_APPROVAL


def test_a_typo_in_the_envelope_is_not_a_grant():
    """A misspelled tier must not read as permission."""
    P = _policy()
    assert P.classify(_action(), envelope={
        "external_write": "need_approval"}) == P.NEEDS_APPROVAL
    assert P.classify(_action(outside=False), envelope={
        "project_folder": "autonomus"}) == P.NEEDS_APPROVAL
    assert P.classify(_action(), envelope={
        "external_write": "AUTONOMOUS "}) == P.AUTONOMOUS


def test_an_approved_shape_is_not_gated_again():
    """AU-08 / §4.1. The FIRST outward action of a shape is checked — that is
    what catches "it emailed the wrong vendor" before it emails forty more."""
    P = _policy()
    action = _action()
    assert P.classify(action, envelope=ENVELOPE) == P.NEEDS_APPROVAL
    assert P.classify(action, envelope=ENVELOPE,
                      grants={P.shape_of(action)}) == P.AUTONOMOUS
    # A different target is a different shape and is still asked about.
    other = _action(target="Y")
    assert P.classify(other, envelope=ENVELOPE,
                      grants={P.shape_of(action)}) == P.NEEDS_APPROVAL


def test_a_grant_cannot_lift_a_blocked_action():
    """Only `needs_approval` is lifted. Approving something forbidden enough
    times does not make it allowed."""
    P = _policy()
    action = _action()
    env = {**ENVELOPE, "external_write": "blocked"}
    assert P.classify(action, envelope=env,
                      grants={P.shape_of(action)}) == P.BLOCKED


def test_credentials_are_blocked_at_every_tier():
    """AU-03, and it outranks both the envelope and any grant."""
    P = _policy()
    secret = _action(dangerous=True, target="/home/me/.ssh/id_rsa")
    permissive = {**ENVELOPE, "external_write": "autonomous"}
    assert P.classify(secret, envelope=permissive) == P.BLOCKED
    assert P.classify(secret, envelope=permissive,
                      grants={P.shape_of(secret)}) == P.BLOCKED


def test_a_projects_own_dot_env_is_not_a_credential():
    """Blocking it would make normal work impossible while protecting nobody:
    the folder is the unit of ownership, so its .env is the project's own."""
    P = _policy()
    assert P.looks_like_credential("/other/place/.env") is True
    assert P.looks_like_credential("/other/place/.env.local") is True
    assert P.looks_like_credential("/other/place/.environment") is False
    assert P.classify(_action(outside=False, target=".env"),
                      envelope=ENVELOPE) == P.AUTONOMOUS


def test_shape_is_stable_and_readable():
    """A shape is what a grant is remembered by, so it must not drift."""
    P = _policy()
    a = P.Action(kind="fs", target=r"D:\out\x.md", write=True, outside=True)
    assert P.shape_of(a) == "fs:write:D:\\out\\x.md"
    assert P.shape_of(a) == P.shape_of(a)
    b = P.Action(kind="fs", target=r"D:\out\x.md", write=False, outside=True)
    assert P.shape_of(b) != P.shape_of(a), "a read is not the same as a write"


# ════════════════════════════════════════════════════════════════════════════
#  The gate — wiring, and the reason it has to be tested at all
# ════════════════════════════════════════════════════════════════════════════


class Rig:
    """A temp project with one member, plus the real tools package."""

    def __init__(self):
        self._tmp = tempfile.TemporaryDirectory()
        self._shim = agents_shim(
            "agents.group.events", "agents.group.store", "agents.group.supervisor",
            "agents.group.manifest", "agents.group.criteria",
            "agents.group.materialize", "agents.group.project",
            "agents.group.runner", "agents.group.planner", "agents.group.tools",
            "agents.group.policy")
        self._shim.__enter__()
        self.root = os.path.join(self._tmp.name, "projects")
        os.environ["STARK_PROJECTS_ROOT"] = self.root
        from server.store import group_registry
        self.registry = group_registry
        self.registry.set_projects_root(self.root)

        from _util import real_tools
        self.tools = real_tools()
        out = self.tools.create_group_project(
            goal="produce a tiny report",
            milestones=[{
                "id": "m1", "title": "Write it",
                "items": [{
                    "id": "m1.i1", "title": "write it", "detail": "write it",
                    "acceptance": [{"kind": "check", "check": "file_exists",
                                    "path": "context/artifacts/hello.txt"}],
                }],
            }],
            project_id="envelope-rig")
        assert out.startswith("[OK]"), out

    @property
    def project_id(self) -> str:
        return "envelope-rig"

    @property
    def project_root(self) -> str:
        return self.registry.project_paths(self.project_id).project_dir

    def store(self):
        return self.registry.open_store(self.project_id)

    def outside_path(self) -> str:
        return os.path.join(tempfile.gettempdir(), "gp-outside-test", "x.md")

    def close(self):
        try:
            self.registry.invalidate()
            self.registry.set_projects_root(None)
        finally:
            self._shim.__exit__(None, None, None)
            self._tmp.cleanup()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()
        return False


def _with_gate(rig):
    """Install the project's gate the way the runner does."""
    from agents.group.runner import build_gate
    from tools._runtime import active_approval_gate
    gate = build_gate(rig.store(), rig.project_id, "m1.i1",
                      rig.project_root, ENVELOPE)
    return active_approval_gate.set(gate)


def test_without_a_gate_an_outside_path_is_still_refused():
    """The behaviour every non-group agent depends on. Must not have moved."""
    from tools._runtime import _sanitize_path

    path = os.path.join(tempfile.gettempdir(), "gp-outside-test", "x.md")
    try:
        _sanitize_path(path)
    except Exception as exc:  # noqa: BLE001
        assert type(exc).__name__ in ("ValueError", "PermissionError"), (
            f"an outside path with no gate raised {type(exc).__name__}"
        )
    else:
        raise AssertionError("an outside path was allowed with no gate in force")


def test_an_outside_path_is_asked_about_not_refused():
    """The feature. Before this the member got a ValueError and the human was
    never told there was anything to decide."""
    from tools._runtime import ApprovalRequired, _sanitize_path, active_approval_gate

    with Rig() as rig:
        token = _with_gate(rig)
        try:
            try:
                _sanitize_path(rig.outside_path())
            except ApprovalRequired as exc:
                assert "outside the project folder" in str(exc)
            else:
                raise AssertionError("an outside write was allowed without asking")
        finally:
            active_approval_gate.reset(token)

        # And the question is on the board, not just in an exception.
        item = rig.store().item("m1.i1")
        assert item["blocked_reason"] == "awaiting_approval", item
        asked = [e for e in rig.store().events(rig.project_id)
                 if e["kind"] == "approval.requested"]
        assert asked, "the park was never recorded"
        assert asked[-1]["data"].get("shape"), "the question has no shape"


def test_a_path_inside_the_project_folder_is_never_asked_about():
    from tools._runtime import _sanitize_path, active_approval_gate

    with Rig() as rig:
        token = _with_gate(rig)
        try:
            inside = os.path.join(rig.project_root, "context", "artifacts", "x.md")
            assert _sanitize_path(inside)
        finally:
            active_approval_gate.reset(token)


def test_the_full_loop_refused_then_asked_then_allowed():
    """The point of the whole phase, end to end.

    An outside write is refused while nobody has been asked. It parks. The
    human approves. The same write then proceeds — which is only possible
    because the grant carries the action's SHAPE, so the re-dispatched member
    is told yes instead of asking the same question forever.
    """
    from tools._runtime import ApprovalRequired, _sanitize_path, active_approval_gate

    with Rig() as rig:
        outside = rig.outside_path()

        # ── 1. asked, and parked ──
        token = _with_gate(rig)
        try:
            try:
                _sanitize_path(outside)
            except ApprovalRequired:
                pass
            else:
                raise AssertionError("not gated")
        finally:
            active_approval_gate.reset(token)

        # ── 2. the human answers ──
        result = rig.registry.toolkit().resolve_approval(
            rig.project_id, item_id="m1.i1", grant=True)
        assert result.ok, result.message

        grant = [e for e in rig.store().events(rig.project_id)
                 if e["kind"] == "approval.granted"][-1]
        assert grant["data"].get("shape"), (
            "the grant did not carry a shape, so the member would ask again"
        )

        # ── 3. the same action now proceeds ──
        token = _with_gate(rig)          # rebuilt, as a re-dispatch would
        try:
            assert _sanitize_path(outside) == os.path.normpath(outside), (
                "the approved action was gated a second time"
            )
        finally:
            active_approval_gate.reset(token)


def test_the_runner_installs_the_gate():
    """The wiring, which is the part that keeps failing in this codebase.

    Everything above drives the guards with a gate installed by hand. This
    drives a real member run through ``LocalRunner._execute_node`` and lets the
    fake runtime find out what a member would actually experience. A gate that
    is correct but never installed is ``tools/bindings.json`` all over again.
    """
    import types as _types

    with Rig() as rig:
        outside = rig.outside_path()
        probe = _types.ModuleType("gp_gate_probe")

        def run(_state):
            from tools._runtime import ApprovalRequired, _sanitize_path
            try:
                _sanitize_path(outside)
            except ApprovalRequired:
                return {"agent_report": "gated"}
            except Exception as exc:  # noqa: BLE001
                return {"agent_report": f"other:{type(exc).__name__}"}
            return {"agent_report": "allowed"}

        probe.run = run
        sys.modules["gp_gate_probe"] = probe
        try:
            from agents.group.runner import LocalRunner, RunContext
            result = LocalRunner()._execute_node(RunContext(
                project_id=rig.project_id,
                project_root=rig.project_root,
                item={"id": "m1.i1", "title": "t"},
                member={"runtime": "gp_gate_probe:run", "derived_from": "general"},
                member_name="researcher", chat_id="c", autonomy=ENVELOPE,
                store=rig.store(),
            ))
        finally:
            sys.modules.pop("gp_gate_probe", None)

        assert result.report == "gated", (
            f"a member run saw {result.report!r} for an outside write"
        )
        # And the question reached the board, not just the exception.
        assert rig.store().item("m1.i1")["blocked_reason"] == "awaiting_approval"


def test_the_runner_does_not_gate_a_write_inside_the_folder():
    """The same probe, pointed at the project's own folder."""
    import types as _types

    with Rig() as rig:
        inside = os.path.join(rig.project_root, "context", "artifacts", "x.md")
        probe = _types.ModuleType("gp_gate_probe_in")

        def run(_state):
            from tools._runtime import ApprovalRequired, _sanitize_path
            try:
                _sanitize_path(inside)
            except ApprovalRequired:
                return {"agent_report": "gated"}
            except Exception as exc:  # noqa: BLE001
                return {"agent_report": f"other:{type(exc).__name__}"}
            return {"agent_report": "allowed"}

        probe.run = run
        sys.modules["gp_gate_probe_in"] = probe
        try:
            from agents.group.runner import LocalRunner, RunContext
            result = LocalRunner()._execute_node(RunContext(
                project_id=rig.project_id,
                project_root=rig.project_root,
                item={"id": "m1.i1", "title": "t"},
                member={"runtime": "gp_gate_probe_in:run", "derived_from": "general"},
                member_name="researcher", chat_id="c", autonomy=ENVELOPE,
                store=rig.store(),
            ))
        finally:
            sys.modules.pop("gp_gate_probe_in", None)

        assert result.report == "allowed", (
            f"a member's own folder was gated: {result.report!r}"
        )


def test_the_approval_hook_gates_paths_inside_other_allowed_roots():
    """A second site guarding a different set of paths.

    ``_sanitize_path`` gates paths outside EVERY allowed root. This hook gates
    the paths that are inside a root but are not the project's — scratch,
    memories, the harness's own tree. Without it, "outside the project folder"
    would only mean "outside every folder the harness trusts".
    """
    from tools._runtime import (
        _HARNESS_ROOT,
        ApprovalRequired,
        _check_and_request_approval,
        active_approval_gate,
    )

    with Rig() as rig:
        target = os.path.join(_HARNESS_ROOT, "some_notes.md")
        token = _with_gate(rig)
        try:
            try:
                _check_and_request_approval(target, "edit")
            except ApprovalRequired:
                pass
            else:
                raise AssertionError(
                    "a path outside the project but inside an allowed root "
                    "was not gated"
                )
        finally:
            active_approval_gate.reset(token)


def test_the_approval_hook_allows_the_projects_own_files():
    from tools._runtime import _check_and_request_approval, active_approval_gate

    with Rig() as rig:
        target = os.path.join(rig.project_root, "context", "notes.md")
        token = _with_gate(rig)
        try:
            _check_and_request_approval(target, "edit")   # must not raise
        finally:
            active_approval_gate.reset(token)


def _drive(sup, project_id: str, predicate, *, ticks: int = 80) -> bool:
    """Tick until ``predicate`` holds.

    ``LocalRunner`` runs each member on a thread, so a tick returns *before*
    the member has finished. Every test above drives the guards directly and so
    never noticed; this is the seam the full-loop test exists to find.
    """
    for _ in range(ticks):
        sup.tick(project_id)
        if predicate():
            return True
        time.sleep(0.05)
    return False


def _supervisor_with(rig, runtime: str):
    """A real LocalRunner over one member, so the runner's gates are installed."""
    from agents.group.runner import LocalRunner
    from agents.group.supervisor import ProjectContext
    from server.services import group_scheduler

    context = ProjectContext(
        project_id=rig.project_id, root=rig.project_root,
        members={"researcher": {"name": "researcher", "runtime": runtime}},
        autonomy=ENVELOPE, manifest_members=[],
    )
    return group_scheduler.build_supervisor(
        runner=LocalRunner(), context_for=lambda _p: context)


def test_a_retry_gets_a_real_second_attempt():
    """The stale-outcome bug, with no approval involved.

    `_harvest` runs in the SAME tick as dispatch, so a re-claimed item still
    carrying its previous attempt's ``error`` was settled as failed before the
    member had run — and the retry ladder burned through every attempt without
    ever actually retrying.
    """
    import types as _types

    with Rig() as rig:
        probe = _types.ModuleType("gp_retry_probe")
        calls = {"n": 0}

        def run(_state):
            calls["n"] += 1
            if calls["n"] == 1:
                return {"error": "first attempt fails on purpose"}
            target = os.path.join(rig.project_root, "context", "artifacts")
            os.makedirs(target, exist_ok=True)
            with open(os.path.join(target, "hello.txt"), "w", encoding="utf-8") as fh:
                fh.write("hello\n")
            return {"agent_report": "second attempt works"}

        probe.run = run
        sys.modules["gp_retry_probe"] = probe
        try:
            sup = _supervisor_with(rig, "gp_retry_probe:run")
            store = rig.store()
            done = _drive(sup, rig.project_id,
                          lambda: store.project(rig.project_id)["status"] == "done")
            item = store.item("m1.i1")
            assert done and item["status"] == "done", (
                f"the retry never ran: status={item['status']!r} "
                f"attempts={item['attempts']} error={str(item.get('error'))[:80]!r}"
            )
            assert calls["n"] >= 2, "the member was never retried for real"
        finally:
            sys.modules.pop("gp_retry_probe", None)


def test_the_whole_loop_through_the_supervisor():
    """Refused → asked → approved → done, driven by the real supervisor.

    Everything else in this file tests one layer. This is the only test that
    runs the sequence an owner actually experiences, and it is the one that can
    catch a seam between them — which is how a feature that "has tests" still
    does not work.
    """
    import types as _types

    with Rig() as rig:
        outside = rig.outside_path()
        probe = _types.ModuleType("gp_loop_probe")
        calls = {"n": 0}

        def run(_state):
            from tools._runtime import _sanitize_path
            calls["n"] += 1
            # Raises on the first run only; after the grant it returns.
            _sanitize_path(outside)
            target = os.path.join(rig.project_root, "context", "artifacts")
            os.makedirs(target, exist_ok=True)
            with open(os.path.join(target, "hello.txt"), "w", encoding="utf-8") as fh:
                fh.write("hello\n")
            return {"agent_report": "wrote it"}

        probe.run = run
        sys.modules["gp_loop_probe"] = probe
        try:
            sup = _supervisor_with(rig, "gp_loop_probe:run")
            store = rig.store()

            # ── 1. the member hits the boundary, and is ASKED ──
            assert _drive(sup, rig.project_id,
                          lambda: store.item("m1.i1")["blocked_reason"]
                          == "awaiting_approval"), (
                f"never parked: {store.item('m1.i1')}"
            )
            assert store.project(rig.project_id)["status"] == "running"

            # ── 2. the owner approves ──
            result = rig.registry.toolkit().resolve_approval(
                rig.project_id, item_id="m1.i1", grant=True)
            assert result.ok, result.message

            # ── 3. the work completes ──
            done = _drive(sup, rig.project_id,
                          lambda: store.project(rig.project_id)["status"] == "done")
            item = store.item("m1.i1")
            assert done and item["status"] == "done", (
                f"after approval the item ended {item['status']!r} "
                f"(blocked={item.get('blocked_reason')!r}, "
                f"error={str(item.get('error'))[:120]!r})"
            )
            assert calls["n"] >= 2, "the member never got a second run"
        finally:
            sys.modules.pop("gp_loop_probe", None)


def test_a_credential_path_is_refused_even_when_approved():
    """AU-03 at the wiring level, not just in the policy."""
    from tools._runtime import _sanitize_path, active_approval_gate

    with Rig() as rig:
        secret = os.path.join(tempfile.gettempdir(), "gp-outside-test",
                              ".ssh", "id_rsa")

        token = _with_gate(rig)
        try:
            try:
                _sanitize_path(secret)
            except PermissionError:
                pass
            except Exception as exc:
                raise AssertionError(
                    f"a credential path raised {type(exc).__name__}, "
                    f"not a refusal"
                ) from exc
            else:
                raise AssertionError("a credential path was allowed")
        finally:
            active_approval_gate.reset(token)

        # Nothing was asked, because there is nothing to decide.
        item = rig.store().item("m1.i1")
        assert item["blocked_reason"] is None, (
            "a credential path parked the item instead of refusing it"
        )
