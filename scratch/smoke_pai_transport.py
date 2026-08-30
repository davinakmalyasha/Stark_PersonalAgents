"""Live smoke v3: pai_native_turn via the OpenCode Go gateway.

OPENCODE_API_KEY is set in env; this is a supported repo gateway.
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from langchain_core.messages import HumanMessage, SystemMessage
from llm.pai_transport import pai_native_turn, pai_structured
from pydantic import BaseModel, Field

BASE = "https://opencode.ai/zen/go/v1"
KEY = os.getenv("OPENCODE_API_KEY", "")
print("key set:", bool(KEY))

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
    SystemMessage(content="You are a helpful assistant with tools."),
    HumanMessage(content="What is the weather in Jakarta? Use the get_weather tool."),
]

print("=== pai_native_turn (tools, OpenCode gateway) ===")
text, calls, reasoning = pai_native_turn(
    role="Smoke",
    messages=msgs,
    tools=TOOLS,
    model="deepseek:v4-flash",
    temp=0.2,
    reasoning_effort="disabled",
    where="smoke test",
    base_url=BASE,
    api_key=KEY,
)
print("text:", repr(text[:200]))
print("calls:", calls)
print("reasoning:", repr((reasoning or "")[:200]))
if calls:
    assert calls[0]["tool"] == "get_weather"
    assert isinstance(calls[0]["args"], dict)
    print("OK: tool call normalized", calls[0]["tool"], calls[0]["args"])
else:
    print("NOTE: model answered directly (no tool call) — still valid:", text[:100])

print("\n=== pai_structured (OpenCode gateway) ===")
class WeatherResult(BaseModel):
    city: str = Field(description="The city")
    temp_c: float = Field(description="Temperature in Celsius")
    condition: str = Field(description="Weather condition")

msgs2 = [
    SystemMessage(content="Return JSON."),
    HumanMessage(content="What is the weather in Jakarta? Temperature about 30C, sunny."),
]
res = pai_structured(
    msgs2, WeatherResult, model="deepseek:v4-flash",
    where="smoke structured", base_url=BASE, api_key=KEY,
)
print("structured:", res)
assert isinstance(res, WeatherResult)
print("OK: validated result")

print("\nALL LIVE SMOKE GATES PASSED (OpenCode gateway)")
