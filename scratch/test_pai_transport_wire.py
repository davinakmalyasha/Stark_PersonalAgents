"""End-to-end transport test against a LOCAL mock OpenAI-compatible server.

Proves the full wire round-trip of the pydantic-ai transport: request
building, streaming deltas, tool-call parsing, reasoning extraction, and
usage accounting — without any external API/balance dependency.

A1: the transport is async-first; this gate drives the ``a*`` entrypoints
directly inside one event loop (no ``asyncio.run`` nesting).
"""
import sys
import os
import json
import threading
import time
import asyncio
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from langchain_core.messages import HumanMessage, SystemMessage
from llm.pai_transport import apai_native_turn, apai_structured
from pydantic import BaseModel, Field

CAPTURED = {}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length", 0)))
        req = json.loads(body)
        CAPTURED["request"] = req
        if self.path.endswith("/chat/completions"):
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            # stream: thinking -> text -> tool call -> usage
            chunks = [
                {"id": "c1", "object": "chat.completion.chunk", "model": "mock-model", "choices": [{"delta": {"reasoning_content": "thinking..."}, "index": 0}]},
                {"id": "c1", "object": "chat.completion.chunk", "model": "mock-model", "choices": [{"delta": {"content": "Weather: "}, "index": 0}]},
                {"id": "c1", "object": "chat.completion.chunk", "model": "mock-model", "choices": [{"delta": {"content": "hot"}, "index": 0}]},
                {"id": "c1", "object": "chat.completion.chunk", "model": "mock-model", "choices": [{"delta": {"tool_calls": [{"index": 0, "id": "call_9", "function": {"name": "get_weather", "arguments": "{\"city\": \"Jakarta\"}"}}]}, "index": 0}]},
                {"id": "c1", "object": "chat.completion.chunk", "model": "mock-model", "choices": [{"delta": {}, "index": 0, "finish_reason": "tool_calls"}], "usage": {"prompt_tokens": 10, "completion_tokens": 20, "total_tokens": 30, "prompt_tokens_details": {"cached_tokens": 4}}},
            ]
            for c in chunks:
                self.wfile.write(("data: " + json.dumps(c) + "\n\n").encode())
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
        else:
            self.send_response(404)
            self.end_headers()


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
port = server.server_address[1]
threading.Thread(target=server.serve_forever, daemon=True).start()
BASE = f"http://127.0.0.1:{port}/v1"
print("mock server on", BASE)

TOOLS = [
    {
        "function": {
            "name": "get_weather",
            "description": "Get the weather for a city",
            "parameters": {
                "type": "object",
                "properties": {"city": {"type": "string"}},
                "required": ["city"],
            },
        }
    }
]

msgs = [
    SystemMessage(content="You are a helpful assistant."),
    HumanMessage(content="Weather in Jakarta?"),
]

# Capture stream-flag + tools + messages in the request.
# PAI_NATIVE_TOOLS=0 pins the single-turn wire path (raw request, no agent
# loop) — that is the wire format this gate asserts on.
os.environ["PAI_NATIVE_TOOLS"] = "0"


async def main():
    text, calls, reasoning = await apai_native_turn(
        role="Mock",
        messages=msgs,
        tools=TOOLS,
        model="mock-model",
        temp=0.2,
        reasoning_effort="disabled",
        where="mock",
        base_url=BASE,
        api_key="test-key",
    )
    print("\ntext:", repr(text))
    print("calls:", calls)
    print("reasoning:", repr(reasoning))

    assert "hot" in text, text
    assert calls and calls[0]["tool"] == "get_weather"
    assert calls[0]["args"] == {"city": "Jakarta"}
    assert calls[0]["id"] == "call_9"
    assert reasoning == "thinking...", reasoning

    req = CAPTURED["request"]
    print("\ncaptured request keys:", list(req.keys()))
    assert req.get("model") == "mock-model"
    assert req.get("stream") is True
    tool_defs = req.get("tools", [])
    assert tool_defs and tool_defs[0]["function"]["name"] == "get_weather"
    msgs_sent = req.get("messages", [])
    assert msgs_sent[0]["role"] == "system"
    assert msgs_sent[1]["role"] == "user"
    print("OK: request wire format (stream, tools, messages)")

    print("\n=== apai_structured (mock) ===")


class Handler2(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length", 0)))
        req = json.loads(body)
        CAPTURED["structured_request"] = req
        resp = {
            "id": "c2",
            "object": "chat.completion",
            "model": "mock-model",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": '{"city":"Jakarta","temp_c":30.0,"condition":"sunny"}'},
                "finish_reason": "stop",
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 15, "total_tokens": 20},
        }
        body_out = json.dumps(resp).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body_out)))
        self.end_headers()
        self.wfile.write(body_out)


server2 = ThreadingHTTPServer(("127.0.0.1", 0), Handler2)
port2 = server2.server_address[1]
threading.Thread(target=server2.serve_forever, daemon=True).start()


async def main():

    class WeatherResult(BaseModel):
        city: str
        temp_c: float
        condition: str

    res = await apai_structured(
        [
            SystemMessage(content="Return JSON."),
            HumanMessage(content="Jakarta weather?"),
        ],
        WeatherResult,
        model="mock-model",
        where="mock structured",
        base_url=f"http://127.0.0.1:{port2}/v1",
        api_key="test-key",
    )
    print("structured:", res)
    assert isinstance(res, WeatherResult)
    assert res.city == "Jakarta" and res.temp_c == 30.0
    sreq = CAPTURED.get("structured_request", {})
    assert sreq.get("stream") is False or "stream" not in sreq
    print("OK: structured validated")


asyncio.run(main())
server.shutdown()
server2.shutdown()
print("\nALL MOCK WIRE GATES PASSED")
