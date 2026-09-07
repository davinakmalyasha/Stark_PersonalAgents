"""
native_conv.py --- Canonical LangChain-transcript <-> OpenAI-native conversion.

Single home for the converters that were duplicated across agents:

- ``lc_to_native``: LangChain message objects -> OpenAI-native dicts
  (system/user/assistant/tool with function tool_calls). Image content-block
  lists pass through untouched.
- ``normalize_calls``: raw API tool-call payloads -> the internal
  ``{"tool", "args", "id"}`` convention used by the agent loops.

Behavior is byte-identical to the per-agent copies they replace.
"""

from __future__ import annotations

import json


def lc_to_native(messages: list) -> list[dict]:
    """LangChain transcript -> OpenAI-native dicts.

    Image content-block lists pass through untouched.
    """
    out: list[dict] = []
    for m in messages:
        kind = type(m).__name__
        content = getattr(m, "content", "")
        if kind == "SystemMessage":
            out.append({"role": "system", "content": content})
        elif kind == "HumanMessage":
            out.append({"role": "user", "content": content})
        elif kind == "AIMessage":
            entry: dict = {"role": "assistant", "content": content or ""}
            tcs = getattr(m, "tool_calls", None) or []
            if tcs:
                entry["tool_calls"] = [
                    {
                        "id": tc.get("id", ""),
                        "type": "function",
                        "function": {
                            "name": tc.get("name") or tc.get("tool", ""),
                            "arguments": json.dumps(
                                tc.get("args") or {}, separators=(",", ":")
                            ),
                        },
                    }
                    for tc in tcs
                    if isinstance(tc, dict)
                ]
            out.append(entry)
        elif kind == "ToolMessage":
            out.append(
                {
                    "role": "tool",
                    "tool_call_id": getattr(m, "tool_call_id", "") or "",
                    "name": getattr(m, "name", "") or "",
                    "content": str(content),
                }
            )
    return out


def normalize_calls(raw_calls, id_prefix: str = "call") -> list[dict]:
    """Raw API tool calls -> ``{"tool", "args", "id"}`` dicts.

    ``id_prefix`` is only used for the synthetic id fallback when the provider
    omits one (e.g. ``"gen"`` -> ``gen-1``).
    """
    calls: list[dict] = []
    for index, tc in enumerate(raw_calls or []):
        if not isinstance(tc, dict):
            continue
        name = tc.get("tool") or tc.get("name", "")
        args = tc.get("args") or {}
        if isinstance(args, str):
            try:
                args = json.loads(args or "{}")
            except Exception:
                continue
        if name and isinstance(args, dict):
            calls.append(
                {
                    "tool": name,
                    "args": args,
                    "id": tc.get("id") or f"{id_prefix}-{index + 1}",
                }
            )
    return calls
