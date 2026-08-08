import unittest
from unittest.mock import patch
import os
from tools import task as run_task, _SUBAGENTS_REGISTRY, _load_subagents
from subagent_swarm import load_subagent_history, get_subagent_history_path
from langchain_core.messages import SystemMessage, HumanMessage, AIMessage

class TestSwarmRehydration(unittest.TestCase):
    def setUp(self):
        _load_subagents()
        # Clean up any leftover files from previous test failures
        for chat_id in ["chat_success", "chat_failure"]:
            path = get_subagent_history_path(chat_id, "DevOps-Issues")
            if os.path.exists(path):
                os.remove(path)

    def tearDown(self):
        for chat_id in ["chat_success", "chat_failure"]:
            path = get_subagent_history_path(chat_id, "DevOps-Issues")
            if os.path.exists(path):
                os.remove(path)

    @patch("llm.invoke_messages_with_fallback")
    def test_swarm_rehydration_success(self, mock_invoke):
        from state_sync import shared_state
        shared_state["chat_id"] = "chat_success"
        
        mock_invoke.return_value = "Task completed successfully"
        
        run_task("DevOps-Issues", "Perform action A")
        
        self.assertTrue(mock_invoke.called)
        messages_arg = mock_invoke.call_args[1]["messages"]
        print("MESSAGES ARG:", [(type(m).__name__, getattr(m, 'content', '')) for m in messages_arg])
        self.assertEqual(len(messages_arg), 2)
        self.assertIsInstance(messages_arg[0], SystemMessage)
        self.assertIsInstance(messages_arg[1], HumanMessage)
        self.assertEqual(messages_arg[1].content, "Perform action A")
        
        path = get_subagent_history_path("chat_success", "DevOps-Issues")
        self.assertFalse(os.path.exists(path))

    @patch("llm.invoke_messages_with_fallback")
    def test_swarm_rehydration_failure_preserves_history(self, mock_invoke):
        from state_sync import shared_state
        shared_state["chat_id"] = "chat_failure"
        
        mock_invoke.side_effect = RuntimeError("LLM crashed")
        
        with self.assertRaises(RuntimeError):
            run_task("DevOps-Issues", "Perform action B")
            
        path = get_subagent_history_path("chat_failure", "DevOps-Issues")
        self.assertTrue(os.path.exists(path))
        
        saved_messages = load_subagent_history("chat_failure", "DevOps-Issues")
        self.assertEqual(len(saved_messages), 2)
        self.assertEqual(saved_messages[1].content, "Perform action B")

if __name__ == '__main__':
    unittest.main()
