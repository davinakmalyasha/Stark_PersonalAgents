import unittest
from unittest.mock import patch, MagicMock
import sys

# Ensure project root is in python path
sys.path.insert(0, r"D:\MyProject\LangChain")

import langchain_openai.chat_models.base as base_mod
from langchain_core.messages import AIMessage, SystemMessage, HumanMessage
from llm import invoke_messages_with_fallback

class TestReasoningPreservation(unittest.TestCase):
    
    def test_monkeypatch_serialization(self):
        # Create an AIMessage with reasoning_content in additional_kwargs
        msg = AIMessage(content="test content", additional_kwargs={"reasoning_content": "test reasoning"})
        
        # Verify it converts message including reasoning_content
        d = base_mod._convert_message_to_dict(msg)
        self.assertEqual(d.get("role"), "assistant")
        self.assertEqual(d.get("content"), "test content")
        self.assertEqual(d.get("reasoning_content"), "test reasoning")

    @patch("llm.llm.ChatOpenAI")
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

if __name__ == "__main__":
    unittest.main()
