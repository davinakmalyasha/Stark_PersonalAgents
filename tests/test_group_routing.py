"""Phase 3 tests — routing, the tool surface, and live capability changes.

The centrepiece is :func:`test_routing_regression`, which ``03`` section 2.3
requires to assert **both directions**. The expensive failure is not "the
orchestrator missed a project" — it is wrapping a trivial job in a three-day
project, so the negative cases matter more than the positive ones.

The classifier is deterministic, so this whole file runs offline with no model.
That is the point of keeping :func:`~agents.group.planner.classify_shape` free
of LLM calls: a routing test that needed a model would not be run.
"""
from __future__ import annotations

import json
import os
import sys
import tempfile
import types

HERE = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.dirname(HERE)
if REPO_ROOT not in sys.path:
    sys.path.insert(0, REPO_ROOT)
if HERE not in sys.path:
    sys.path.insert(0, HERE)

from _util import agents_shim

MANIFEST = """# Synthetic vault

## Why
A project that needs a capability it does not yet have.

---
```yaml
schema: 1
id: synthetic-vault
owner: localhost
completion:
  kind: all_milestones_done
members:
  - name: researcher
    derived_from: general
    model: "deepseek/deepseek-v4.1-flash"
    thinking: high
    tools: [read_file, edit_file]
environment:
  skills: []
  mcp: []
milestones:
  - id: m1
    title: Gather
    items:
      - id: i1
        title: fetch
        acceptance:
          - {kind: check, check: file_exists, path: context/artifacts/one.md}
```
"""

CATALOGUE = {
    "servers": {
        "sentry": {"url": "https://mcp.sentry.dev/mcp",
                   "bearer_token_env_var": "SENTRY_ACCESS_TOKEN",
                   "scopes": ["deploy"]},
        "shadcn": {"command": "npx", "args": ["shadcn@latest", "mcp"]},
    }
}


def _read_json(path: str) -> dict:
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)


def _read_text(path: str) -> str:
    with open(path, encoding="utf-8") as handle:
        return handle.read()


def _load_toolkit_class():
    """The class once, outside any rig — some tests only need its surface."""
    with agents_shim("agents.group.tools"):
        from agents.group.tools import GroupToolkit
        return GroupToolkit


def _load_planner():
    """``plan_execution`` is pure, so the routing tests need no project at all."""
    with agents_shim("agents.group.planner"):
        from agents.group import planner
        return planner


GroupToolkit_ = _load_toolkit_class()
planner_ = _load_planner()


def _preview(goal: str):
    return planner_.plan_execution(goal)


def _spec(key: str = "general") -> types.SimpleNamespace:
    return types.SimpleNamespace(
        key=key, display=key.title(), tier="workspace", provider_tier="",
        kind="workspace", runtime="agents.general_agent:general_node",
        runtime_name="General", state_builder="plain",
        prompt_module="agents.general_prompt:_GENERAL_SYSTEM_TEMPLATE",
        tools=("read_file", "edit_file", "run_command"), tool_caps={},
        compaction={}, permissions=(), system_prompt="", model="",
        thinking="inherit",
    )


class Rig:
    def __init__(self, manifest_text: str = MANIFEST):
        self._tmp = tempfile.TemporaryDirectory()
        self._shim = agents_shim(
            "agents.group.events", "agents.group.store", "agents.group.supervisor",
            "agents.group.manifest", "agents.group.criteria",
            "agents.group.materialize", "agents.group.project",
            "agents.group.runner", "agents.group.planner", "agents.group.tools")
        mods = self._shim.__enter__()
        g = {k.split(".")[-1]: v for k, v in mods.items()}
        self.ev, self.store_mod = g["events"], g["store"]
        self.sup_mod, self.manifest = g["supervisor"], g["manifest"]
        self.criteria, self.materialize = g["criteria"], g["materialize"]
        self.project_mod, self.planner = g["project"], g["planner"]
        self.tools_mod = g["tools"]

        self.root = os.path.join(self._tmp.name, "projects")
        self.manifest_obj = self.manifest.parse(manifest_text)
        self.paths, self.store = self.project_mod.create_group_project(
            self.root, self.manifest_obj,
            spec_lookup=_spec, skill_lookup=lambda n: {
                "name": n, "description": "how", "body": "# How", "frontmatter": {},
                "allowed_tools": ["read_file"]},
            mcp_catalogue=CATALOGUE,
        )
        self.pid = self.manifest_obj.id
        self.context = self.sup_mod.ProjectContext(
            project_id=self.pid, root=self.paths.project_dir,
            members={"researcher": {"name": "researcher", "runtime": "test:noop",
                                    "derived_from": "general"}},
            autonomy=self.manifest_obj.autonomy,
            manifest_members=self.manifest_obj.members,
            paths=self.paths, spec_lookup=_spec,
            skill_lookup=lambda n: {"name": n, "description": "how", "body": "# How",
                                    "frontmatter": {}, "allowed_tools": ["read_file"]},
            mcp_catalogue=CATALOGUE,
        )
        self.sup = self.sup_mod.Supervisor(
            self.store, runner=None, context_for=lambda _p: self.context)
        self.toolkit = self.tools_mod.GroupToolkit(
            lambda project_id: (self.store, self.context) if project_id == self.pid else None,
            create=self.project_mod.create_group_project)

    def close(self):
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


# ════════════════════════════════════════════════════════════════════════════
#  Routing regression — the phase gate
# ════════════════════════════════════════════════════════════════════════════

#: ``(goal, expected shape)``. Negative cases are the ones that cost money.
ROUTES = [
    # ── should become a project ──────────────────────────────────────────
    ("sort out the q3 vendor invoices, match them to POs", "group"),
    ("reconcile the invoices against the purchase orders, flag mismatches", "group"),
    ("find out why auth is failing in staging and fix it", "group"),
    ("design the landing page and then build it", "group"),
    ("research the competitors then write the positioning brief", "group"),
    ("audit the vendor contracts end to end", "group"),
    # ── must NOT become a project ────────────────────────────────────────
    ("rename these 40 files to kebab-case", "solo"),
    ("write a python script that parses the csv", "solo"),
    ("show me the last 20 lines of the log", "solo"),
    ("convert this json to csv", "solo"),
    ("every weekday at 8am check the deploy status", "scheduled"),
    ("daily summary of the overnight build", "scheduled"),
]


def test_routing_regression():
    """``03`` section 2.3, both directions. No model involved."""
    failures = []
    for goal, expected in ROUTES:
        preview = _preview(goal)
        if preview.shape != expected:
            failures.append(f"  {goal!r}\n    expected {expected}, got "
                            f"{preview.shape}\n    {preview.rationale}")
    assert not failures, "routing mismatches:\n" + "\n".join(failures)


def test_routing_returns_a_rationale_for_every_case():
    """The decision must be explainable, not just correct (``03`` §2.2)."""
    for goal, _expected in ROUTES:
        preview = _preview(goal)
        assert preview.rationale.strip(), f"no rationale for {goal!r}"
        assert preview.shape in ("solo", "group", "scheduled")


def test_a_group_plan_is_actually_runnable():
    """A preview that produces an undispatachable item is not a preview."""
    preview = planner_.plan_execution(
        "sort out the q3 vendor invoices",
        agents=[{"name": "general", "key": "general",
                 "description": "documents research"}])
    assert preview.shape == "group"
    assert preview.environment["members"], "no members proposed"
    for milestone in preview.plan:
        assert milestone.get("items"), f"{milestone.get('id')} has no items"
        for item in milestone["items"]:
            assert item.get("acceptance"), f"{item.get('id')} has no criteria"


def test_at_task_forces_and_is_not_a_second_router():
    forced, goal = planner_.parse_directive("@task clean up the invoices")
    assert forced is True and goal == "clean up the invoices"
    not_forced, plain = planner_.parse_directive("clean up the invoices")
    assert not_forced is False and plain == "clean up the invoices"
    # Forcing is recorded, and it changes confirmation, not classification.
    preview = planner_.plan_execution("@task rename the files")
    assert preview.forced is True
    assert preview.shape == "solo", "forcing must not change the shape"
    assert planner_.requires_confirmation(preview) is False


# ════════════════════════════════════════════════════════════════════════════
#  Capability changes reach the folder, and only this project
# ════════════════════════════════════════════════════════════════════════════


def test_attach_mcp_end_to_end():
    with Rig() as rig:
        result = rig.toolkit.attach_mcp(rig.pid, server="sentry")
        assert result.ok, result.message
        assert not os.path.exists(os.path.join(rig.paths.mcp_dir, "servers.json")), (
            "the file changed before the op was applied"
        )
        rig.sup.tick(rig.pid)

        config_path = os.path.join(rig.paths.mcp_dir, "servers.json")
        config = _read_json(config_path)
        assert "sentry" in config["servers"]
        # DM-10: a name, never a value.
        assert config["servers"]["sentry"]["bearer_token_env_var"] == "SENTRY_ACCESS_TOKEN"

        kinds = [e["kind"] for e in rig.store.events(rig.pid)]
        assert rig.ev.CAPABILITY_CHANGED in kinds, "the change left no event"


def test_attach_refuses_a_server_that_is_not_installed():
    with Rig() as rig:
        rig.toolkit.attach_mcp(rig.pid, server="does-not-exist")
        rig.sup.tick(rig.pid)
        rejected = [e for e in rig.store.events(rig.pid) if e["kind"] == rig.ev.OP_REJECTED]
        assert rejected, "an unknown MCP server was accepted"


def test_capability_change_affects_only_its_own_project():
    """The folder is the unit: another project must see nothing."""
    with Rig() as rig:
        other_root = os.path.join(rig._tmp.name, "other")
        other_paths, other_store = rig.project_mod.create_group_project(
            other_root, rig.manifest.parse(MANIFEST.replace(
                "id: synthetic-vault", "id: other-project")),
            spec_lookup=_spec, skill_lookup=lambda _n: None, mcp_catalogue=CATALOGUE)

        rig.toolkit.attach_mcp(rig.pid, server="sentry")
        rig.sup.tick(rig.pid)

        mine = os.path.join(rig.paths.mcp_dir, "servers.json")
        theirs = os.path.join(other_paths.mcp_dir, "servers.json")
        assert os.path.exists(mine)
        assert not os.path.exists(theirs), "the change leaked into another project"
        other_store.close()


def test_attach_tool_cannot_widen_past_the_scope_ceiling():
    with Rig() as rig:
        # `edit_file` IS in the ceiling and is not withheld by the envelope,
        # so this is allowed. (`run_command` is also in the ceiling but is
        # `never` by default — D-1, `06` §3 — so it would be refused earlier,
        # by the envelope rather than by the ceiling. That path is covered in
        # tests/test_group_surface.py.)
        rig.toolkit.attach_tool(rig.pid, member="researcher",
                                tools=["read_file", "edit_file"])
        rig.sup.tick(rig.pid)
        agent = _read_json(os.path.join(rig.paths.agents_dir, "researcher.json"))
        assert set(agent["tools"]) == {"read_file", "edit_file"}

        # `rm_rf` is not in the ceiling: refused at op level, folder untouched.
        rig.toolkit.attach_tool(rig.pid, member="researcher", tools=["read_file", "rm_rf"])
        rig.sup.tick(rig.pid)
        rejected = [e for e in rig.store.events(rig.pid) if e["kind"] == rig.ev.OP_REJECTED]
        assert rejected, "a tool outside the ceiling was accepted"
        agent = _read_json(os.path.join(rig.paths.agents_dir, "researcher.json"))
        assert "rm_rf" not in agent["tools"]


def test_add_and_remove_member():
    with Rig() as rig:
        rig.toolkit.add_member(rig.pid, name="writer", derived_from="general",
                               tools=["read_file"])
        rig.sup.tick(rig.pid)
        path = os.path.join(rig.paths.agents_dir, "writer.json")
        assert os.path.isfile(path), "add_member did not materialise an agent"
        agent = _read_json(path)
        assert agent["derived_from"] == "general"
        assert agent["derived_at_version"], "no provenance (DM-09)"

        rig.toolkit.remove_member(rig.pid, name="writer")
        rig.sup.tick(rig.pid)
        assert not os.path.exists(path), "remove_member left the agent behind"


def test_attach_skill_copies_it_into_the_folder():
    with Rig() as rig:
        rig.toolkit.attach_skill(rig.pid, name="web-research")
        rig.sup.tick(rig.pid)
        copied = os.path.join(rig.paths.skills_dir, "web-research", "SKILL.md")
        assert os.path.isfile(copied), "the skill was not copied in"
        assert "# How" in _read_text(copied)


def test_folder_stays_portable_after_capability_changes():
    with Rig() as rig:
        rig.toolkit.attach_mcp(rig.pid, server="sentry")
        rig.toolkit.attach_skill(rig.pid, name="web-research")
        rig.toolkit.add_member(rig.pid, name="writer", derived_from="general")
        rig.sup.tick(rig.pid)
        problems = rig.project_mod.check_portability(rig.paths)
        assert problems == [], f"capability changes broke portability: {problems}"


# ════════════════════════════════════════════════════════════════════════════
#  The tool surface is narrow where it must be
# ════════════════════════════════════════════════════════════════════════════


def test_members_cannot_mutate_membership_or_policy():
    """OT-08: a member cannot grant itself anything."""
    member = set(GroupToolkit_.member_tools())
    forbidden = {
        "add_item", "add_milestone", "cancel_item", "invalidate_item",
        "add_member", "remove_member", "attach_tool",
        "attach_mcp", "detach_mcp", "attach_skill", "detach_skill",
        "pause", "resume", "close", "create_group_project",
    }
    leaked = member & forbidden
    assert not leaked, f"members can call {sorted(leaked)}"
    assert "request_capability" in member, (
        "members must be able to ASK for a capability, or the boundary is "
        "obstructive rather than safe"
    )


def test_orchestrator_surface_includes_the_reads():
    tools = set(GroupToolkit_.orchestrator_tools())
    assert {"plan_execution", "create_group_project", "board", "events"} <= tools
    assert {"attach_mcp", "attach_skill", "attach_tool", "add_member"} <= tools


def test_reads_do_not_queue():
    """OT-07: a read must not wait 15 seconds for a tick."""
    with Rig() as rig:
        before = rig.store.max_seq(rig.pid)
        rig.toolkit.board(rig.pid)
        rig.toolkit.events(rig.pid)
        rig.toolkit.item_detail(rig.pid, item_id="i1")
        assert rig.store.max_seq(rig.pid) == before, "a read wrote to the log"


def test_board_reports_progress_and_waits():
    with Rig() as rig:
        text = rig.toolkit.board(rig.pid)
        assert "milestones 0/1" in text, text
        assert "i1" in text, text


# ════════════════════════════════════════════════════════════════════════════
#  The llm criterion, via an injected judge
# ════════════════════════════════════════════════════════════════════════════


def test_llm_criterion_is_unsupported_without_a_judge():
    with Rig() as rig:
        result = rig.criteria.evaluate_criterion(
            {"kind": "llm", "prompt": "is this good?"}, rig.paths.project_dir)
        assert result.status == rig.criteria.UNSUPPORTED, (
            "an llm criterion must not be guessed when no judge is configured"
        )


def test_llm_criterion_uses_the_judge():
    with Rig() as rig:
        criterion = {"kind": "llm", "prompt": "is this good?"}
        root = rig.paths.project_dir

        passing = rig.criteria.evaluate_criterion(
            criterion, root, judge=lambda _c, _r: (True, "looks right"))
        assert passing.status == rig.criteria.PASS

        failing = rig.criteria.evaluate_criterion(
            criterion, root, judge=lambda _c, _r: (False, "not acceptable"))
        assert failing.status == rig.criteria.FAIL

        def exploding(_c, _r):
            raise RuntimeError("judge blew up")

        broken = rig.criteria.evaluate_criterion(criterion, root, judge=exploding)
        assert broken.status == rig.criteria.FAIL, "a broken judge must not pass"


def test_the_supervisor_can_judge_llm_criteria_through_the_context():
    with Rig() as rig:
        rig.context.judge = lambda _c, _r: (True, "acceptable")
        # i1's criterion is a file check, so make it an llm one to prove the
        # judge actually reaches phase 7.
        rig.store.append(rig.pid, rig.ev.ITEM_CREATED, "orchestrator",
                         milestone_id="m1", item_id="i2",
                         data={"id": "i2", "title": "judged",
                               "acceptance": [{"kind": "llm", "prompt": "ok?"}]})
        rig.sup.tick(rig.pid)
        rig.store.append(rig.pid, rig.ev.ITEM_CLAIMED, "supervisor", item_id="i2",
                         data={"lease_owner": "researcher", "lease_expires": 1e12})
        rig.store.append(rig.pid, rig.ev.ITEM_PROGRESS, "member:researcher",
                         item_id="i2", data={"result": "wrote something"})
        rig.sup.tick(rig.pid)
        assert rig.store.item("i2")["status"] == "done", (
            "the judge verdict did not reach the transition"
        )
