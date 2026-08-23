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
    """Advisory exact-repeat detector used at one point in the tool loop."""

    warning_after = 3
    warning_interval = 8

    @classmethod
    def check_pre_execute(
        cls,
        tool_call_log: List[Dict[str, Any]],
        tool_name: str,
        tool_args: Dict[str, Any],
    ) -> Optional[tuple[str, str]]:
        """Warn about exact consecutive repeats, never abort them."""
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
            f"[LOOP WARNING] exact call #{call_number} for {tool_name} "
            "returned; execute it only if it advances the task, otherwise "
            "choose a different action.",
        )


__all__ = ["LoopGuard"]
