"""Compaction policy (single shared budget) — P0-LLM.

The old single-checkpoint engine (select_tail_indices / checkpoint_compact /
compact_history / free-pool compactor) is deleted: agents now use
``llm.model_history.compact_history_native`` + ``llm.compaction_native``
(ProcessHistory capability). This module keeps only the shared policy knobs
plus ``truncation_note`` for clipped tool outputs.
"""
import os
import re
from dataclasses import dataclass


@dataclass(frozen=True)
class CompactionPolicy:
    """One append-only/checkpoint budget shared by all agents.

    Defaults: trigger 400k / hard gate 440k / summary 4096. Env overrides:
    COMPACTION_* or <ROLE>_COMPACTION_*.
    """

    role: str = "developer"
    checkpoint_trigger: int = 400_000
    hard_limit: int = 440_000
    summary_output_tokens: int = 4_096


def _positive_int(name: str, default: int) -> int:
    """Read a positive integer environment override without raising in a run."""
    try:
        value = int(os.environ.get(name, ""))
        return value if value > 0 else default
    except (TypeError, ValueError):
        return default


def get_compaction_policy(role: str = "developer") -> CompactionPolicy:
    """Return the shared policy, with optional global/per-role overrides.

    Settings use ``COMPACTION_*`` (or ``<ROLE>_COMPACTION_*``).
    """
    role_key = re.sub(r"[^A-Za-z0-9]", "_", str(role or "developer")).upper()

    def setting(suffix: str, default: int) -> int:
        role_name = f"{role_key}_COMPACTION_{suffix}"
        return _positive_int(role_name, _positive_int(f"COMPACTION_{suffix}", default))

    return CompactionPolicy(
        role=role,
        checkpoint_trigger=setting("CHECKPOINT_TOKENS", 400_000),
        hard_limit=setting("HARD_LIMIT", 440_000),
        summary_output_tokens=setting("SUMMARY_OUTPUT_TOKENS", 4_096),
    )


# Teaching note appended to truncated outputs — SWE-agent pattern: tell the
# model WHY output was clipped and HOW to avoid re-dumping it.
def truncation_note(orig_len: int, cap: int) -> str:
    """Instructional suffix for truncated tool outputs."""
    return (
        f"\n[Output truncated at {cap:,} chars ({orig_len:,} original) to protect context. "
        "To retrieve specific content, use search_code / grep with a narrow pattern, "
        "or read_file with offset+limit. Do not re-dump whole files or outputs.]"
    )
