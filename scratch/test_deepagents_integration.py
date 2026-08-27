import unittest

class TestDeepAgentsIntegration(unittest.TestCase):
    
    def test_developer_prompt_is_local_and_explicit(self):
        from agents.developer_agent import _build_system_prompt
        system_prompt, current_suffix = _build_system_prompt(
            "inspect the project", ".", is_first_call=False
        )
        self.assertIn("## Workspace Root", system_prompt)
        self.assertTrue(current_suffix == "" or current_suffix.startswith("<current-context>"))

    def test_task_tool_subagent_registry(self):
        # Test that subagents are successfully loaded and retrievable
        from tools import _load_subagents, _SUBAGENTS_REGISTRY
        _load_subagents()

        self.assertIn("Dev", _SUBAGENTS_REGISTRY)
        self.assertIn("General", _SUBAGENTS_REGISTRY)
        # Back-compat aliases resolve to the General spec
        self.assertIn("FreeAgent", _SUBAGENTS_REGISTRY)
        self.assertIn("general-purpose", _SUBAGENTS_REGISTRY)
        self.assertIn("Vision", _SUBAGENTS_REGISTRY)
        self.assertEqual(
            set(_SUBAGENTS_REGISTRY["General"]["tools"]),
            {"run_command", "browser_act", "read_pdf", "create_docx", "create_xlsx",
             "ask_user", "update_plan"},
        )
        # General is a terminal node: no delegation tools leak into its surface
        self.assertNotIn("task", _SUBAGENTS_REGISTRY["General"]["tools"])
        self.assertNotIn("async_task", _SUBAGENTS_REGISTRY["General"]["tools"])

if __name__ == "__main__":
    unittest.main()
