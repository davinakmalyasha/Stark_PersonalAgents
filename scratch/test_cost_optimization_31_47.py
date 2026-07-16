import unittest
from unittest.mock import patch, MagicMock
import os
from state_sync import shared_state
from llm import _get_deepseek_client
from tools import task

class TestCostOptimization31and47(unittest.TestCase):
    
    def setUp(self) -> None:
        # Reset total cost and remaining steps before each test
        shared_state["token_usage"] = {
            "total_input_tokens": 0,
            "total_output_tokens": 0,
            "total_cache_hit_tokens": 0,
            "total_cache_miss_tokens": 0,
            "total_cost": 0.0,
            "calls": []
        }
        if "remaining_steps" in shared_state:
            del shared_state["remaining_steps"]
        os.environ["DEEPSEEK_API_KEY"] = "fake_key_for_testing"

    @patch("llm.ChatOpenAI")
    def test_dynamic_token_allocation_low_steps(self, mock_chat_openai) -> None:
        # 1. Set remaining_steps to < 5
        shared_state["remaining_steps"] = 3
        
        # Test Developer role (capped at 1500 tokens, medium reasoning effort)
        _get_deepseek_client(model="deepseek-pro", temp=0.7, role="Developer")
        kwargs = mock_chat_openai.call_args[1]
        self.assertEqual(kwargs.get("max_tokens"), 1500)
        self.assertEqual(kwargs.get("reasoning_effort"), "medium")
        self.assertEqual(kwargs.get("extra_body"), {"thinking": {"type": "enabled"}})
        
        # Test Supervisor role (capped at 400 tokens, medium reasoning effort)
        _get_deepseek_client(model="deepseek-pro", temp=0.7, role="Supervisor")
        kwargs = mock_chat_openai.call_args[1]
        self.assertEqual(kwargs.get("max_tokens"), 400)
        self.assertEqual(kwargs.get("reasoning_effort"), "medium")
        self.assertEqual(kwargs.get("extra_body"), {"thinking": {"type": "enabled"}})

        # Test non-heavy role (capped at 400 tokens, thinking disabled)
        _get_deepseek_client(model="deepseek-pro", temp=0.7, role="default")
        kwargs = mock_chat_openai.call_args[1]
        self.assertEqual(kwargs.get("max_tokens"), 400)
        self.assertNotIn("reasoning_effort", kwargs)
        self.assertEqual(kwargs.get("extra_body"), {"thinking": {"type": "disabled"}})

    @patch("llm.ChatOpenAI")
    def test_dynamic_token_allocation_normal_steps(self, mock_chat_openai) -> None:
        # 1. Set remaining_steps to >= 5
        shared_state["remaining_steps"] = 10
        
        # Test Developer role (should use standard limits: 8000 tokens, max reasoning effort)
        _get_deepseek_client(model="deepseek-pro", temp=0.7, role="Developer")
        kwargs = mock_chat_openai.call_args[1]
        self.assertEqual(kwargs.get("max_tokens"), 8000)
        self.assertEqual(kwargs.get("reasoning_effort"), "max")
        self.assertEqual(kwargs.get("extra_body"), {"thinking": {"type": "enabled"}})
        
        # Test Supervisor role (should use standard limits: 800 tokens, max reasoning effort)
        _get_deepseek_client(model="deepseek-pro", temp=0.7, role="Supervisor")
        kwargs = mock_chat_openai.call_args[1]
        self.assertEqual(kwargs.get("max_tokens"), 800)
        self.assertEqual(kwargs.get("reasoning_effort"), "high")
        self.assertEqual(kwargs.get("extra_body"), {"thinking": {"type": "enabled"}})

    @patch("llm.invoke_messages_with_fallback")
    @patch("tools.load_subagent_history")
    @patch("tools.save_subagent_history")
    @patch("tools.clear_subagent_history")
    def test_subagent_budget_isolation_under_cap(self, mock_clear, mock_save, mock_load, mock_invoke) -> None:
        mock_load.return_value = []
        
        # Set initial cost
        shared_state["token_usage"]["total_cost"] = 0.10
        
        # Mock invoke to increment cost by 0.03 (under cap of 0.05)
        def side_effect(*args, **kwargs):
            shared_state["token_usage"]["total_cost"] += 0.03
            return "Subagent success output"
        mock_invoke.side_effect = side_effect
        
        res = task(name="BA-GapAnalyzer", task="Analyze the gap")
        self.assertEqual(res, "Subagent success output")
        self.assertAlmostEqual(shared_state["token_usage"]["total_cost"], 0.13)

    @patch("llm.invoke_messages_with_fallback")
    @patch("tools.load_subagent_history")
    @patch("tools.save_subagent_history")
    @patch("tools.clear_subagent_history")
    def test_subagent_budget_isolation_over_cap(self, mock_clear, mock_save, mock_load, mock_invoke) -> None:
        mock_load.return_value = []
        
        # Set initial cost
        shared_state["token_usage"]["total_cost"] = 0.10
        
        # Mock invoke to increment cost by 0.20 (above legacy cap)
        def side_effect(*args, **kwargs):
            shared_state["token_usage"]["total_cost"] += 0.20
            return "Subagent output above budget"
        mock_invoke.side_effect = side_effect
        
        # Budget check has been deleted; verify it completes without raising BudgetExceededException
        res = task(name="BA-GapAnalyzer", task="Analyze the gap")
        self.assertEqual(res, "Subagent output above budget")
        self.assertAlmostEqual(shared_state["token_usage"]["total_cost"], 0.30)

if __name__ == "__main__":
    unittest.main()
