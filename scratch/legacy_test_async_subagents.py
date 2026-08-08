import unittest
import os
import time
import threading
from unittest.mock import patch, MagicMock
from it_department_nodes_base import ITState
from tester_agent import tester_node, _BACKGROUND_TASKS, _tasks_lock
from supervisor_agent import supervisor_node

class TestAsyncSubagents(unittest.TestCase):
    def setUp(self):
        self.project_path = os.path.abspath(".")
        with _tasks_lock:
            _BACKGROUND_TASKS.clear()

    @patch("tester_agent.determine_test_command")
    @patch("tester_agent.TerminalSubAgent")
    def test_tester_node_async_flow(self, mock_terminal_class, mock_determine_cmd):
        mock_determine_cmd.return_value = ("pytest", True, False, False, False)
        
        mock_terminal = MagicMock()
        mock_terminal_class.return_value = mock_terminal
        
        event = threading.Event()
        
        def slow_run(cmd, path):
            event.wait(timeout=10)
            return {"status": "SUCCESS", "exit_code": 0, "output": "pytest passed!"}
            
        mock_terminal.run_command.side_effect = slow_run
        
        state = ITState(
            client_request="run tests",
            requirements="",
            tech_spec="",
            code="",
            agent_report="",
            test_report="",
            devops_config="",
            analytics_report="",
            error_count=0,
            next_agent="",
            project_path=self.project_path,
            chat_id="test_chat",
            agents_plan="",
            active_tasks=[],
            requirements_updated=False,
            tech_spec_updated=False,
            code_updated=False
        )
        
        # 1. First run: should spawn background task and return RUNNING
        res = tester_node(state)
        test_report = res.get("test_report", "")
        self.assertIn("STATUS: RUNNING", test_report)
        self.assertIn("Task ID: test_", test_report)
        
        import re
        task_id = re.search(r"Task ID:\s*(\S+)", test_report).group(1)
        
        # 2. Second run: still running
        state["test_report"] = test_report
        res2 = tester_node(state)
        self.assertEqual(res2["test_report"], test_report)
        
        # Unblock mock run and wait for completion
        event.set()
        time.sleep(0.2)
        
        # 3. Third run: completed
        res3 = tester_node(state)
        self.assertEqual(res3["test_report"], "STATUS: PASS\n\nAll automated checks passed.")
        self.assertEqual(res3["error_count"], 0)

    def test_supervisor_async_routing(self):
        state = ITState(
            client_request="go on",
            requirements="",
            tech_spec="",
            code="",
            agent_report="",
            test_report="STATUS: RUNNING\nTask ID: test_1234",
            devops_config="",
            analytics_report="",
            error_count=0,
            next_agent="",
            project_path=self.project_path,
            chat_id="test_chat",
            agents_plan="",
            active_tasks=[],
            requirements_updated=False,
            tech_spec_updated=False,
            code_updated=False
        )
        
        res = supervisor_node(state)
        self.assertEqual(res["next_agent"], "tester")
        self.assertEqual(res["active_tasks"], ["tester"])
        
        state["client_request"] = "how are you?"
        res2 = supervisor_node(state)
        self.assertEqual(res2["next_agent"], "suspended")
        self.assertEqual(res2["active_tasks"], [])

if __name__ == "__main__":
    unittest.main()
