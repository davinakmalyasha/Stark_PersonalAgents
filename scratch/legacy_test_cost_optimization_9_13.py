import unittest
from unittest.mock import patch, MagicMock
from compact_exchange import list_to_compact, compact_to_list
from ba_subagents import GapAnalysisResponse, GapAnalyzerSubAgent
from it_department_nodes_base import vfs_state_wrapper, ITState
from it_department_graph import supervisor_command_node, developer_command_node, tester_command_node
from sa_agent import sa_node
from analytics_agent import analytics_node
from langgraph.types import Command

class TestCostOptimization9to13(unittest.TestCase):
    def test_compact_delimiters(self):
        # Test basic list compaction
        lst = ["a", "b", "c|d", ""]
        compact = list_to_compact(lst)
        self.assertEqual(compact, "a|||b|||c\\|d")
        
        # Test extraction
        extracted = compact_to_list(compact)
        self.assertEqual(extracted, ["a", "b", "c|d"])
        
        self.assertEqual(compact_to_list(""), [])
        self.assertEqual(list_to_compact([]), "")

    @patch("ba_subagents.invoke_with_fallback")
    def test_ba_subagent_compact_parsing(self, mock_invoke):
        # Mock LLM to return compact pipe-separated lists
        mock_response = GapAnalysisResponse(
            status="UNCLEAR",
            clarifications="what is the database?|||what is the port?",
            in_scope="backend development|||frontend widgets",
            out_of_scope="deploying to aws",
            assumptions="use sqlite"
        )
        mock_invoke.return_value = mock_response
        
        analyzer = GapAnalyzerSubAgent()
        res = analyzer.analyze_gaps("client request", "existing reqs")
        
        self.assertIn("STATUS: UNCLEAR", res)
        self.assertIn("- what is the database?", res)
        self.assertIn("- what is the port?", res)
        self.assertIn("- backend development", res)
        self.assertIn("- deploying to aws", res)

    def test_state_recursion_throttler(self):
        # Create a state with remaining_steps = 0
        state = ITState(
            client_request="req",
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
            chat_id="test_chat_recursion",
            agents_plan="",
            active_tasks=[],
            requirements_updated=False,
            tech_spec_updated=False,
            code_updated=False,
            messages=[],
            remaining_steps=0
        )
        
        # 1. Test supervisor node wrapper
        res_cmd = supervisor_command_node(state)
        self.assertEqual(res_cmd.goto, "gc")
        self.assertEqual(res_cmd.update.get("remaining_steps"), -1)
        
        # 2. Test developer node wrapper
        res_cmd = developer_command_node(state)
        self.assertEqual(res_cmd.goto, "gc")
        
        # 3. Test tester node wrapper
        res_cmd = tester_command_node(state)
        self.assertEqual(res_cmd.goto, "gc")
        
        # 4. Test vfs_state_wrapper directly
        @vfs_state_wrapper
        def dummy_node(s: ITState):
            return {"output": "success"}
            
        res_wrapped = dummy_node(state)
        self.assertIsInstance(res_wrapped, Command)
        self.assertEqual(res_wrapped.goto, "gc")
        self.assertEqual(res_wrapped.update.get("remaining_steps"), -1)

    @patch("tools.task")
    @patch("sa_agent.compile_revision_history")
    @patch("sa_agent.save_autonomous_document")
    def test_sa_node_parallel_exec(self, mock_save, mock_compile_rev, mock_task):
        mock_task.return_value = '{"table_layouts": "layout", "indexing_strategy": "index", "query_scale_rules": "rules"}'
        mock_compile_rev.return_value = "Revision history table"
        
        state = ITState(
            client_request="req",
            requirements="reqs",
            tech_spec="db: existing\napi: existing\nlayered: existing\nresilience: existing\ndesign_system: existing\nsequence: existing\necosystem: existing",
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
            active_tasks=["SA-Database", "SA-API"],
            requirements_updated=False,
            tech_spec_updated=False,
            code_updated=False,
            messages=[],
            remaining_steps=10
        )
        
        res = sa_node(state)
        self.assertTrue(res["tech_spec_updated"])
        # Verify tools.task was called for the active tasks SA-Database and SA-API
        self.assertEqual(mock_task.call_count, 2)

    @patch("tools.task")
    @patch("analytics_agent.save_autonomous_document")
    def test_analytics_node_parallel_exec(self, mock_save, mock_task):
        mock_task.side_effect = [
            '{"completed_deliverables": ["d1"], "missing_deliverables": [], "specification_gaps": []}',
            '{"performance_compliance": ["ok"], "security_compliance": [], "modularity_compliance": [], "non_compliance_findings": []}',
            '{"quality_index": "100", "efficiency_grade": "A", "dashboard_metrics": [], "improvement_recommendations": []}',
            '{"compiled_report_markdown": "Report content"}'
        ]
        
        state = ITState(
            client_request="req",
            requirements="reqs",
            tech_spec="spec",
            code="code",
            agent_report="",
            test_report="STATUS: PASS",
            devops_config="",
            analytics_report="",
            error_count=0,
            next_agent="",
            project_path="d:\\MyProject\\LangChain",
            chat_id="test_chat_analytics",
            agents_plan="",
            active_tasks=[],
            requirements_updated=False,
            tech_spec_updated=False,
            code_updated=False,
            messages=[],
            remaining_steps=10
        )
        
        res = analytics_node(state)
        self.assertIn("Report content", res["analytics_report"])
        # Verify 4 calls total: 3 concurrent (Auditor, Compliance, KPI) + 1 sequential (Reporter)
        self.assertEqual(mock_task.call_count, 4)

if __name__ == "__main__":
    unittest.main()
