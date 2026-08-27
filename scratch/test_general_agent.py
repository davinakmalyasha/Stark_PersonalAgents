"""Tests for the General subagent: registry, scope, loop, dispatch, backends."""

import os
import unittest
from types import SimpleNamespace
from unittest.mock import patch


def _fake_llm_factory(turns):
    """Build a fake deepseek backend: list of (text, tool_calls) per turn."""
    calls = {"n": 0}

    def fake_llm(**kwargs):
        idx = min(calls["n"], len(turns) - 1)
        calls["n"] += 1
        text, tcs = turns[idx]
        return (text, tcs, None)

    return fake_llm, calls


class TestGeneralRegistry(unittest.TestCase):
    def test_registry_has_general_and_aliases(self):
        from tools import _load_subagents, _SUBAGENTS_REGISTRY

        _load_subagents()
        self.assertIn("General", _SUBAGENTS_REGISTRY)
        self.assertIn("Dev", _SUBAGENTS_REGISTRY)
        for alias in ("FreeAgent", "general-purpose", "Vision"):
            self.assertIn(alias, _SUBAGENTS_REGISTRY)
            self.assertEqual(
                _SUBAGENTS_REGISTRY[alias]["system_prompt"],
                _SUBAGENTS_REGISTRY["General"]["system_prompt"],
            )

    def test_general_scope_surface_is_life_work_kit(self):
        from tools._schemas import get_native_tool_names

        names = set(get_native_tool_names("general"))
        self.assertEqual(
            names,
            {"run_command", "read_pdf", "create_docx", "create_xlsx",
             "ask_user", "update_plan", "browser_act"},
        )
        # No code-repo access, no delegation leakage
        self.assertNotIn("read_file", names)
        self.assertNotIn("edit_file", names)
        self.assertNotIn("search_code", names)
        self.assertNotIn("task", names)
        self.assertNotIn("async_task", names)

    def test_continuable_children_resolve_general_spec(self):
        """P6 durable children rebuild prompt/permissions from the registry."""
        from agents.subagent_manager import _child_system_prompt, _child_permissions
        from agents.general_prompt import _GENERAL_SYSTEM_TEMPLATE
        from tools import _load_subagents

        _load_subagents()
        self.assertEqual(_child_system_prompt("General"), _GENERAL_SYSTEM_TEMPLATE)
        self.assertEqual(_child_permissions("General"), [])


class TestGeneralLoop(unittest.TestCase):
    def setUp(self):
        self.env = patch.dict(os.environ, {
            "GENERAL_MODEL_BACKEND": "deepseek",
            "REASONING_SUBAGENT_ROUTINE": "disabled",
        })
        self.env.start()
        self.addCleanup(self.env.stop)

        self.sess = patch("agents.general_agent._open_session", lambda *a, **k: None)
        self.sess.start()
        self.addCleanup(self.sess.stop)

    def test_loop_executes_tool_then_reports(self):
        fake_llm, calls = _fake_llm_factory([
            ("", [{"tool": "read_pdf", "args": {"file_path": "x.pdf"}, "id": "c1"}]),
            ("Report: PDF says hello.", []),
        ])
        exec_calls = []

        def fake_exec(name, args, scope=None):
            exec_calls.append((name, args, scope))
            return SimpleNamespace(content="PDF TEXT", is_error=False, exit_code=0,
                                   timed_out=False, offloaded=False)

        with patch("agents.general_agent.invoke_messages_with_fallback_stream", fake_llm), \
             patch("agents.general_agent.execute_tool_detailed", fake_exec):
            from agents.general_agent import general_loop

            report = general_loop("summarize x.pdf")

        self.assertEqual(report, "Report: PDF says hello.")
        self.assertEqual(exec_calls, [("read_pdf", {"file_path": "x.pdf"}, "General")])
        self.assertEqual(calls["n"], 2)

    def test_out_of_scope_tool_is_blocked_not_executed(self):
        fake_llm, _ = _fake_llm_factory([
            ("", [{"tool": "edit_file", "args": {"file_path": "a.py"}, "id": "c1"}]),
            ("done", []),
        ])
        executed = []

        def fake_exec(name, args, scope=None):
            executed.append(name)
            return SimpleNamespace(content="", is_error=False, exit_code=0,
                                   timed_out=False, offloaded=False)

        with patch("agents.general_agent.invoke_messages_with_fallback_stream", fake_llm), \
             patch("agents.general_agent.execute_tool_detailed", fake_exec):
            from agents.general_agent import general_loop

            general_loop("mutate a repo file")

        self.assertEqual(executed, [])  # never reached the executor

    def test_free_backend_selected_by_env(self):
        import llm.free_providers as fp

        class FakeAgent:
            calls = []

            def chat_with_info(self, **kwargs):
                FakeAgent.calls.append(kwargs)
                if len(FakeAgent.calls) == 1:
                    return {"reply": "", "tool_calls": [{
                        "id": "f1", "name": "ask_user",
                        "args": {"question": "which format?"},
                    }]}
                return {"reply": "answered", "tool_calls": []}

            def stream_complete(self, **kwargs):
                return self.chat_with_info(**kwargs)

        def fake_exec(name, args, scope=None):
            return SimpleNamespace(content="docx preferred", is_error=False,
                                   exit_code=0, timed_out=False, offloaded=False)

        with patch.dict(os.environ, {"GENERAL_MODEL_BACKEND": "free"}), \
             patch.object(fp, "FreeProviders", FakeAgent), \
             patch("agents.general_agent.execute_tool_detailed", fake_exec), \
             patch("agents.general_agent._open_session", lambda *a, **k: None):
            from agents.general_agent import general_loop

            report = general_loop("pick a doc format")

        self.assertEqual(report, "answered")
        # Free transport received OpenAI-native dicts with tool schemas
        first = FakeAgent.calls[0]
        self.assertIsInstance(first["messages"], list)
        self.assertEqual(first["messages"][0]["role"], "system")

    def test_direct_generator_yields_snapshots_and_final_report(self):
        fake_llm, _ = _fake_llm_factory([("all done early", [])])

        with patch("agents.general_agent.invoke_messages_with_fallback_stream", fake_llm), \
             patch("agents.general_agent._open_session", lambda *a, **k: None):
            from agents.general_agent import run_general_direct

            snapshots = list(run_general_direct("quick answer"))

        self.assertTrue(snapshots)
        self.assertEqual(snapshots[-1]["outputs"]["agent_report"], "all done early")


class TestTaskDispatch(unittest.TestCase):
    def setUp(self):
        self.env = patch.dict(os.environ, {"REASONING_SUBAGENT_ROUTINE": "disabled"})
        self.env.start()
        self.addCleanup(self.env.stop)

    def test_task_routes_to_general_node(self):
        with patch("agents.general_agent.general_loop",
                   return_value="General finished the errand."), \
             patch("agents.general_agent._open_session", lambda *a, **k: None):
            from tools.AddOnTool.subagentTools import task

            result = task("General", "plan my week")

        self.assertIn("AGENT HANDOFF REPORT", result)
        self.assertIn("DONE", result)
        self.assertIn("General finished the errand.", result)

    def test_legacy_alias_resolves_to_general_engine(self):
        with patch("agents.general_agent.general_loop",
                   return_value="handled by General"), \
             patch("agents.general_agent._open_session", lambda *a, **k: None):
            from tools.AddOnTool.subagentTools import task

            result = task("FreeAgent", "old-name request")

        self.assertIn("handled by General", result)


if __name__ == "__main__":
    unittest.main()
