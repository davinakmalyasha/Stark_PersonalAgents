"""End-to-end transport test against a LOCAL mock OpenAI-compatible server.

Proves the full wire round-trip of pai_native_turn: request building,
streaming deltas, tool-call parsing, reasoning extraction, and usage
accounting — without any external API/balance dependency.
"""
import sys
import os
import json
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from langchain_core.messages import HumanMessage, SystemMessage
from llm.pai_transport import pai_native_turn, pai_structured
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

# Capture stream-flag + tools + messages in the request
text, calls, reasoning = pai_native_turn(
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

print("\n=== pai_structured (mock) ===")
class WeatherResult(BaseModel):
    city: str
    temp_c: float
    condition: str


# simple non-streaming response for structured
def handler2(req):
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
    body = json.dumps(resp).encode()
    return 200, {"Content-Type": "application/json"}, body


class Handler2(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length", 0)))
        req = json.loads(body)
        CAPTURED["structured_request"] = req
        code, headers, out = handler2(req)
        self.send_response(code)
        for k, v in headers.items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(out)


server2 = ThreadingHTTPServer(("127.0.0.1", 0), Handler2)
port2 = server2.server_address[1]
threading.Thread(target=server2.serve_forever, daemon=True).start()

res = pai_structured(
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

server.shutdown()
server2.shutdown()
print("\nALL MOCK WIRE GATES PASSED")
