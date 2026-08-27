import unittest
from unittest.mock import patch

from langchain_core.messages import AIMessage, HumanMessage, SystemMessage, ToolMessage

from memory.context_compaction import (
    checkpoint_compact,
    compact_history,
    CheckpointCompactionError,
    get_compaction_policy,
)


class TestSingleCheckpointCompaction(unittest.TestCase):
    def test_below_threshold_is_append_only(self):
        messages = [
            SystemMessage(content="System"),
            HumanMessage(content="Task"),
            AIMessage(content="Done"),
        ]
        result, info = compact_history(messages, policy=get_compaction_policy())
        self.assertIs(result, messages)
        self.assertFalse(info["checkpoint"])

    def test_checkpoint_places_map_before_summary_and_keeps_exact_tail(self):
        messages = [
            SystemMessage(content="System"),
            HumanMessage(content="old task"),
            AIMessage(content="old response"),
            HumanMessage(content="projectMap+:\nold delta"),
            HumanMessage(content="new task"),
            AIMessage(content="tool call", tool_calls=[{
                "name": "read_file",
                "args": {"file_path": "a.py"},
                "id": "call-1",
                "type": "tool_call",
            }]),
            ToolMessage(content="[read_file]:\n[FILE] a.py\ncurrent", tool_call_id="call-1", name="read_file"),
        ]
        result = checkpoint_compact(
            messages,
            tool_call_log=[{"tool": "read_file"}],
            created=[],
            modified=[],
            tail_budget_tokens=4_000,
            full_project_map="projectMap:\nroot: a.py",
            summary_text="[COMPACTION]\nstate: in_progress\n[/COMPACTION]",
        )

        self.assertEqual(result[0].content, "System")
        self.assertIn("projectMap:", result[1].content)
        self.assertIn("[COMPACTION]", result[2].content)
        self.assertNotIn("projectMap+:", "\n".join(str(m.content) for m in result))
        self.assertEqual(result[-1].content, "[read_file]:\n[FILE] a.py\ncurrent")
        self.assertEqual(result[-2].tool_calls[0]["name"], "read_file")

    def test_600k_and_700k_use_one_checkpoint_operation(self):
        from memory.context_budget import estimate_messages

        messages = [
            SystemMessage(content="System"),
            HumanMessage(content="task"),
            AIMessage(content="response"),
        ]
        before = estimate_messages(messages)
        policy = get_compaction_policy().__class__(
            role="developer",
            checkpoint_trigger=before,
            hard_limit=before + 1,
            tail_budget_tokens=4_000,
            summary_output_tokens=4_096,
        )
        with patch(
            "memory.context_compaction._refresh_checkpoint_map",
            return_value="projectMap:\nroot: app.py",
        ) as refresh, patch(
            "memory.context_compaction._build_free_compaction_summary",
            return_value="[COMPACTION]\nstate: in_progress\n[/COMPACTION]",
        ) as summarize:
            normal, normal_info = compact_history(
                messages, policy=policy, projected_tokens=before, chat_id="checkpoint-test"
            )
            emergency, emergency_info = compact_history(
                messages, policy=policy, projected_tokens=before + 1, chat_id="checkpoint-test"
            )

        self.assertTrue(normal_info["checkpoint"])
        self.assertEqual(normal_info["trigger"], "checkpoint")
        self.assertTrue(emergency_info["checkpoint"])
        self.assertEqual(emergency_info["trigger"], "hard_guard")
        self.assertEqual(refresh.call_count, 2)
        self.assertEqual(summarize.call_count, 2)
        self.assertIn("projectMap:", normal[1].content)
        self.assertIn("projectMap:", emergency[1].content)

    def test_checkpoint_failure_does_not_rewrite_input(self):
        from memory.context_budget import estimate_messages

        messages = [
            SystemMessage(content="System"),
            HumanMessage(content="task"),
            AIMessage(content="response"),
        ]
        original = list(messages)
        before = estimate_messages(messages)
        policy = get_compaction_policy().__class__(
            role="developer", checkpoint_trigger=before, hard_limit=before + 1
        )
        with patch(
            "memory.context_compaction._refresh_checkpoint_map",
            side_effect=CheckpointCompactionError("map unavailable"),
        ):
            with self.assertRaises(CheckpointCompactionError):
                compact_history(messages, policy=policy, projected_tokens=before)
        self.assertEqual(messages, original)

    def test_free_router_prefers_a_fitting_million_token_model(self):
        from llm.free_providers import _get_models_for_capability

        models = _get_models_for_capability(
            "text", min_context_tokens=900_000, prefer_context=True
        )
        self.assertTrue(models)
        self.assertGreaterEqual(models[0]["context"], 900_000)
        self.assertEqual(models[0]["context"], max(m["context"] for m in models))

    def test_tool_catalog_has_one_canonical_native_and_diagnostic_view(self):
        from tools._schemas import (
            format_compact_tool_help,
            get_compact_tool_docs,
            get_native_tool_names,
            get_native_tools,
        )

        names = get_native_tool_names("developer")
        native = get_native_tools("developer")
        docs = get_compact_tool_docs("developer")
        native_names = [item["function"]["name"] for item in native]

        self.assertEqual(len(names), len(set(names)))
        self.assertEqual(set(names), set(native_names))
        self.assertEqual(set(names), set(docs))
        help_text = format_compact_tool_help(names)
        self.assertEqual(help_text.count("- run_command("), 1)
        self.assertNotIn("_NATIVE_DESCRIPTIONS", help_text)
        self.assertNotIn("```tool", help_text)

        # General: life-work kit — no repo read/edit, no delegation tools
        general_names = get_native_tool_names("general")
        self.assertEqual(
            set(general_names),
            {"run_command", "read_pdf", "create_docx", "create_xlsx",
             "ask_user", "update_plan", "browser_act"},
        )
        self.assertNotIn("browser_navigate", format_compact_tool_help(general_names))


if __name__ == "__main__":
    unittest.main()
