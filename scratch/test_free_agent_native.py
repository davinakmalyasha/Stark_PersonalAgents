"""Pure tests for the General agent's free-provider native tool transport."""

import os
import unittest
from types import SimpleNamespace
from unittest.mock import patch

from llm.free_providers import ProviderClient
from tools._schemas import get_native_tools


class TestFreeAgentNativeTransport(unittest.TestCase):
    def test_body_preserves_messages_and_native_tools(self):
        client = ProviderClient.__new__(ProviderClient)
        client.default_model = "test-model"
        messages = [
            {"role": "system", "content": "rules"},
            {"role": "user", "content": "inspect"},
        ]
        tools = get_native_tools("general")
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
        """The General engine's free backend preserves the exact native
        assistant/tool message sequence across turns."""
        import llm.free_providers as free_provider_module

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

            def stream_complete(self, **kwargs):
                return self.chat_with_info(**kwargs)

        with patch.dict(os.environ, {"GENERAL_MODEL_BACKEND": "free"}), \
             patch.object(free_provider_module, "FreeProviders", FakeAgent), patch(
            "agents.general_agent.execute_tool_detailed",
            return_value=SimpleNamespace(content="Python 3"),
        ):
            from agents.general_agent import run_general_direct

            list(run_general_direct("inspect"))

        self.assertEqual(len(FakeAgent.calls), 2)
        follow_up = FakeAgent.calls[1]
        self.assertEqual(follow_up[-2]["role"], "assistant")
        self.assertEqual(follow_up[-2]["tool_calls"][0]["id"], "call_1")
        self.assertEqual(follow_up[-1]["role"], "tool")
        self.assertEqual(follow_up[-1]["tool_call_id"], "call_1")


# ── Streaming / reasoning / fallback refinements ─────────────────────────────

def _make_client(name="kilo", model="m1"):
    """ProviderClient without env/key setup — only the pure methods run."""
    import itertools

    from llm.free_providers import ProviderClient
    client = ProviderClient.__new__(ProviderClient)
    client.name = name
    client.cfg = {"base_url": "https://unit.test/v1", "response_wrapped": False}
    client.default_model = model
    client.auth_dead = False
    client.fail_streak = 0
    client._key_cycle = itertools.cycle([""])
    return client


def _sse_response(lines):
    import io

    class _Resp(io.RawIOBase):
        def __init__(self, chunks):
            self._buf = b"".join((ln + "\n").encode() for ln in chunks)

        def readable(self):
            return True

        def read(self, n=-1):
            out, self._buf = self._buf[:n], self._buf[n:]
            return out

        def __iter__(self):
            return iter(self._buf.split(b"\n"))

        def __enter__(self):
            return self

        def __exit__(self, *a):
            return False

    return _Resp(lines)


class TestStreamingAssembly(unittest.TestCase):
    def test_stream_assembles_reasoning_fragmented_tools_and_callbacks(self):
        from llm.free_providers import ProviderClient
        from unittest.mock import patch

        client = _make_client()
        sse = [
            'data: {"choices":[{"delta":{"reasoning_content":"thinking "}}]}',
            'data: {"model":"test-model","choices":[{"delta":{"reasoning_content":"hard"}}]}',
            'data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"browser_"}}]}}]}',
            'data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\\"act"}}]}}]}',
            'data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"act","arguments":"ion\\":\\"nav\\"}"}}]}}]}',
            'data: {"choices":[{"delta":{"content":"done"},"finish_reason":"tool_calls"}]}',
            "data: [DONE]",
        ]
        tokens, reasons = [], []
        with patch("urllib.request.urlopen", return_value=_sse_response(sse)):
            info = client.stream_chat_with_info(
                messages=[{"role": "user", "content": "x"}],
                tools=[{"type": "function", "function": {"name": "browser_act"}}],
                on_token=tokens.append,
                on_reasoning=reasons.append,
            )
        self.assertEqual(info["reply"], "done")
        self.assertIn("thinking", info["reasoning"])
        self.assertIn("hard", info["reasoning"])
        self.assertEqual(info["model"], "test-model")
        self.assertEqual(tokens, ["done"])
        self.assertEqual(reasons, ["thinking ", "hard"])
        self.assertEqual(
            info["tool_calls"],
            [{"id": "c1", "name": "browser_act", "args": {"action": "nav"}}],
        )

    def test_inline_think_tags_move_to_reasoning(self):
        client = _make_client()
        raw = {
            "choices": [{"message": {
                "content": "<think>secret plan</think>The answer is 4.",
            }}],
        }
        client._post = lambda body, timeout=None: raw
        info = client.chat_with_info(messages=[{"role": "user", "content": "2+2"}])
        self.assertEqual(info["reply"], "The answer is 4.")
        self.assertIn("secret plan", info["reasoning"])


class TestFallbackRefinements(unittest.TestCase):
    def _pool_with(self, *clients):
        import llm.free_providers as fp
        pool = fp.FreeProviders.__new__(fp.FreeProviders)
        pool.providers = {c.name: c for c in clients}
        pool._last_provider = None
        return pool

    def _fake_client(self, name, model_id):
        from llm.free_providers import RateLimitTracker
        c = _make_client(name=name)
        c.model_id = model_id
        c.rate_limiter = RateLimitTracker(rpm=99)

        def chat(task="", **kw):
            result = c.next_result
            if isinstance(result, Exception):
                raise result
            return result

        c.chat = chat

        def chat_with_info(task="", **kw):
            result = c.next_result
            if isinstance(result, Exception):
                raise result
            return result

        c.chat_with_info = chat_with_info
        c.stream_chat_with_info = chat_with_info
        c.next_result = {"reply": f"answer-{name}", "tool_calls": []}
        return c

    def test_auth_error_disables_provider_for_pool_lifetime(self):
        import json
        from unittest.mock import patch
        from llm.free_providers import FreeProviderError

        a = self._fake_client("groq", "groq-model")
        a.next_result = FreeProviderError("bad key", provider="groq", status=401)
        b = self._fake_client("nvidia", "nvidia-model")

        pool = self._pool_with(a, b)
        with patch.object(pool, "_route",
                          return_value=[(a, "groq-model"), (b, "nvidia-model")]):
            info = pool.chat("", messages=[{"role": "user", "content": "x"}],
                             tools=[{"type": "function"}], capability="coding",
                             _return_info=True)
        self.assertTrue(a.auth_dead)
        self.assertEqual(json.dumps(info["reply"]), '"answer-nvidia"')
        # disabled provider is excluded from future routes
        routes = pool._route("coding")
        self.assertNotIn(a, [c for c, _ in routes])

    def test_empty_reply_falls_through_to_next_provider(self):
        from unittest.mock import patch

        a = self._fake_client("groq", "groq-model")
        a.next_result = {"reply": "", "tool_calls": []}
        b = self._fake_client("nvidia", "nvidia-model")
        b.next_result = {"reply": "real answer", "tool_calls": []}

        pool = self._pool_with(a, b)
        with patch.object(pool, "_route",
                          return_value=[(a, "groq-model"), (b, "nvidia-model")]):
            info = pool.chat("", messages=[{"role": "user", "content": "x"}],
                             tools=[{"type": "function"}], capability="coding",
                             _return_info=True)
        self.assertEqual(info["reply"], "real answer")


if __name__ == "__main__":
    unittest.main()
