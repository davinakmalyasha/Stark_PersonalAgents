import unittest
from unittest.mock import patch, MagicMock
import os
import json
import sys

# Ensure project root is in python path
sys.path.insert(0, r"D:\MyProject\LangChain")

import langchain_openai.chat_models.base as base_mod
from langchain_core.messages import AIMessage, SystemMessage, HumanMessage
from llm import invoke_messages_with_fallback
from developer_agent import developer_node
from it_department_nodes_base import ITState

class TestReasoningPreservation(unittest.TestCase):
    
    def test_monkeypatch_serialization(self):
        # Create an AIMessage with reasoning_content in additional_kwargs
        msg = AIMessage(content="test content", additional_kwargs={"reasoning_content": "test reasoning"})
        
        # Verify it converts message including reasoning_content
        d = base_mod._convert_message_to_dict(msg)
        self.assertEqual(d.get("role"), "assistant")
        self.assertEqual(d.get("content"), "test content")
        self.assertEqual(d.get("reasoning_content"), "test reasoning")

    @patch("llm.ChatOpenAI")
    def test_invoke_returns_reasoning(self, mock_chat_openai):
        # Set up mock client response
        mock_choice = MagicMock()
        mock_choice.message = MagicMock()
        mock_choice.message.content = "my final code"
        mock_choice.message.function_call = None
        mock_choice.message.tool_calls = None
        mock_choice.message.audio = None
        mock_choice.message.model_extra = {}
        mock_choice.message.additional_kwargs = {"reasoning_content": "my deep thoughts"}
        
        mock_response = MagicMock()
        mock_response.choices = [mock_choice]
        mock_response.content = "my final code"  # Set directly for _extract_text
        mock_response.additional_kwargs = {"reasoning_content": "my deep thoughts"}
        mock_response.usage = MagicMock(prompt_tokens=10, completion_tokens=10, total_tokens=20)
        
        mock_client = mock_chat_openai.return_value
        mock_client.invoke.return_value = mock_response
        
        # We need tools to trigger the tuple return format in invoke_messages_with_fallback
        fake_tools = [{"type": "function", "function": {"name": "read_file"}}]
        
        # Call invoke_messages_with_fallback
        res = invoke_messages_with_fallback(
            role="Developer",
            messages=[SystemMessage(content="system instructions"), HumanMessage(content="task")],
            tools=fake_tools
        )
        
        # Verify it returned a 3-tuple including reasoning
        self.assertIsInstance(res, tuple)
        self.assertEqual(len(res), 3)
        val, tool_calls, reasoning = res
        self.assertEqual(val, "my final code")
        self.assertEqual(tool_calls, [])
        self.assertEqual(reasoning, "my deep thoughts")

    @patch("developer_agent.invoke_messages_with_fallback")
    @patch("developer_agent.execute_tool")
    @patch("developer_agent.save_task_tracking")
    @patch("developer_agent.load_task_tracking")
    @patch("developer_agent._detect_test_command")
    def test_developer_node_preserves_reasoning_and_serializes(self, mock_detect, mock_load, mock_save, mock_execute, mock_invoke):
        mock_detect.return_value = ""
        mock_execute.return_value = "[OK] Done"
        
        # Set up load_task_tracking mock to return dummy tracking info
        mock_load.return_value = {
            "current_step": 0,
            "steps": [{"description": "step 1", "status": "in_progress"}],
            "status": "in_progress"
        }
        
        # We mock save_task_tracking to capture the serialized developer_state
        serialized_state_captured = {}
        def save_side_effect(task_data, project_path, chat_id):
            if "developer_state" in task_data:
                serialized_state_captured.update(task_data["developer_state"])
        mock_save.side_effect = save_side_effect
        
        # Force the loop to run 1 iteration then raise exception on 2nd to exit
        mock_invoke.side_effect = [
            (
                "```tool\n{\"tool\": \"read_file\", \"args\": {\"file_path\": \"app.py\"}}\n```",
                [],
                "thought process 1"
            ),
            Exception("stop loop")
        ]
        
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
            chat_id="test_chat_serialization",
            agents_plan="",
            active_tasks=["developer"],
            requirements_updated=False,
            tech_spec_updated=False,
            code_updated=False,
            messages=[]
        )
        
        developer_node(state)
                
        # Check if reasoning_content is in the serialized state messages
        self.assertIn("messages", serialized_state_captured)
        serialized_msgs = serialized_state_captured["messages"]
        
        # The AI message should have additional_kwargs with reasoning_content
        ai_msg = None
        for msg in serialized_msgs:
            if msg.get("type") == "AIMessage":
                ai_msg = msg
                break
        
        self.assertIsNotNone(ai_msg)
        self.assertIn("additional_kwargs", ai_msg)
        self.assertEqual(ai_msg["additional_kwargs"].get("reasoning_content"), "thought process 1")

    @patch("developer_agent.invoke_messages_with_fallback")
    @patch("developer_agent.execute_tool")
    @patch("developer_agent.save_task_tracking")
    @patch("developer_agent.load_task_tracking")
    @patch("developer_agent._detect_test_command")
    def test_remaining_steps_decrement_and_cap(self, mock_detect, mock_load, mock_save, mock_execute, mock_invoke):
        from state_sync import shared_state, safe_update_state
        mock_detect.return_value = ""
        mock_execute.return_value = "[OK] Done"
        mock_load.return_value = None
        
        # Set remaining_steps in shared_state to 3
        safe_update_state({"remaining_steps": 3})
        
        # LLM calls
        mock_invoke.side_effect = [
            (
                "```tool\n{\"tool\": \"read_file\", \"args\": {\"file_path\": \"app.py\"}}\n```",
                [],
                "thought process 1"
            ),
            (
                "Finished changes.",
                [],
                "thought process 2"
            )
        ]
        
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
            chat_id="test_chat_steps",
            agents_plan="",
            active_tasks=["developer"],
            requirements_updated=False,
            tech_spec_updated=False,
            code_updated=False,
            messages=[]
        )
        
        res = developer_node(state)
        
        # remaining_steps should have decremented twice (from 3 to 1)
        self.assertEqual(res.get("remaining_steps"), 1)

if __name__ == "__main__":
    unittest.main()
