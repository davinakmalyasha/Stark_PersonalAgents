"""compare_agents.py --- dual-run: registry transport vs native agent.

Runs the SAME conversation through both execution engines against a
stateful mock gateway that:
  1. first model call -> tool call get_weather(city=Jakarta)
  2. second model call (with tool result) -> final text 'Weather: hot'

Asserts both engines produce the same (text, tool_calls) outcome and the
native path actually EXECUTES the tool (real implementation via TOOL_MAP).

Usage: venv/Scripts/python scratch/compare_agents.py
"""
import json
import sys
import threading
import time
import os

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from langchain_core.messages import SystemMessage, HumanMessage
from llm.pai_transport import pai_native_turn
from llm.deps import WorkspaceDeps
import llm.pai_transport as pt

SEEN_TOOL_CALL = {"made": False}


class MockGateway(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))))
        msgs = body.get("messages", [])
        # Tool result present? pydantic-ai sends tool results as role=tool.
        has_tool_result = any(m.get("role") == "tool" for m in msgs)
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        chunks = []
        if not has_tool_result:
            SEEN_TOOL_CALL["made"] = True
            chunks = [
                {
                    "id": "1", "object": "chat.completion.chunk", "model": "m",
                    "choices": [{"delta": {"tool_calls": [
                        {"index": 0, "id": "call_wx", "type": "function",
                         "function": {"name": "get_weather", "arguments": json.dumps({"city": "Jakarta"})}}
                    ]}, "index": 0}],
                },
                {"id": "1", "object": "chat.completion.chunk", "model": "m",
                 "choices": [{"delta": {}, "index": 0, "finish_reason": "tool_calls"}]},
            ]
        else:
            chunks = [
                {"id": "1", "object": "chat.completion.chunk", "model": "m",
                 "choices": [{"delta": {"content": "Weather: "}, "index": 0}]},
                {"id": "1", "object": "chat.completion.chunk", "model": "m",
                 "choices": [{"delta": {"content": "hot"}, "index": 0}]},
                {"id": "1", "object": "chat.completion.chunk", "model": "m",
                 "choices": [{"delta": {}, "index": 0, "finish_reason": "stop"}],
                 "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}},
            ]
        for c in chunks:
            self.wfile.write(("data: " + json.dumps(c) + "\n\n").encode())
            self.wfile.flush()
            time.sleep(0.01)
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()


def main() -> int:
    # Register a test tool so the native agent can execute it via TOOL_MAP.
    import tools as _facade

    def _get_weather(city: str = "") -> str:
        return f"[OK] Weather in {city}: hot"

    _facade.get_weather = _get_weather
    _facade.TOOL_MAP["get_weather"] = _get_weather

    server = ThreadingHTTPServer(("127.0.0.1", 0), MockGateway)
    port = server.server_address[1]
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{port}/v1"
    msgs = [
        SystemMessage(content="You are a weather assistant."),
        HumanMessage(content="What's the weather in Jakarta?"),
    ]
    tools = [
        {"type": "function", "function": {
            "name": "get_weather", "description": "Get weather for a city.",
            "parameters": {"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]},
        }}
    ]
    deps = WorkspaceDeps(
        project_path=os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
        chat_id="cmp-1", role="Developer",
        llm_provider={"base": base, "key": "k", "flash": "m"},
    )
    captured = []
    orig = pt._emit_stream
    pt._emit_stream = lambda r, k, t, w: captured.append((k, t))

    # 1) Registry (noop) path — single turn, tool call surfaced, not executed.
    text1, calls1, reasoning1 = pai_native_turn(
        "Developer", msgs, tools=tools, model="deepseek:v4-flash", temp=0.2,
        where="cmp-registry", deps=deps, base_url=base, api_key="k",
    )
    print("REGISTRY path: text=%r calls=%s" % (text1, calls1))
    assert len(calls1) == 1 and calls1[0]["tool"] == "get_weather", calls1

    # 2) Native agent path — tool executed internally, final text returned.
    os.environ["PAI_NATIVE_TOOLS"] = "1"
    try:
        SEEN_TOOL_CALL["made"] = False
        captured.clear()
        text2, calls2, reasoning2 = pai_native_turn(
            "Developer", msgs, tools=tools, model="deepseek:v4-flash", temp=0.2,
            where="cmp-native", deps=deps, base_url=base, api_key="k",
        )
        print("NATIVE path: text=%r calls=%s" % (text2, calls2))
        assert "Weather: hot" in text2, text2
        assert SEEN_TOOL_CALL["made"], "native agent must have executed get_weather"

        # 3) run_native_agent full-turn — transcript slice (AIMessage +
        # ToolMessage + final AIMessage) so caller loops keep history.
        from llm.native_tools import run_native_agent
        SEEN_TOOL_CALL["made"] = False
        text3, new_lc, reasoning3 = run_native_agent(
            "Developer", msgs, tools=tools, temp=0.2, where="cmp-rna",
            deps=deps, system_prompt="You are a weather assistant.",
            stream_emit=False,
        )
        kinds = [type(m).__name__ for m in new_lc]
        print("RNA path: text=%r lc=%s" % (text3, kinds))
        assert "Weather: hot" in text3, text3
        assert "AIMessage" in kinds and "ToolMessage" in kinds, kinds
        assert SEEN_TOOL_CALL["made"], "run_native_agent must execute the tool"
    finally:
        os.environ.pop("PAI_NATIVE_TOOLS", None)
        pt._emit_stream = orig
    server.shutdown()
    print("COMPARE PASSED: registry tool-call surfaced; native executed + final text")
    return 0


if __name__ == "__main__":
    sys.exit(main())
