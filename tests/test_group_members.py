"""Phase 2 tests — the project folder, materialisation, criteria, members.

Test project: ``synthetic-report`` (``06-phases-and-open-questions.md``,
Phase 2) — one member, two milestones, items whose acceptance criteria are real
file checks.

Members are exercised with a **fake runner**. That is deliberate and not a
shortcut: :class:`~agents.group.runner.ItemRunner` exists precisely so the tick
can be driven end to end with no model involved. A real LLM run is not
asserted anywhere in this suite, and saying otherwise would be a lie.
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

MANIFEST = """# Synthetic report

## Why
Prove a self-contained project folder works end to end.

---
```yaml
schema: 1
id: synthetic-report
owner: localhost
budget_usd: 5.0
completion:
  kind: all_milestones_done
  also_require: [no_item_status_failed]
autonomy:
  project_folder: autonomous
  external_read: autonomous
  external_write: needs_approval
  destructive: blocked
members:
  - name: researcher
    derived_from: general
    model: "deepseek/deepseek-v4.1-flash"
    thinking: high
    tools: [read_file, edit_file]
    skills: [web-research]
environment:
  skills: [web-research]
  mcp: []
milestones:
  - id: m1
    title: Gather
    items:
      - id: i1
        title: fetch one
        detail: fetch a page and write it down
        acceptance:
          - {kind: check, check: file_exists, path: context/artifacts/one.md}
  - id: m2
    title: Write
    depends_on: [m1]
    items:
      - id: i2
        title: compose
        acceptance:
          - {kind: check, check: regex, path: context/artifacts/report.md, pattern: "^# "}
```
"""


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


def _skill(name: str) -> dict:
    return {"name": name, "description": "how to research", "body": "# Research\n\nLook it up.",
            "frontmatter": {}, "allowed_tools": ["read_file"]}


class FakeRunner:
    """Deterministic stand-in for a member. Writes files, returns a report."""

    def __init__(self, behaviour=None):
        self.behaviour = behaviour or (lambda ctx: self._write_files(ctx))
        self.started: list[str] = []
        self._pending: list = []
        self.cancelled: list[str] = []

    def _write_files(self, ctx):
        from agents.group.runner import RunResult
        item = ctx.item
        target = os.path.join(ctx.project_root, "context", "artifacts")
        os.makedirs(target, exist_ok=True)
        if item["id"] == "i1":
            with open(os.path.join(target, "one.md"), "w", encoding="utf-8") as fh:
                fh.write("# fetched\n")
        elif item["id"] == "i2":
            with open(os.path.join(target, "report.md"), "w", encoding="utf-8") as fh:
                fh.write("# Report\n\nall done\n")
        return RunResult(item_id=item["id"], ok=True, report="wrote the file",
                         cost_usd=0.1, cost_tokens=100)

    def member_for(self, item):
        return item.get("owner_member") or "researcher"

    def start(self, ctx):
        self.started.append(ctx.item["id"])
        self._pending.append(self.behaviour(ctx))

    def collect(self, project_id):
        pending, self._pending = self._pending, []
        return pending

    def cancel(self, project_id, item_id):
        self.cancelled.append(item_id)


class Rig:
    def __init__(self, runner=None, manifest_text: str = MANIFEST, **sup_kwargs):
        self._tmp = tempfile.TemporaryDirectory()
        self._shim = agents_shim("agents.group.events", "agents.group.store",
                                 "agents.group.supervisor", "agents.group.manifest",
                                 "agents.group.criteria", "agents.group.materialize",
                                 "agents.group.project", "agents.group.runner")
        mods = self._shim.__enter__()
        g = {k.split(".")[-1]: v for k, v in mods.items()}
        self.ev, self.store_mod, self.sup_mod = g["events"], g["store"], g["supervisor"]
        self.manifest, self.criteria = g["manifest"], g["criteria"]
        self.project_mod, self.materialize = g["project"], g["materialize"]
        self.runner_mod = g["runner"]

        self.root = os.path.join(self._tmp.name, "projects")
        self.manifest_obj = self.manifest.parse(manifest_text)
        self.paths, self.store = self.project_mod.create_group_project(
            self.root, self.manifest_obj,
            spec_lookup=_spec, skill_lookup=_skill,
        )
        self.pid = self.manifest_obj.id
        self.runner = runner if runner is not None else FakeRunner()
        self.sup = self.sup_mod.Supervisor(
            self.store, runner=self.runner,
            context_for=lambda _p: self.context, **sup_kwargs,
        )

    @property
    def context(self):
        return self.sup_mod.ProjectContext(
            project_id=self.pid, root=self.paths.project_dir,
            members={"researcher": {"name": "researcher", "runtime": "test:noop"}},
            autonomy=self.manifest_obj.autonomy,
            manifest_members=self.manifest_obj.members,
        )

    def drive(self, max_ticks: int = 20) -> dict:
        report = {}
        for _ in range(max_ticks):
            report = self.sup.tick(self.pid)
            if self.store.project(self.pid)["status"] in ("done", "failed", "cancelled"):
                return report
            if report.get("halted"):
                return report
        return report

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
#  Manifest
# ════════════════════════════════════════════════════════════════════════════


def test_manifest_round_trips():
    with Rig() as rig:
        parsed = rig.manifest.parse(rig.manifest.render(rig.manifest_obj))
        assert parsed.id == rig.pid
        assert parsed.budget_usd == 5.0
        assert [m["id"] for m in parsed.milestones] == ["m1", "m2"]
        assert parsed.autonomy["external_write"] == "needs_approval"


def test_manifest_rejects_an_item_without_acceptance():
    with Rig() as rig:
        broken = rig.manifest.parse(MANIFEST.replace(
            "        acceptance:\n"
            "          - {kind: check, check: file_exists, path: context/artifacts/one.md}\n",
            ""))
        try:
            broken.validate()
        except rig.manifest.ManifestError as exc:
            assert "acceptance" in str(exc)
        else:
            raise AssertionError("an item with no acceptance criteria was accepted")


# ════════════════════════════════════════════════════════════════════════════
#  The folder
# ════════════════════════════════════════════════════════════════════════════


def test_create_scaffolds_and_materialises():
    with Rig() as rig:
        base = rig.paths.project_dir
        for relative in (
            "PROJECT.md", "AGENTS.md", "inbox.md",
            "agents/researcher.json",
            os.path.join("skills", "web-research", "SKILL.md"),
            os.path.join("tools", "bindings.json"),
            ".gp/state.db", ".gp/lock.json", ".gp/plan.md",
            os.path.join("context", "decisions.md"),
            os.path.join("context", "findings.md"),
        ):
            assert os.path.exists(os.path.join(base, relative)), f"missing {relative}"

        agent_path = os.path.join(base, "agents", "researcher.json")
        with open(agent_path, encoding="utf-8") as handle:
            agent = json.load(handle)
        # DM-09: provenance is what makes a snapshot regenerable.
        assert agent["derived_from"] == "general"
        assert agent["derived_at_version"] == rig.materialize.HARNESS_VERSION
        assert agent["model"] == "deepseek/deepseek-v4.1-flash"
        assert agent["thinking"] == "high"
        # A project may narrow capabilities, never widen them.
        assert set(agent["tools"]) <= {"read_file", "edit_file", "run_command"}


def test_a_project_cannot_widen_a_members_tools():
    with Rig() as rig:
        try:
            rig.materialize.agent_from_spec(_spec(), "x", tools=["run_command", "rm_rf"])
        except rig.materialize.MaterializationError as exc:
            assert "widen" in str(exc)
        else:
            raise AssertionError("an unknown tool was materialised")


def test_mcp_config_refuses_a_literal_credential():
    with Rig() as rig:
        for bad in (
            {"s": {"url": "https://x", "env_vars": ["sk-abcdefghijklmnopqrstuvwx"]}},
            {"s": {"url": "https://x", "bearer_token_env_var": "ghp_abcdefghijklmnopqrst"}},
            {"s": {"url": "https://x", "http_headers": {"Authorization": "Bearer abc"}}},
            {"s": {"url": "https://x", "unknown_key": "whatever"}},
        ):
            try:
                rig.materialize.validate_mcp_servers(bad)
            except rig.materialize.MaterializationError:
                pass
            else:
                raise AssertionError(f"accepted an unsafe MCP config: {bad}")
        # The correct form passes.
        rig.materialize.validate_mcp_servers({
            "sentry": {"url": "https://mcp.sentry.dev/mcp",
                       "bearer_token_env_var": "SENTRY_ACCESS_TOKEN",
                       "enabled_tools": ["list_issues"], "approval": "prompt"},
        })


def test_folder_is_portable_and_survives_a_copy():
    with Rig() as rig:
        assert rig.project_mod.check_portability(rig.paths) == []
        rig.store.close()

        other_root = os.path.join(rig._tmp.name, "elsewhere")
        moved = rig.project_mod.copy_to(rig.paths, other_root)
        assert os.path.isfile(moved.state_db)
        assert rig.project_mod.check_portability(moved) == []

        reopened = rig.store_mod.ProjectStore(moved.state_db)
        try:
            assert reopened.project(rig.pid)["status"] == "running"
            assert len(reopened.items(rig.pid)) == 2
        finally:
            reopened.close()
        rig.store = reopened


def test_portability_check_catches_an_absolute_path():
    with Rig() as rig:
        leaky = os.path.join(rig.paths.agents_dir, "leaky.json")
        with open(leaky, "w", encoding="utf-8") as handle:
            handle.write('{"name": "leaky", "workdir": "D:\\\\MyProject\\\\out"}')
        problems = rig.project_mod.check_portability(rig.paths)
        assert any("P-1" in p and "leaky.json" in p for p in problems), problems


# ════════════════════════════════════════════════════════════════════════════
#  Acceptance criteria
# ════════════════════════════════════════════════════════════════════════════


def test_criteria_checks():
    with Rig() as rig:
        root = rig.paths.project_dir
        target = os.path.join(root, "context", "artifacts")
        os.makedirs(target, exist_ok=True)
        with open(os.path.join(target, "n.md"), "w", encoding="utf-8") as fh:
            fh.write("# Title\n\nbody text\n")

        cases = [
            ({"kind": "check", "check": "file_exists", "path": "context/artifacts/n.md"}, True),
            ({"kind": "check", "check": "file_exists", "path": "context/artifacts/no.md"}, False),
            ({"kind": "check", "check": "dir_exists", "path": "context/artifacts"}, True),
            ({"kind": "check", "check": "min_bytes", "path": "context/artifacts/n.md", "bytes": 5}, True),
            ({"kind": "check", "check": "min_bytes", "path": "context/artifacts/n.md", "bytes": 9999}, False),
            ({"kind": "check", "check": "glob_nonempty", "pattern": "context/artifacts/*.md"}, True),
            ({"kind": "check", "check": "glob_nonempty", "pattern": "context/artifacts/*.xyz"}, False),
            ({"kind": "check", "check": "regex", "path": "context/artifacts/n.md", "pattern": "^# Title"}, True),
            ({"kind": "check", "check": "regex", "path": "context/artifacts/n.md", "pattern": "^# Nope"}, False),
            ({"kind": "check", "check": "nonsense"}, False),
        ]
        for criterion, expected in cases:
            result = rig.criteria.evaluate_criterion(criterion, root)
            assert result.ok is expected, f"{criterion} -> {result.status} {result.detail}"


def test_criteria_refuse_to_escape_the_project_root():
    with Rig() as rig:
        for bad in ("../../outside.txt", "D:\\Windows\\system32", "/etc/passwd"):
            result = rig.criteria.evaluate_criterion(
                {"kind": "check", "check": "file_exists", "path": bad},
                rig.paths.project_dir)
            assert result.status == rig.criteria.FAIL, bad
            assert result.ok is False


def test_manual_criterion_is_pending_not_failed():
    with Rig() as rig:
        result = rig.criteria.evaluate_criterion(
            {"kind": "manual", "prompt": "does this look right?"},
            rig.paths.project_dir)
        assert result.status == rig.criteria.PENDING


# ════════════════════════════════════════════════════════════════════════════
#  Members do work
# ════════════════════════════════════════════════════════════════════════════


def test_members_run_items_and_criteria_decide_done():
    with Rig() as rig:
        rig.drive()
        assert rig.store.project(rig.pid)["status"] == "done", (
            rig.store.project(rig.pid)["status"]
        )
        assert rig.runner.started == ["i1", "i2"]
        i1 = rig.store.item("i1")
        assert i1["status"] == "done"
        assert i1["attempts"] == 1
        # Cost flows from the run into the project, so the gate can see it.
        assert rig.store.project(rig.pid)["spent_usd"] > 0


def test_a_failing_run_retries_then_fails():
    with Rig() as rig:
        from agents.group.runner import RunResult

        def never_writes(ctx):
            return RunResult(item_id=ctx.item["id"], ok=True, report="did nothing")
            # ok=True but the criterion is not met: the CRITERIA decide, not the run.

        rig.runner.behaviour = never_writes
        rig.drive(max_ticks=30)
        i1 = rig.store.item("i1")
        assert i1["status"] == "failed", i1["status"]
        assert i1["attempts"] == int(i1["max_attempts"]), i1["attempts"]


def test_a_manual_criterion_parks_the_item_without_stalling():
    """A `manual` criterion must park, and a parked item is not a stall."""
    parked = MANIFEST.replace(
        "- {kind: check, check: file_exists, path: context/artifacts/one.md}",
        '- {kind: manual, prompt: "does this look right"}')
    with Rig(manifest_text=parked, stall_ticks=2) as rig:
        for _ in range(6):
            rig.sup.tick(rig.pid)
        item = rig.store.item("i1")
        assert item["status"] == "doing", f"manual criterion did not park: {item['status']}"
        assert rig.store.project(rig.pid)["halt_reason"] != "stalled", (
            "a legitimate approval wait was counted as a stall"
        )
