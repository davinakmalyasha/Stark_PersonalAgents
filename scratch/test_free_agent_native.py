"""Pure tests for the FreeAgent native tool transport."""

import unittest
from types import SimpleNamespace
from unittest.mock import patch

from agents.free_agent import ProviderClient
from tools._schemas import get_native_tools


class TestFreeAgentNativeTransport(unittest.TestCase):
    def test_body_preserves_messages_and_native_tools(self):
        client = ProviderClient.__new__(ProviderClient)
        client.default_model = "test-model"
        messages = [
            {"role": "system", "content": "rules"},
            {"role": "user", "content": "inspect"},
        ]
        tools = get_native_tools("free_agent")
        body = client._build_body("ignored", messages=messages, tools=tools)

        self.assertEqual(body["messages"], messages)
        self.assertEqual(body["tools"], tools)
        self.assertNotIn("ignored", str(body["messages"]))

    def test_native_tool_calls_are_normalized_without_text_parsing(self):
        body = {
            "choices": [{
                "message": {
                    "content": None,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "run_command",
                            "arguments": '{"command":"python -V"}',
                        },
                    }],
                },
            }],
        }

        self.assertEqual(
            ProviderClient._parse_tool_calls(body),
            [{"id": "call_1", "name": "run_command", "args": {"command": "python -V"}}],
        )
        self.assertEqual(ProviderClient._parse_tool_calls({"choices": []}), [])

    def test_direct_loop_keeps_native_assistant_and_tool_messages(self):
        import agents.free_agent as free_agent_module

        class FakeAgent:
            calls = []

            def chat_with_info(self, **kwargs):
                FakeAgent.calls.append(kwargs["messages"])
                if len(FakeAgent.calls) == 1:
                    return {
                        "reply": "",
                        "tool_calls": [{
                            "id": "call_1",
                            "name": "run_command",
                            "args": {"command": "python -V"},
                        }],
                    }
                return {"reply": "done", "tool_calls": []}

        with patch.object(free_agent_module, "FreeAgent", FakeAgent), patch(
            "tools.execute_tool_detailed",
            return_value=SimpleNamespace(content="Python 3"),
        ):
            list(free_agent_module.run_freeagent_direct("inspect"))

        self.assertEqual(len(FakeAgent.calls), 2)
        follow_up = FakeAgent.calls[1]
        self.assertEqual(follow_up[-2]["role"], "assistant")
        self.assertEqual(follow_up[-2]["tool_calls"][0]["id"], "call_1")
        self.assertEqual(follow_up[-1]["role"], "tool")
        self.assertEqual(follow_up[-1]["tool_call_id"], "call_1")


if __name__ == "__main__":
    unittest.main()
