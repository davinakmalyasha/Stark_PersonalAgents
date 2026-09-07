"""One non-blocking loop controller for tool-using agents.

The controller is intentionally advisory. Coding agents may spend hours
reading, testing, or retrying different approaches, so broad read/write ratios,
stale-read interception, iteration ceilings, and automatic aborts do not belong
in the execution loop. Only an exact consecutive repeat receives a compact
warning; the tool still executes and the model decides what to do next.
"""

from __future__ import annotations

import json
from typing import Any, Dict, List, Optional


def _canonical_args(args: Any) -> str:
    """Return a stable signature for one tool call."""
    try:
        return json.dumps(args or {}, sort_keys=True, separators=(",", ":"), default=str)
    except Exception:
        return repr(args)


class LoopGuard:
    """Advisory exact-repeat detector — byte-identical tool+args only, never tool name alone.

    Triggers only when the same tool with byte-identical args (json dumps
    sort_keys) repeats >5 times consecutively (6th call). run_command with
    different commands are different signatures and don't trigger.
    No max_turns / iteration ceiling — purely advisory nudge.
    """

    warning_after = 6
    warning_interval = 3

    @classmethod
    def check_pre_execute(
        cls,
        tool_call_log: List[Dict[str, Any]],
        tool_name: str,
        tool_args: Dict[str, Any],
    ) -> Optional[tuple[str, str]]:
        """Warn about byte-identical consecutive repeats (>5), never abort them."""
        signature = (tool_name, _canonical_args(tool_args))
        repeats = 0
        for item in reversed(tool_call_log):
            item_signature = (
                item.get("tool", ""),
                _canonical_args(item.get("args", {})),
            )
            if item_signature != signature:
                break
            repeats += 1

        call_number = repeats + 1
        if call_number < cls.warning_after:
            return None
        if (call_number - cls.warning_after) % cls.warning_interval:
            return None

        return (
            "WARNING",
            f"[STALL] byte-identical call #{call_number} for {tool_name} "
            f"with args {_canonical_args(tool_args)[:300]} — you have repeated the exact same "
            "tool call >5 times without variation. Change the args, try a different tool, or "
            "summarize progress. Choose a different action next turn.",
        )

    @classmethod
    def has_byte_identical_stall(cls, tool_call_log: List[Dict[str, Any]], threshold: int = 6) -> Optional[str]:
        """Check trailing log for any byte-identical stall of >=threshold consecutive calls.

        Returns warning string if stalled, else None. Used by native-agent path
        where tools ran inside pydantic-ai and check_pre_execute wasn't called per-tool.
        """
        if len(tool_call_log) < threshold:
            return None
        # only need to test the last signature's run length
        last = tool_call_log[-1]
        sig = (last.get("tool", ""), _canonical_args(last.get("args", {})))
        repeats = 0
        for item in reversed(tool_call_log):
            if (item.get("tool", ""), _canonical_args(item.get("args", {}))) != sig:
                break
            repeats += 1
            if repeats >= threshold:
                return (
                    f"[STALL] byte-identical {sig[0]} repeated {repeats}× consecutively "
                    f"(args {_canonical_args(last.get('args', {}))[:300]}). "
                    "Vary the input or switch tools — repeating the identical call will not advance the task."
                )
        return None


__all__ = ["LoopGuard"]
