"""Compaction prompt and the small set of remaining knobs.

One prompt is used for the one same-model summary. The summary is injected as
the final handoff message; project.md and the repo map are re-anchored fresh
and are never summarized.

Overrides:
    COMPACTION_PROMPT             inline prompt override
    COMPACTION_PROMPT_FILE        read the prompt from a file (wins)
    COMPACTION_SUMMARY_MAX_TOKENS 0 = uncapped summary (default)
    COMPACT_USER_MESSAGE_MAX_TOKENS retained exact user asks (default 20000)
    COMPACTION_CHARS_PER_TOKEN    conservative sizing factor (default 3.5)
"""
import os

COMPACTION_PROMPT_DEFAULT = (
    "You are performing a CONTEXT CHECKPOINT COMPACTION. Create a handoff "
    "summary for another LLM that will resume the task.\n\n"
    "Include:\n"
    "- Current progress and key decisions made\n"
    "- Important context, constraints, or user preferences\n"
    "- What remains to be done (clear next steps)\n"
    "- Any critical data, examples, or references needed to continue\n\n"
    "Be complete: another model will continue from this summary alone, so "
    "never drop file paths, symbol names, commands, or error strings. Later "
    "conversation supersedes earlier notes if they conflict. Do not summarize "
    "the project memory or repository map; they are re-injected fresh after "
    "compaction.\n\n"
    "Return EXACTLY this format:\n"
    "[SUMMARY]\n"
    "<the handoff summary>\n"
    "[PROJECT.MD]\n"
    "The COMPLETE updated '## Identity' and '## Design' bullet sections - or "
    "the single word NO_CHANGE if nothing materially changed. Never invent: "
    "record only facts already evidenced in the conversation. Keep the "
    "bullet keys (project, purpose, stack, goals / direction, theme, palette, "
    "typography, concept)."
)

# Codex's templates/compact/summary_prefix.md — frames the injected summary so
# the resuming model treats it as prior work rather than a user instruction.
SUMMARY_PREFIX = (
    "Another language model started to solve this problem and produced a "
    "summary of its thinking process. You also have access to the state of "
    "the tools that were used by that language model. Use this to build on "
    "the work that has already been done and avoid duplicating work. Here is "
    "the summary produced by the other language model, use the information "
    "in this summary to assist with your own analysis:"
)


def _env_int(name: str, default: int) -> int:
    try:
        return int(os.getenv(name, str(default)))
    except (TypeError, ValueError):
        return default


def _read_prompt_file(env_name: str) -> str:
    path = (os.getenv(env_name) or "").strip()
    if not path or not os.path.isfile(path):
        return ""
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as f:
            return f.read().strip()
    except Exception:
        return ""


def get_compaction_prompt() -> str:
    """Codex-style ``compact_prompt`` resolution: file > inline env > default."""
    return (
        _read_prompt_file("COMPACTION_PROMPT_FILE")
        or (os.getenv("COMPACTION_PROMPT") or "").strip()
        or COMPACTION_PROMPT_DEFAULT
    )


def summary_max_tokens() -> int:
    """Final summary output cap; 0 = uncapped (default)."""
    return max(0, _env_int("COMPACTION_SUMMARY_MAX_TOKENS", 0))


def chars_per_token() -> float:
    """Conservative chars/token for sizing (code is denser than prose)."""
    try:
        return max(1.0, float(os.getenv("COMPACTION_CHARS_PER_TOKEN", "3.5")))
    except (TypeError, ValueError):
        return 3.5


def retained_user_message_tokens() -> int:
    """Codex COMPACT_USER_MESSAGE_MAX_TOKENS equivalent for post-compaction."""
    return max(0, _env_int("COMPACT_USER_MESSAGE_MAX_TOKENS", 20000))
