import unittest
from unittest.mock import patch, MagicMock
from langchain_core.messages import SystemMessage, HumanMessage, AIMessage, RemoveMessage
from it_department_nodes_base import merge_active_tasks, ITState
from developer_agent import developer_node

class TestCostOptimization5to8(unittest.TestCase):
    def test_merge_active_tasks_reducer(self):
        # 1. Base merge
        self.assertEqual(
            merge_active_tasks(["task1", "task2"], ["task3"]),
            ["task1", "task2", "task3"]
        )
        # 2. Duplicate removal, order preservation
        self.assertEqual(
            merge_active_tasks(["task1", "task2"], ["task1", "task3"]),
            ["task1", "task2", "task3"]
        )
        # 3. None handling
        self.assertEqual(merge_active_tasks(None, ["task1"]), ["task1"])
        self.assertEqual(merge_active_tasks(["task1"], None), ["task1"])
        
        # 4. Explicit removal via prefix '-'
        self.assertEqual(
            merge_active_tasks(["task1", "task2", "task3"], ["-task2"]),
            ["task1", "task3"]
        )
        # 5. Reset via 'CLEAR_ALL'
        self.assertEqual(
            merge_active_tasks(["task1", "task2"], ["CLEAR_ALL", "task3"]),
            ["task3"]
        )

    @patch("developer_agent.invoke_messages_with_fallback")
    @patch("developer_agent.execute_tool")
    def test_developer_node_selective_message_deletion(self, mock_execute, mock_invoke):
        # Turn 1: LLM returns read_file tool call
        t1_response = '<thinking>Need to read</thinking>\n```tool\n{"tool": "read_file", "args": {"file_path": "app.py"}}\n```'
        # Turn 2: LLM returns edit_file tool call
        t2_response = '<thinking>Need to edit</thinking>\n```tool\n{"tool": "edit_file", "args": {"file_path": "app.py", "diff": ""}}\n```'
        # Turn 3: LLM returns final response
        t3_response = "Finished changes."
        
        mock_invoke.side_effect = [t1_response, t2_response, t3_response]
        mock_execute.return_value = "[OK] Read file contents successfully"

        # Create dummy state
        state = ITState(
            client_request="Make some changes",
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
            chat_id="test_chat_selective",
            agents_plan="",
            active_tasks=["developer"],
            requirements_updated=False,
            tech_spec_updated=False,
            code_updated=False,
            messages=[]
        )

        with patch("sync_helpers.save_task_tracking") as mock_save, \
             patch("developer_agent._detect_test_command") as mock_detect_test:
            mock_detect_test.return_value = ""
            
            res = developer_node(state)
            
            self.assertIn("messages", res)
            returned_msgs = res["messages"]
            
            # The returned_msgs list should contain:
            # 0: SystemMessage
            # 1: HumanMessage (dynamic context)
            # 2: HumanMessage (task)
            # 3: AIMessage (Turn 1 response, with ID)
            # 4: HumanMessage (Turn 1 tool output, with ID)
            # 5: AIMessage (Turn 2 response, with ID)
            # 6: HumanMessage (Turn 2 tool output, with ID)
            # plus 2 RemoveMessage instances for the Turn 1 messages.
            remove_msgs = [m for m in returned_msgs if isinstance(m, RemoveMessage)]
            self.assertEqual(len(remove_msgs), 2)
            
            ai_msg_1 = returned_msgs[2]
            human_msg_1 = returned_msgs[3]
            self.assertEqual(remove_msgs[0].id, ai_msg_1.id)
            self.assertEqual(remove_msgs[1].id, human_msg_1.id)

if __name__ == "__main__":
    unittest.main()
