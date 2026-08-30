"""Offline test: pai_transport conversion + result extraction with a fake model.

Proves the transport's LC->pai conversion, tool-call normalization, reasoning
extraction, and structured output without any network/API dependency.
"""
import sys
import os
import asyncio

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from langchain_core.messages import (
    AIMessage,
    HumanMessage,
    SystemMessage,
    ToolMessage,
)

from llm.pai_transport import (
    _lc_messages_to_pai,
    _result_to_native,
    _as_user_content,
)

# ---- 1. LC -> pai conversion ----
msgs = [
    SystemMessage(content="sys"),
    HumanMessage(content="hi"),
    AIMessage(
        content="let me check",
        tool_calls=[{"name": "get_weather", "args": {"city": "Jakarta"}, "id": "c1"}],
        additional_kwargs={"reasoning_content": "thinking text"},
    ),
    ToolMessage(content="30C", tool_call_id="c1", name="get_weather"),
]
pai = _lc_messages_to_pai(msgs)
print("converted messages:", len(pai))
assert len(pai) == 4, f"expected 4, got {len(pai)}"

# AIMessage -> ModelResponse with ThinkingPart + ToolCallPart + TextPart
resp_msg = pai[2]
print("ai kind:", resp_msg.kind)
parts = resp_msg.parts
kinds = [p.part_kind for p in parts]
print("ai parts:", kinds)
assert "thinking" in kinds, "reasoning_content must become ThinkingPart"
assert "tool-call" in kinds, "tool_calls must become ToolCallPart"
tc = [p for p in parts if p.part_kind == "tool-call"][0]
print("tool call:", tc.tool_name, tc.args, tc.tool_call_id)
assert tc.tool_name == "get_weather"
assert tc.args == {"city": "Jakarta"}
assert tc.tool_call_id == "c1"

# ToolMessage -> ToolReturnPart
tr = pai[3].parts[0]
assert tr.part_kind == "tool-return", tr.part_kind
assert tr.tool_name == "get_weather" and tr.tool_call_id == "c1" and tr.content == "30C"
print("OK: tool-return conversion")

# SystemMessage/HumanMessage
assert pai[0].parts[0].part_kind == "system-prompt"
assert pai[1].parts[0].part_kind == "user-prompt"
print("OK: system/user conversion")

# ---- 2. _result_to_native ----
from pydantic_ai.messages import ModelResponse, TextPart, ThinkingPart, ToolCallPart

fake_resp = ModelResponse(
    parts=[
        ThinkingPart(content="hmm"),
        TextPart(content="weather: "),
        ToolCallPart(tool_name="get_weather", args='{"city":"Jakarta"}', tool_call_id="c9"),
    ]
)
text, calls, reasoning = _result_to_native(fake_resp.parts, fake_resp)
print("native:", repr(text), calls, repr(reasoning))
assert "weather" in text
assert calls[0]["tool"] == "get_weather"
assert calls[0]["args"] == {"city": "Jakarta"}  # string args parsed to dict
assert calls[0]["id"] == "c9"
assert reasoning == "hmm"
print("OK: result_to_native (args string -> dict, reasoning preserved)")

# ---- 3. thinking-only fallback ----
fake_resp2 = ModelResponse(parts=[ThinkingPart(content="only thinking")])
text2, calls2, reasoning2 = _result_to_native(fake_resp2.parts, fake_resp2)
assert text2 == "only thinking" and calls2 == [] and reasoning2 == "only thinking"
print("OK: thinking-only fallback text")

# ---- 4. image content-block passthrough ----
img_content = [
    {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAA"}},
    {"type": "text", "text": "look at this"},
]
hm = HumanMessage(content=img_content)
up = _lc_messages_to_pai([hm])[0].parts[0]
assert isinstance(up.content, list) and up.content[0]["type"] == "image_url"
assert up.content[1]["text"] == "look at this"
print("OK: image content passthrough")

print("\nALL OFFLINE TRANSPORT GATES PASSED")
