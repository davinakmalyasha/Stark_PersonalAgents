"""Phase 4 integration tests — registry, boot, scheduler, tool surface.

These are the tests Phase 0-3 could not have: they go through the *wired*
path (``server.store.group_registry`` → ``GroupToolkit`` → tools) rather than
constructing a rig by hand. A module that imports cleanly and a module that is
reachable from the app are different things.

The scheduler tests inject a fake runner, so still no model is involved.
"""
from __future__ import annotations

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


class Rig:
    """A temp projects root with the registry pointed at it."""

    def __init__(self, runner=None, patch_tools: bool = True):
        self._tmp = tempfile.TemporaryDirectory()
        self._shim = agents_shim(
            "agents.group.events", "agents.group.store", "agents.group.supervisor",
            "agents.group.manifest", "agents.group.criteria",
            "agents.group.materialize", "agents.group.project",
            "agents.group.runner", "agents.group.planner", "agents.group.tools")
        self._shim.__enter__()

        self.root = os.path.join(self._tmp.name, "projects")
        os.environ["STARK_PROJECTS_ROOT"] = self.root
        from server.store import group_registry
        self.registry = group_registry
        self.registry.set_projects_root(self.root)

        self.runner = runner if runner is not None else FakeRunner()
        self.tools = None
        if patch_tools:
            # Other suites install an offline `tools` stub that is never
            # restored, so a bare `import tools` can silently yield the stub.
            # Evict it first (_util.restore_tools) or every attribute lookup
            # fails with a confusing AttributeError.
            from _util import real_tools
            self.tools = real_tools()

    def create(self, project_id: str = "tiny-report", goal: str = "produce a tiny report",
               milestones=None) -> str:
        if self.tools is not None:
            out = self.tools.create_group_project(
                goal=goal, milestones=milestones or MILESTONES,
                project_id=project_id)
        else:
            from tools.AddOnTool import groupTools
            out = groupTools.create_group_project(
                goal=goal, milestones=milestones or MILESTONES,
                project_id=project_id)
        assert out.startswith("[OK]"), out
        return project_id

    def supervisor(self):
        from server.services import group_scheduler
        return group_scheduler.build_supervisor(runner=self.runner)

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


class FakeRunner:
    """Writes the file the criterion asks for. No model involved."""

    def __init__(self, write: bool = True):
        self.write = write
        self.started: list[str] = []
        self._pending: list = []

    def member_for(self, item):
        return item.get("owner_member") or None

    def start(self, ctx):
        from agents.group.runner import RunResult
        self.started.append(ctx.item["id"])
        if self.write:
            target = os.path.join(ctx.project_root, "context", "artifacts")
            os.makedirs(target, exist_ok=True)
            with open(os.path.join(target, "hello.txt"), "w", encoding="utf-8") as fh:
                fh.write("hello from a member\n")
        self._pending.append(RunResult(item_id=ctx.item["id"], ok=True,
                                       report="wrote hello.txt"))

    def collect(self, project_id):
        pending, self._pending = self._pending, []
        return pending

    def cancel(self, project_id, item_id):
        pass


# ════════════════════════════════════════════════════════════════════════════
#  Registry — the folder IS the registry
# ════════════════════════════════════════════════════════════════════════════


def test_registry_discovers_a_project_on_disk():
    with Rig() as rig:
        rig.create("tiny-report")
        assert "tiny-report" in rig.registry.list_project_ids()

        rows = rig.registry.list_projects()
        assert len(rows) == 1
        assert rows[0]["id"] == "tiny-report"
        assert rows[0]["status"] == "running", rows[0]

        found = rig.registry.resolve("tiny-report")
        assert found is not None
        _paths, store, context = found
        assert context.project_id == "tiny-report"
        assert context.paths is not None
        assert len(store.items("tiny-report")) == 1


def test_registry_survives_a_bad_project():
    """One unreadable project must not take down the list."""
    with Rig() as rig:
        rig.create("good")
        broken = os.path.join(rig.root, "broken")
        os.makedirs(broken, exist_ok=True)
        with open(os.path.join(broken, "PROJECT.md"), "w", encoding="utf-8") as fh:
            fh.write("```yaml\nid: [unclosed\n```\n")
        ids = rig.registry.list_project_ids()
        assert "good" in ids and "broken" in ids
        rows = rig.registry.list_projects()
        assert any(r["id"] == "good" for r in rows), "a bad project hid a good one"


def test_copy_in_is_discovered():
    """A folder copied in from elsewhere needs no registration step."""
    with Rig() as rig:
        rig.create("tiny-report")
        rig.registry.invalidate()

        import shutil
        target = os.path.join(rig.root, "copied")
        shutil.copytree(os.path.join(rig.root, "tiny-report"), target)
        assert "copied" in rig.registry.list_project_ids()
        # Its own state.db still answers for its own id.
        store = rig.registry.open_store("copied")
        assert store.project("tiny-report") is not None


# ════════════════════════════════════════════════════════════════════════════
#  The tool surface reaches the engine
# ════════════════════════════════════════════════════════════════════════════


def test_plan_execution_advisory_shapes():
    with Rig(patch_tools=False):
        from tools.AddOnTool import groupTools
        assert "shape=group" in groupTools.plan_execution(
            "reconcile the invoices against the purchase orders")
        assert "shape=scheduled" in groupTools.plan_execution(
            "every weekday at 8am check the deploy status")
        assert "shape=solo" in groupTools.plan_execution("rename these files")


def test_create_then_board_through_the_tools():
    with Rig() as rig:
        rig.create("tiny-report")
        board = rig.tools.group_board("tiny-report")
        assert "tiny-report" in board
        assert "m1" in board
        assert "m1.i1" in board
        assert "milestones 0/1" in board


def test_group_update_queues_and_applies():
    with Rig() as rig:
        rig.create("tiny-report")
        out = rig.tools.group_update("tiny-report", "add_item", {
            "item_id": "m1.i2", "milestone_id": "m1", "title": "second",
            "detail": "another one",
            "acceptance": [{"kind": "check", "check": "file_exists",
                            "path": "context/artifacts/second.txt"}],
        })
        assert out.startswith("[OK]"), out
        # Queued: not visible until a tick.
        assert len(rig.registry.open_store("tiny-report").items("tiny-report")) == 1
        rig.supervisor().tick("tiny-report")
        assert len(rig.registry.open_store("tiny-report").items("tiny-report")) == 2


def test_group_update_rejects_an_unknown_op():
    with Rig() as rig:
        rig.create("tiny-report")
        assert rig.tools.group_update("tiny-report", "explode").startswith("[ERR]")


# ════════════════════════════════════════════════════════════════════════════
#  The scheduler drives projects
# ════════════════════════════════════════════════════════════════════════════


def test_a_tick_drives_a_project_to_done():
    with Rig() as rig:
        rig.create("tiny-report")
        supervisor = rig.supervisor()
        for _ in range(6):
            supervisor.tick("tiny-report")
            project = rig.registry.open_store("tiny-report").project("tiny-report")
            if project["status"] == "done":
                break
        project = rig.registry.open_store("tiny-report").project("tiny-report")
        assert project["status"] == "done", project
        assert rig.runner.started == ["m1.i1"]


def test_tick_all_walks_every_project():
    with Rig() as rig:
        rig.create("one")
        rig.create("two", goal="a different tiny report")
        supervisor = rig.supervisor()
        # build_supervisor is used by tick_all via the module singleton; drive
        # the same instance directly so the fake runner is the one used.
        for project_id in rig.registry.list_project_ids():
            supervisor.tick(project_id)
        assert set(rig.runner.started) == {"m1.i1"}


def test_boot_recovery_rebuilds_projections():
    with Rig() as rig:
        rig.create("tiny-report")
        store = rig.registry.open_store("tiny-report")
        before = sorted((i["id"], i["status"]) for i in store.items("tiny-report"))

        # Corrupt a row the way a hand-edit would, checkpoint current.
        store._db.execute("UPDATE items SET status='nonsense' WHERE id='m1.i1'")
        store._db.commit()
        assert store.item("m1.i1")["status"] == "nonsense"

        from server.services import group_scheduler
        rebuilt = group_scheduler.recover_all()
        assert rebuilt == 1
        after = sorted((i["id"], i["status"])
                       for i in rig.registry.open_store("tiny-report").items("tiny-report"))
        assert after == before, "boot recovery did not repair the projection"


def test_stopping_the_scheduler_drops_cached_stores():
    with Rig() as rig:
        rig.create("tiny-report")
        with_ = rig.registry.open_store("tiny-report")
        from server.services import group_scheduler
        group_scheduler.stop_group_scheduler(timeout=0.1)
        # The registry invalidates on stop; the next open must be a new handle.
        again = rig.registry.open_store("tiny-report")
        assert again is not with_


# ════════════════════════════════════════════════════════════════════════════
#  Tool registration
# ════════════════════════════════════════════════════════════════════════════


def test_group_tools_are_in_the_registry():
    """A tool the model cannot see is a tool that does not exist."""
    import tools._schemas as schemas

    names = {d["name"] for d in schemas.TOOL_DEFINITIONS}
    required = {"plan_execution", "create_group_project", "group_board",
                "group_item_detail", "group_events", "group_update"}
    assert required <= names, f"missing schemas: {sorted(required - names)}"
    assert required <= set(schemas._TOOL_DOCS), "missing doc one-liners"
    assert required <= set(schemas._SCOPE_MAP.get("orchestrator") or []), (
        "the orchestrator scope does not grant the group tools"
    )


def test_group_tools_resolve_through_the_facade():
    from _util import real_tools
    facade = real_tools()
    for name in ("plan_execution", "create_group_project", "group_board",
                 "group_item_detail", "group_events", "group_update"):
        assert callable(getattr(facade, name, None)), f"{name} is not on the facade"


# ════════════════════════════════════════════════════════════════════════════
#  The runner scopes its run — found by the first REAL run
# ════════════════════════════════════════════════════════════════════════════


def test_local_runner_scopes_the_run_and_captures_cost():
    """A member run must not pollute root state, and must report its cost.

    Found by the first real run: ``spend`` stayed $0.00 because the runner
    read a ``cost_usd`` key that no agent node returns. Cost lives in the
    scoped ``state_sync`` token usage, exactly as delegation reads it
    (``delegation.py:739``) — so the runner has to set a scope, or the budget
    ceiling can never fire.
    """
    import types as _types

    from sync.state_sync import reset_shared_state, safe_get_state

    root_before = dict(safe_get_state().get("outputs") or {})

    fake = _types.ModuleType("gp_fake_agent")

    def run(state):
        from sync.state_sync import safe_update_state
        safe_update_state({
            "outputs": {"agent_report": "did the thing"},
            "token_usage": {"total_input_tokens": 100, "total_output_tokens": 20,
                            "total_cache_hit_tokens": 0,
                            "total_cache_miss_tokens": 0,
                            "total_cost": 0.25, "calls": []},
        })
        return {"agent_report": "did the thing"}

    fake.run = run
    sys.modules["gp_fake_agent"] = fake
    try:
        from agents.group.runner import LocalRunner, RunContext
        result = LocalRunner()._execute_node(RunContext(
            project_id="p", project_root=REPO_ROOT,
            item={"id": "i1", "title": "t"},
            member={"runtime": "gp_fake_agent:run", "derived_from": "general"},
            member_name="m", chat_id="c", autonomy={},
        ))
    finally:
        sys.modules.pop("gp_fake_agent", None)

    assert result.ok, result.error
    assert result.report == "did the thing"
    assert result.cost_usd == 0.25, "cost was not captured from the scoped state"
    assert result.cost_tokens == 120
    # The ROOT state must be untouched: a member run is scoped, like a job.
    assert (safe_get_state().get("outputs") or {}) == root_before, (
        "the member run leaked into the root state"
    )
    reset_shared_state()
