"""Phase 4 surface tests — workspace scoping, the approvals grant, the router.

Two things are asserted here that no earlier suite could:

1. **A project's origin is recorded in the HARNESS, not the folder.** The
   portability contract (``01`` §14) requires a folder to load and resume at
   any root, and ``06`` §7 names "an absolute path sneaks in" as a failure
   mode. So the test that matters is the negative one: assert the folder
   does not contain the workspace reference.

2. **A parked item can be answered.** ``request_approval`` shipped with every
   reducer for the answer already written and nothing that emitted either —
   and the stall detector exempts ``AWAITING_APPROVAL``, so a parked item was
   silent forever. The regression test drives the whole loop: park, grant,
   and prove the item is dispatchable again.

No model is involved anywhere in this file.
"""
from __future__ import annotations

import json
import os
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.dirname(HERE)
if REPO_ROOT not in sys.path:
    sys.path.insert(0, REPO_ROOT)
if HERE not in sys.path:
    sys.path.insert(0, HERE)

from _util import agents_shim

MILESTONES = [{
    "id": "m1",
    "title": "Write it",
    "items": [{
        "id": "m1.i1",
        "title": "write hello.txt",
        "detail": "Create context/artifacts/hello.txt with a greeting.",
        "acceptance": [{"kind": "check", "check": "file_exists",
                        "path": "context/artifacts/hello.txt"}],
    }],
}]

#: A workspace id that does not need a real workspace registry behind it.
WS_CODING = "ws-coding"
WS_GENERAL = "general"


class Rig:
    """Temp projects root + temp HARNESS_ROOT, so nothing touches the repo."""

    def __init__(self, runner=None):
        self._tmp = tempfile.TemporaryDirectory()
        self._shim = agents_shim(
            "agents.group.events", "agents.group.store", "agents.group.supervisor",
            "agents.group.manifest", "agents.group.criteria",
            "agents.group.materialize", "agents.group.project",
            "agents.group.runner", "agents.group.planner", "agents.group.tools")
        self._shim.__enter__()

        self._tmp_harness = tempfile.TemporaryDirectory()
        self._saved_harness = os.environ.get("HARNESS_ROOT")
        os.environ["HARNESS_ROOT"] = self._tmp_harness.name

        self.root = os.path.join(self._tmp.name, "projects")
        os.environ["STARK_PROJECTS_ROOT"] = self.root
        from server.store import group_registry
        self.registry = group_registry
        self.registry.set_projects_root(self.root)

        self.runner = runner if runner is not None else FakeRunner()

        from _util import real_tools
        self.tools = real_tools()

    def create(self, project_id: str = "tiny-report", *, ctx=None) -> str:
        out = self.tools.create_group_project(
            goal="produce a tiny report", milestones=MILESTONES,
            project_id=project_id, ctx=ctx)
        assert out.startswith("[OK]"), out
        return project_id

    def store(self, project_id: str = "tiny-report"):
        return self.registry.open_store(project_id)

    def close(self):
        try:
            self.registry.invalidate()
            self.registry.set_projects_root(None)
        finally:
            if self._saved_harness is None:
                os.environ.pop("HARNESS_ROOT", None)
            else:
                os.environ["HARNESS_ROOT"] = self._saved_harness
            self._tmp_harness.cleanup()
            self._shim.__exit__(None, None, None)
            self._tmp.cleanup()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()
        return False


class Ctx:
    """Stand-in for WorkspaceDeps — only ``project_path`` is read."""

    def __init__(self, path: str):
        self.project_path = path


class FakeRunner:
    def __init__(self):
        self.started: list[str] = []
        self._pending: list = []

    def member_for(self, item):
        return item.get("owner_member") or None

    def start(self, ctx):
        from agents.group.runner import RunResult
        self.started.append(ctx.item["id"])
        self._pending.append(RunResult(item_id=ctx.item["id"], ok=True,
                                       report="done"))

    def collect(self, project_id):
        pending, self._pending = self._pending, []
        return pending

    def cancel(self, project_id, item_id):
        pass


# ════════════════════════════════════════════════════════════════════════════
#  Origin — recorded in the harness, never in the folder
# ════════════════════════════════════════════════════════════════════════════


def test_origin_round_trips():
    with Rig() as rig:
        rig.create("tiny-report")
        assert rig.registry.origin_workspace("tiny-report") is None
        rig.registry.set_origin_workspace("tiny-report", WS_CODING)
        assert rig.registry.origin_workspace("tiny-report") == WS_CODING


def test_origin_is_NOT_written_into_the_project_folder():
    """The portability contract, asserted as a negative.

    ``check_portability`` (``01`` §14) and DM-18 both exist to stop a root
    from leaking into a folder. If the origin ever lands in there, a copied
    project stops being self-contained — so this test fails loudly rather
    than letting the feature quietly break the guarantee it was built around.
    """
    with Rig() as rig:
        rig.create("tiny-report")
        rig.registry.set_origin_workspace("tiny-report", WS_CODING)
        rig.store("tiny-report")  # flush anything pending

        project_dir = os.path.join(rig.root, "tiny-report")
        offenders = []
        for dirpath, _dirnames, filenames in os.walk(project_dir):
            for filename in filenames:
                if filename.endswith((".db", ".db-wal", ".db-shm")):
                    continue  # binary; the log legitimately holds the project id
                path = os.path.join(dirpath, filename)
                try:
                    with open(path, encoding="utf-8", errors="ignore") as handle:
                        text = handle.read()
                except OSError:
                    continue
                if WS_CODING in text:
                    offenders.append(os.path.relpath(path, project_dir))
        assert not offenders, (
            f"the workspace reference leaked into the project folder: {offenders}"
        )


def test_origin_survives_a_reread():
    """It is on disk, not in memory — otherwise a restart loses every badge."""
    with Rig() as rig:
        rig.create("tiny-report")
        rig.registry.set_origin_workspace("tiny-report", WS_CODING)
        path = os.path.join(rig._tmp_harness.name, ".deep_agents",
                            "group_origins.json")
        assert os.path.isfile(path), "the origin index was never written"
        with open(path, encoding="utf-8") as handle:
            assert json.load(handle) == {"tiny-report": WS_CODING}


def test_create_records_the_origin_from_ctx():
    """The tool learns the workspace from the injected deps, not the model."""
    with Rig() as rig:
        rig.create("tiny-report", ctx=Ctx(r"D:\MyProject\SomeCodingFolder"))
        # No workspace registry in this rig, so the path resolves to nothing;
        # the point is that it READ the path and did not invent one.
        assert rig.registry.origin_workspace("tiny-report") is None


# ════════════════════════════════════════════════════════════════════════════
#  Scoping — one rule, one home
# ════════════════════════════════════════════════════════════════════════════


def test_visible_projects_is_pure():
    rows = [{"id": "a", "origin_workspace_id": WS_CODING},
            {"id": "b", "origin_workspace_id": None},
            {"id": "c", "origin_workspace_id": "ws-other"}]

    assert [r["id"] for r in
            _visible(rows, WS_CODING, scoped=True)] == ["a"]
    assert [r["id"] for r in
            _visible(rows, WS_GENERAL, scoped=False)] == ["a", "b", "c"]
    # An unscoped workspace sees everything even if asked for a workspace id.
    assert len(_visible(rows, WS_CODING, scoped=False)) == 3
    # No workspace at all -> everything.
    assert len(_visible(rows, None, scoped=True)) == 3


def _visible(rows, workspace_id, *, scoped):
    from server.store import group_registry
    return group_registry.visible_projects(
        rows, workspace_id=workspace_id, scoped=scoped)


def test_a_global_project_shows_in_general_but_not_in_a_coding_workspace():
    """Projects predating the origin field must not vanish — they belong in
    General, which is where you would look for them."""
    with Rig() as rig:
        rig.create("legacy")           # no origin recorded
        rig.create("scoped")
        rig.registry.set_origin_workspace("scoped", WS_CODING)

        from server.store import group_registry
        coding = group_registry.visible_projects(
            rig.registry.list_projects(), workspace_id=WS_CODING, scoped=True)
        assert [r["id"] for r in coding] == ["scoped"]

        general = group_registry.visible_projects(
            rig.registry.list_projects(), workspace_id=WS_GENERAL, scoped=False)
        assert sorted(r["id"] for r in general) == ["legacy", "scoped"]


def test_list_projects_carries_the_origin():
    with Rig() as rig:
        rig.create("tiny-report")
        rig.registry.set_origin_workspace("tiny-report", WS_CODING)
        row = next(r for r in rig.registry.list_projects() if r["id"] == "tiny-report")
        assert row["origin_workspace_id"] == WS_CODING


# ════════════════════════════════════════════════════════════════════════════
#  Approvals — the half that was missing
# ════════════════════════════════════════════════════════════════════════════


def _park(rig, project_id="tiny-report", item_id="m1.i1"):
    """Drive one item onto the board, then park it awaiting a human."""
    supervisor = _supervisor(rig)
    supervisor.tick(project_id)          # claims + starts the member
    toolkit = rig.registry.toolkit()
    result = toolkit.request_approval(project_id, item_id=item_id,
                                      detail="wants to write outside the folder")
    assert result.ok, result.message
    store = rig.store(project_id)
    assert store.item(item_id)["blocked_reason"] == "awaiting_approval"
    return supervisor, toolkit


def _supervisor(rig):
    from server.services import group_scheduler
    return group_scheduler.build_supervisor(runner=rig.runner)


def test_request_approval_parks_the_item():
    with Rig(runner=FakeRunner()) as rig:
        rig.create("tiny-report")
        _park(rig)


def test_grant_clears_the_park_and_the_item_can_be_picked_up_again():
    """The regression.

    Before ``resolve_approval`` existed, nothing could emit
    ``APPROVAL_GRANTED``, and ``AWAITING_APPROVAL`` is stall-exempt — so the
    item was silent forever. This drives park -> grant -> dispatchable.
    """
    with Rig(runner=FakeRunner()) as rig:
        rig.create("tiny-report")
        _supervisor_obj, toolkit = _park(rig)
        store = rig.store("tiny-report")

        result = toolkit.resolve_approval("tiny-report", item_id="m1.i1", grant=True)
        assert result.ok, result.message

        item = store.item("m1.i1")
        assert item["blocked_reason"] is None, "the park was not cleared"

        kinds = [e["kind"] for e in store.events("tiny-report")]
        assert "approval.requested" in kinds
        assert "approval.granted" in kinds

        # And the supervisor no longer treats it as waiting on a human.
        from agents.group.supervisor import WAITING_BLOCK_REASONS
        assert item["blocked_reason"] not in WAITING_BLOCK_REASONS


def test_reject_blocks_the_item_and_releases_the_lease():
    with Rig(runner=FakeRunner()) as rig:
        rig.create("tiny-report")
        _supervisor_obj, toolkit = _park(rig)
        store = rig.store("tiny-report")

        result = toolkit.resolve_approval("tiny-report", item_id="m1.i1",
                                          grant=False, note="not this time")
        assert result.ok, result.message

        item = store.item("m1.i1")
        assert item["status"] == "blocked"
        assert item["blocked_reason"] == "approval_rejected"
        assert item["lease_owner"] is None
        assert item["lease_expires"] is None
        assert any(e["kind"] == "approval.rejected"
                   for e in store.events("tiny-report"))


def test_resolving_an_unknown_item_fails_cleanly():
    with Rig(runner=FakeRunner()) as rig:
        rig.create("tiny-report")
        result = rig.registry.toolkit().resolve_approval(
            "tiny-report", item_id="nope", grant=True)
        assert not result.ok
        assert "nope" in result.message


def test_a_grant_does_not_increment_attempts():
    """Approving is not a retry. If it incremented, the distinction between
    `invalidated` and `failed` would be decorative (``06`` §7)."""
    with Rig(runner=FakeRunner()) as rig:
        rig.create("tiny-report")
        _supervisor_obj, toolkit = _park(rig)
        store = rig.store("tiny-report")
        before = store.item("m1.i1")["attempts"]
        toolkit.resolve_approval("tiny-report", item_id="m1.i1", grant=True)
        assert store.item("m1.i1")["attempts"] == before


def test_the_model_cannot_grant_its_own_approval():
    """An approval the orchestrator can grant itself is not an approval."""
    from agents.group.tools import GroupToolkit

    assert "resolve_approval" not in GroupToolkit.member_tools()
    assert "resolve_approval" not in GroupToolkit.orchestrator_tools()

    from tools.AddOnTool import groupTools
    assert "resolve_approval" not in groupTools._OPS


class ParkingRunner(FakeRunner):
    """Parks the item on its first run AND returns a result anyway.

    That is what a real model does when it asks permission and then carries
    on: it calls ``request_approval``, finishes the work, and reports success.
    """

    def __init__(self, store, write: bool = True):
        super().__init__()
        self.store = store
        self.write = write
        self.parked = 0

    def start(self, ctx):
        from agents.group.runner import RunResult
        self.started.append(ctx.item["id"])
        if self.parked == 0:
            self.parked += 1
            self.store.append(ctx.project_id, "approval.requested", "member",
                              item_id=ctx.item["id"],
                              data={"detail": "wants to write outside the folder"})
        if self.write:
            target = os.path.join(ctx.project_root, "context", "artifacts")
            os.makedirs(target, exist_ok=True)
            with open(os.path.join(target, "hello.txt"), "w", encoding="utf-8") as fh:
                fh.write("hello from a member\n")
        self._pending.append(RunResult(item_id=ctx.item["id"], ok=True,
                                       report="done anyway"))


def test_a_member_cannot_finish_the_work_it_asked_permission_for():
    """The hole this found, and the reason the grant path had to ship.

    ``_harvest`` settled any ``doing`` item that had a result — without
    looking at the park. So a member that called ``request_approval`` and
    then returned success had its work marked **done**, behind the very
    approval it had just asked for. Exercising the loop showed the grant
    arriving *after* the project had already closed.

    A park is authoritative until a human answers.
    """
    with Rig() as rig:
        rig.create("tiny-report")
        rig.runner = ParkingRunner(rig.store("tiny-report"))
        supervisor = _supervisor(rig)

        for _ in range(5):
            supervisor.tick("tiny-report")

        store = rig.store("tiny-report")
        item = store.item("m1.i1")
        assert item["blocked_reason"] == "awaiting_approval", item
        assert item["status"] != "done", "the park was bypassed"
        assert store.project("tiny-report")["status"] == "running", (
            "the project completed behind the approval it asked for"
        )

        # The human answers.
        result = rig.registry.toolkit().resolve_approval(
            "tiny-report", item_id="m1.i1", grant=True)
        assert result.ok, result.message

        # And now it resumes and finishes on its own.
        for _ in range(5):
            supervisor.tick("tiny-report")
            if store.project("tiny-report")["status"] == "done":
                break
        assert store.project("tiny-report")["status"] == "done"
        assert store.item("m1.i1")["status"] == "done"


# ════════════════════════════════════════════════════════════════════════════
#  Router — the shape the UI depends on
# ════════════════════════════════════════════════════════════════════════════


def test_router_exposes_the_four_routes():
    from server.routers import groups
    paths = {r.path for r in groups.router.routes}
    assert paths == {
        "/api/groups",
        "/api/groups/{project_id}",
        "/api/groups/{project_id}/items/{item_id}/approval",
        "/api/groups/{project_id}/update",
    }


def test_the_board_cannot_edit_the_plan():
    """The UI is a monitoring surface, not a second mutation path.

    `add_member` / `attach_mcp` / `add_item` stay with the orchestrator, which
    validates them against the plan and the live catalogues. If the UI could
    call them, there would be two writers with different rules.
    """
    from server.routers import groups

    assert groups.UI_OPS == ("pause", "resume", "close")
    assert "add_member" not in groups.UI_OPS
    assert "attach_mcp" not in groups.UI_OPS
    assert "add_item" not in groups.UI_OPS


def _client():
    from fastapi import FastAPI
    from fastapi.testclient import TestClient

    from server.routers import groups

    app = FastAPI()
    app.include_router(groups.router)
    return TestClient(app)


def test_the_board_answers_over_http():
    with Rig(runner=FakeRunner()) as rig:
        rig.create("tiny-report")
        client = _client()

        res = client.get("/api/groups")
        assert res.status_code == 200
        body = res.json()
        assert [p["id"] for p in body["projects"]] == ["tiny-report"]
        assert body["projects"][0]["items_total"] == 1
        assert body["scoped"] is False

        res = client.get("/api/groups/tiny-report")
        assert res.status_code == 200
        detail = res.json()
        assert detail["path"] == rig.registry.project_paths("tiny-report").project_dir
        assert len(detail["milestones"]) == 1
        assert len(detail["items"]) == 1
        assert detail["project"]["status"] == "running"


def test_an_unknown_project_is_a_404_not_a_400():
    """OffBoard stringifies with its class name, which is not language to show
    a person — and the wrong status makes the UI say "bad request" when the
    real answer is "that project is gone"."""
    with Rig(runner=FakeRunner()):
        client = _client()
        assert client.get("/api/groups/nope").status_code == 404

        res = client.post("/api/groups/nope/items/x/approval", json={"grant": True})
        assert res.status_code == 404
        assert "OffBoard" not in res.json()["error"]

        res = client.post("/api/groups/nope/update", json={"op": "pause"})
        assert res.status_code == 404
        assert "OffBoard" not in res.json()["error"]


def test_a_forbidden_op_is_refused_over_http():
    with Rig(runner=FakeRunner()) as rig:
        rig.create("tiny-report")
        res = _client().post("/api/groups/tiny-report/update",
                             json={"op": "add_member"})
        assert res.status_code == 400
        assert "pause, resume, close" in res.json()["error"]


def test_pause_is_reported_as_queued():
    """The honest answer. The UI must not imply the op already happened."""
    with Rig(runner=FakeRunner()) as rig:
        rig.create("tiny-report")
        res = _client().post("/api/groups/tiny-report/update",
                             json={"op": "pause"})
        assert res.status_code == 200
        body = res.json()
        assert body["queued"] is True
        # Still running: the supervisor has not ticked yet.
        assert rig.store("tiny-report").project("tiny-report")["status"] == "running"


def test_progress_counts_for_a_panel_row():
    with Rig(runner=FakeRunner()) as rig:
        rig.create("tiny-report")
        from server.routers import groups
        progress = groups._progress("tiny-report")
        assert progress["milestones_total"] == 1
        assert progress["items_total"] == 1
        assert progress["items_done"] == 0
        assert progress["awaiting_approval"] == 0


def test_progress_survives_a_broken_project():
    """A project that vanishes from the board is worse than one shown broken."""
    with Rig(runner=FakeRunner()):
        from server.routers import groups
        progress = groups._progress("does-not-exist")
        assert progress["items_total"] == 0
