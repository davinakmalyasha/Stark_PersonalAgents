"""Compaction prompt texts and knobs (Codex-parity + ledger pass).

Two prompts exist:

* ``LEDGER``  — pass 1 over an OLD chunk: recall-first notes that feed the
  final pass. Verbose on purpose; it is never injected into history.
* ``FINAL``   — Codex's context-checkpoint prompt + the project.md checker.
  Produces the one summary that is injected (uncapped by default).

Overrides:
    COMPACTION_PROMPT             inline final-prompt override
    COMPACTION_PROMPT_FILE        read the final prompt from a file (wins)
    COMPACTION_LEDGER_PROMPT      inline ledger-prompt override
    COMPACTION_LEDGER_PROMPT_FILE read the ledger prompt from a file (wins)

Budgets:
    COMPACTION_SUMMARY_MAX_TOKENS   0 = uncapped final summary (default)
    COMPACTION_LEDGER_MAX_TOKENS    hard cap for one ledger (default 25000)
    COMPACTION_CHUNK_MAX_TOKENS     intermediate chunk cap (default 210000)
    COMPACTION_LAST_CHUNK_MAX_TOKENS last raw chunk cap (default 200000)
    COMPACTION_SEAM_OVERLAP_TOKENS  raw tail of the previous chunk repeated at
                                    the start of the final chunk (default 2000)
    COMPACT_USER_MESSAGE_MAX_TOKENS retained exact user asks (default 20000)
    COMPACTION_RESERVE_TOKENS       per-request reasoning/output headroom
    COMPACTION_CHARS_PER_TOKEN      conservative sizing factor (default 3.5)
    COMPACTION_MODEL / COMPACTION_CONTEXT_WINDOW
"""
import os

# Codex's templates/compact/prompt.md, extended with the project.md section
# (the compactor both checks and may return an updated memory doc).
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

# Pass 1 prompt: the ledger is INTERMEDIATE only. Maximize recall so the final
# pass can compact with full detail from the old chunk. The token budget line
# is appended at request time (the engine knows the per-ledger allowance).
LEDGER_PROMPT_DEFAULT = (
    "You are the LEDGER WRITER for one older segment of a long coding-agent "
    "conversation. A later segment (raw) plus this ledger go to a FINAL "
    "summarizer that writes the handoff summary for the next engineer-agent. "
    "What you omit here is lost forever: the final summarizer never sees this "
    "segment's raw text.\n\n"
    "Rules:\n"
    "- This is a LEDGER, not a summary. Be dense and factual; prefer specific "
    "detail over brevity. Omitting a load-bearing fact is failure; wordiness "
    "is not.\n"
    "- Use only facts evidenced in the segment. Never invent. Mark uncertainty "
    "as [uncertain].\n"
    "- Keep identifiers EXACT: file paths, function/class names, signatures, "
    "config keys, env vars, command lines, test names, error strings, numbers. "
    "Quote the exact line when it matters (failing assertion, API error, user "
    "constraint).\n"
    "- Quote user instructions verbatim when they carry constraints, "
    "preferences, or acceptance criteria.\n"
    "- Later facts inside this segment supersede earlier ones; state the final "
    "state clearly.\n"
    "- Do NOT describe the repository, stack, or project identity - those are "
    "re-injected separately. No narrative, no praise, no small talk.\n"
    "- If a section truly has nothing, write NONE. NEVER echo the template "
    "with every section NONE: an all-NONE ledger is a failure (the segment "
    "always contains work, commands, or asks to record).\n\n"
    "Output exactly these sections, in this order:\n\n"
    "## SEGMENT GOALS\n"
    "Task(s) active here and status: DONE / IN PROGRESS / BLOCKED / ABANDONED.\n\n"
    "## STATE AT SEGMENT END\n"
    "The exact state when this segment ends (this bridges into the next raw "
    "segment): current task, files changed and their state, checks "
    "passing/failing, last action and result, what was about to happen next.\n\n"
    "## DECISIONS & RATIONALE\n"
    "For each: what was decided, why, alternatives rejected, evidence.\n\n"
    "## FILES & SYMBOLS\n"
    "`path - symbol - what changed and why`; new/deleted APIs, changed "
    "signatures, schema/config/env changes. Include a short exact snippet "
    "(<8 lines) only when the exact text is load-bearing (interface, schema, "
    "regex, config).\n\n"
    "## COMMANDS, TESTS & RESULTS\n"
    "Each meaningful command/run: command, PASS/FAIL, the exact key "
    "output/assertion.\n\n"
    "## ERRORS & FIXES\n"
    "Each: exact message, root cause, fix applied, status.\n\n"
    "## CONSTRAINTS & PREFERENCES\n"
    "User requirements, style rules, compatibility constraints, explicit "
    "forbiddens.\n\n"
    "## DEAD ENDS\n"
    "Approaches that failed and why - so they are not retried.\n\n"
    "## OPEN THREADS & UNCERTAINTIES\n"
    "Unfinished work, unanswered questions, assumptions needing verification.\n\n"
    "## VERBATIM USER INSTRUCTIONS\n"
    "Quotes (or exact-key paraphrase if very long) that constrain future work."
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


def get_ledger_prompt() -> str:
    return (
        _read_prompt_file("COMPACTION_LEDGER_PROMPT_FILE")
        or (os.getenv("COMPACTION_LEDGER_PROMPT") or "").strip()
        or LEDGER_PROMPT_DEFAULT
    )


def summary_max_tokens() -> int:
    """Final summary output cap; 0 = uncapped (default)."""
    return max(0, _env_int("COMPACTION_SUMMARY_MAX_TOKENS", 0))


def ledger_max_tokens() -> int:
    """Hard cap for one intermediate ledger (its output feeds the final pass)."""
    return max(2048, _env_int("COMPACTION_LEDGER_MAX_TOKENS", 25000))


def chunk_max_tokens() -> int:
    """Intermediate (ledgered) chunk cap. 210k keeps two chunks covering the
    400k trigger on a 256k compactor once the final-pass reserve is counted."""
    return max(2048, _env_int("COMPACTION_CHUNK_MAX_TOKENS", 210000))


def last_chunk_max_tokens() -> int:
    """Raw (recent) chunk cap for the final pass."""
    return max(2048, _env_int("COMPACTION_LAST_CHUNK_MAX_TOKENS", 200000))


def reserve_tokens() -> int:
    """Per-request headroom for reasoning + completion at fit-check time."""
    return max(1024, _env_int("COMPACTION_RESERVE_TOKENS", 8000))


def seam_overlap_tokens() -> int:
    """Raw tail of the last old chunk repeated at the start of the final raw
    chunk so pass 2 sees the real transition, not only the ledger's account
    of it. 0 disables."""
    return max(0, _env_int("COMPACTION_SEAM_OVERLAP_TOKENS", 2000))


def chars_per_token() -> float:
    """Conservative chars/token for chunk sizing (code is denser than prose)."""
    try:
        return max(1.0, float(os.getenv("COMPACTION_CHARS_PER_TOKEN", "3.5")))
    except (TypeError, ValueError):
        return 3.5


def retained_user_message_tokens() -> int:
    """Codex COMPACT_USER_MESSAGE_MAX_TOKENS equivalent for post-compaction."""
    return max(0, _env_int("COMPACT_USER_MESSAGE_MAX_TOKENS", 20000))
