import unittest
from unittest.mock import patch, MagicMock
import os
from langgraph.types import Command
from it_department_nodes_base import ITState
from it_department_graph import budget_guard_node
from sa_agent import sa_node
from ba_agent import ba_node

class TestCostOptimization24to26(unittest.TestCase):
    
    def test_budget_guard_node_below_cap(self) -> None:
        # Test routing below budget cap
        state = ITState(
            client_request="Test request",
            requirements="",
            tech_spec="",
            code="",
            agent_report="",
            test_report="",
            devops_config="",
            analytics_report="",
            error_count=0,
            next_agent="developer",
            project_path="d:\\MyProject\\LangChain",
            chat_id="test_chat",
            agents_plan="",
            active_tasks=["developer"],
            requirements_updated=False,
            tech_spec_updated=False,
            code_updated=False,
            remaining_steps=40
        )
        
        from state_sync import shared_state
        shared_state["token_usage"]["total_cost"] = 0.45
        os.environ["DEEP_AGENTS_BUDGET_CAP"] = "1.00"
        
        cmd = budget_guard_node(state)
        self.assertIsInstance(cmd, Command)
        self.assertEqual(cmd.goto, "developer")

    def test_budget_guard_node_above_cap(self) -> None:
        # Test routing above budget cap
        state = ITState(
            client_request="Test request",
            requirements="",
            tech_spec="",
            code="",
            agent_report="",
            test_report="",
            devops_config="",
            analytics_report="",
            error_count=0,
            next_agent="developer",
            project_path="d:\\MyProject\\LangChain",
            chat_id="test_chat",
            agents_plan="",
            active_tasks=["developer"],
            requirements_updated=False,
            tech_spec_updated=False,
            code_updated=False,
            remaining_steps=40
        )
        
        from state_sync import shared_state
        shared_state["token_usage"]["total_cost"] = 1.25
        os.environ["DEEP_AGENTS_BUDGET_CAP"] = "1.00"
        
        cmd = budget_guard_node(state)
        self.assertIsInstance(cmd, Command)
        self.assertEqual(cmd.goto, "gc")
        self.assertEqual(cmd.update.get("next_agent"), "suspended")

    @patch("sa_agent.save_autonomous_document")
    @patch("tools.task")
    def test_sa_agent_deterministic_fallback(self, mock_task, mock_save) -> None:
        # Mock subagent task to fail
        mock_task.side_effect = Exception("API rate limit exceeded")
        
        state = ITState(
            client_request="Modify database schema",
            requirements="New requirement",
            tech_spec="## 1. Database Schema & Persistence\nExisting DB spec text\n",
            code="",
            agent_report="",
            test_report="",
            devops_config="",
            analytics_report="",
            error_count=0,
            next_agent="",
            project_path="d:\\MyProject\\LangChain",
            chat_id="test_chat_sa",
            agents_plan="",
            active_tasks=["SA-Database"],
            requirements_updated=False,
            tech_spec_updated=False,
            code_updated=False,
            remaining_steps=40
        )
        
        res = sa_node(state)
        self.assertIn("tech_spec", res)
        # Even though subagent failed, tech_spec should be preserved and updated with revision history
        self.assertIn("Existing DB spec text", res["tech_spec"])
        self.assertTrue(res.get("tech_spec_updated"))

    @patch("ba_agent.save_autonomous_document")
    @patch("tools.task")
    @patch("ba_agent.invoke_llm")
    def test_ba_agent_deterministic_fallback(self, mock_invoke, mock_task, mock_save) -> None:
        # Mock LLM and subagent tasks
        mock_invoke.return_value = "v1.0 Business Requirements Draft"
        mock_task.side_effect = Exception("Connection timeout")
        
        state = ITState(
            client_request="Create a new login feature",
            requirements="",
            tech_spec="",
            code="",
            agent_report="",
            test_report="",
            devops_config="",
            analytics_report="",
            error_count=0,
            next_agent="",
            project_path="d:\\MyProject\\LangChain",
            chat_id="test_chat_ba",
            agents_plan="",
            active_tasks=["BA-GapAnalyzer", "BA-Gherkin"],
            requirements_updated=False,
            tech_spec_updated=False,
            code_updated=False,
            remaining_steps=40
        )
        
        res = ba_node(state)
        self.assertIn("requirements", res)
        self.assertIn("v1.0 Business Requirements Draft", res["requirements"])
        # Check gap analyzer fallback message is included
        self.assertIn("Gap analysis subagent failed to execute", res["requirements"])
        self.assertTrue(res.get("requirements_updated"))

if __name__ == "__main__":
    unittest.main()
