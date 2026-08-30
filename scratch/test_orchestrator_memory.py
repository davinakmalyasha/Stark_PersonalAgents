import unittest
import os
import shutil
import uuid
from unittest.mock import patch, MagicMock
from langchain_core.messages import HumanMessage

import workspace_manager as wm
from agents.orchestrator_agent import orchestrator_node

class TestOrchestratorMemory(unittest.TestCase):
    def setUp(self):
        # Create a temp directory inside scratch
        self.temp_ws_path = os.path.abspath(os.path.join(os.path.dirname(__file__), f"temp_ws_{uuid.uuid4().hex[:6]}"))
        os.makedirs(self.temp_ws_path, exist_ok=True)
        # Register the workspace
        self.workspace_info = wm.add_workspace(self.temp_ws_path, "Test Workspace")
        self.workspace_id = self.workspace_info["id"]

    def tearDown(self):
        # Remove workspace
        wm.remove_workspace(self.workspace_id)
        if os.path.exists(self.temp_ws_path):
            try:
                shutil.rmtree(self.temp_ws_path)
            except Exception:
                pass

    @patch("agents.orchestrator_agent.run_native_agent")
    def test_orchestrator_receives_chat_history(self, mock_invoke):
        # Create a new chat
        chat = wm.create_chat(self.temp_ws_path, "Test Chat")
        chat_id = chat["id"]

        # Append messages
        wm.add_message(self.temp_ws_path, chat_id, "user", "my name is Davin")
        wm.add_message(self.temp_ws_path, chat_id, "agent", "Nice to meet you, Davin!")

        # Setup state
        state = {
            "client_request": "whats my name?",
            "project_path": self.temp_ws_path,
            "chat_id": chat_id,
            "requirements": "",
            "tech_spec": "",
        }

        # Mock LLM to return a simple response so the orchestrator completes
        from langchain_core.messages import AIMessage

        mock_invoke.return_value = ("Your name is Davin.", [AIMessage(content="Your name is Davin.")], None)

        # Execute orchestrator_node
        res = orchestrator_node(state)

        # Retrieve the messages passed to run_native_agent
        self.assertTrue(mock_invoke.called)
        called_args, called_kwargs = mock_invoke.call_args
        messages = called_kwargs.get("messages") if "messages" in called_kwargs else called_args[0]

        # Verify messages structure
        self.assertTrue(len(messages) >= 2)
        task_msg = messages[1]
        self.assertIsInstance(task_msg, HumanMessage)

        # Verify the content has recent chat history and user request
        content = task_msg.content
        self.assertIn("## Recent Chat History", content)
        self.assertIn("[USER]: my name is Davin", content)
        self.assertIn("Nice to meet you, Davin!", content)
        self.assertIn("## User Request", content)
        self.assertIn("whats my name?", content)

if __name__ == "__main__":
    unittest.main()
