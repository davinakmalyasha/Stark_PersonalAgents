"""Single-checkpoint context management for agent transcripts.

Normal turns are append-only.  Once the shared checkpoint threshold is
reached, the harness performs one atomic rewrite: refresh the full repository
map, summarize the replaceable transcript with the dedicated free compactor,
retain a small exact tail, and rebuild the transcript once.

Native assistant tool calls and their ToolMessage results are kept together.
"""
import os
import re
import threading
from dataclasses import dataclass
from langchain_core.messages import HumanMessage, AIMessage, ToolMessage, SystemMessage
from memory.context_budget import estimate_message_tokens, estimate_messages, estimate_tokens


# ── Shared compaction policy ────────────────────────────────────────────────
#
# Keep the thresholds in one place.  The defaults assume a 1M-token provider
# context and leave room for the model response and native tool schemas.  The
# hard guard invokes the exact same checkpoint operation as the normal trigger;
# it is not a second compaction tier.

_DEFAULT_TAIL_TOKENS = 12_000
_MAX_TAIL_TOKENS = 20_000


@dataclass(frozen=True)
class CompactionPolicy:
    """One append-only/checkpoint budget shared by all agents."""

    role: str = "developer"
    checkpoint_trigger: int = 600_000
    hard_limit: int = 700_000
    tail_budget_tokens: int = _DEFAULT_TAIL_TOKENS
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

    tail = setting("TAIL_TOKENS", _DEFAULT_TAIL_TOKENS)
    tail = min(max(tail, 4_000), _MAX_TAIL_TOKENS)

    return CompactionPolicy(
        role=role,
        checkpoint_trigger=setting("CHECKPOINT_TOKENS", 600_000),
        hard_limit=setting("HARD_LIMIT", 700_000),
        tail_budget_tokens=tail,
        summary_output_tokens=setting("SUMMARY_OUTPUT_TOKENS", 4_096),
    )


_LEGACY_TOOL_NAMES = {
    "read_file", "safe_read_file", "edit_file", "batch_edit", "write_to_file",
    "replace_file_content", "multi_replace_file_content", "run_command",
    "exec_command", "search_code", "ask_user",
    "browser_act", "browser_navigate", "browser_extract", "browser_screenshot",
    "browser_click", "browser_fill", "browser_close", "template_list",
    "apply_template", "template_new", "github_apply_gate",
}
_LEGACY_TOOL_HEAD = re.compile(r"^\[([a-zA-Z0-9_-]+)\]\s*:", re.IGNORECASE)


def _is_tool_result(message) -> bool:
    """Return whether a message carries a native or legacy tool result.

    Older checkpoints encoded tool output as HumanMessage. Treating every
    HumanMessage as a tool result would let compaction rewrite tasks, nudges,
    and summaries, so legacy results are identified by their tool-labelled
    payload instead.
    """
    if isinstance(message, ToolMessage):
        return True
    if not isinstance(message, HumanMessage):
        return False
    content = _tool_result_content(message).lstrip()
    match = _LEGACY_TOOL_HEAD.match(content)
    if not match:
        return False
    label = match.group(1).lower()
    if label in _LEGACY_TOOL_NAMES:
        return True
    # Preserve compatibility with future/removed tools when an old merged
    # result contains an unmistakable tool-output marker.
    return "TOOL ERROR" in content[:500] or "[FILE]" in content[:500]


def _tool_result_content(message) -> str:
    return str(getattr(message, "content", ""))


def _is_project_map_message(message) -> bool:
    """Return whether a transcript message carries an injected map block."""
    content = str(getattr(message, "content", "") or "").lstrip()
    if not content.startswith(("<current-context>", "projectMap:", "projectMap+:")):
        return False
    return bool(re.search(r"(?m)^projectMap\+?:\s*$", content))


def _is_full_project_map_message(message) -> bool:
    """Return whether a message is a full project-map anchor."""
    content = str(getattr(message, "content", "") or "").lstrip()
    if not content.startswith(("<current-context>", "projectMap:")):
        return False
    return bool(re.search(r"(?m)^projectMap:\s*$", content))


# Teaching note appended to truncated outputs — SWE-agent pattern: tell the
# model WHY output was clipped and HOW to avoid re-dumping it.
def truncation_note(orig_len: int, cap: int) -> str:
    """Instructional suffix for truncated tool outputs."""
    return (
        f"\n[Output truncated at {cap:,} chars ({orig_len:,} original) to protect context. "
        "To retrieve specific content, use search_code / grep with a narrow pattern, "
        "or read_file with offset+limit. Do not re-dump whole files or outputs.]"
    )


# ── Tail selection: keep the newest exact messages up to a token budget ──
# The checkpoint stores a bounded exact tail after the durable summary. No
# message is rewritten or pruned before that single checkpoint.

def select_tail_indices(
    messages: list,
    budget_tokens: int,
    keep_recent_turns: int = 2,
    min_keep_messages: int = 3,
) -> set:
    """
    Pick the indices to KEEP from the tail of the conversation.
    Walks backward turn by turn (each AIMessage starts a new turn); keeps whole
    turns while they fit in the budget; if the last turn overflows the remaining
    budget, partially keeps its most recent messages. Falls back to the last
    min_keep_messages when the budget would keep too little.
    """
    if not messages:
        return set()
    if len(messages) == 1:
        return {0}
    body_start = 1  # the frozen SystemMessage is never part of the tail
    if budget_tokens <= 0:
        budget_tokens = sum(
            estimate_message_tokens(message)
            for message in messages[max(body_start, len(messages) - min_keep_messages):]
        )

    # Segment into turns scanning backward: a turn ends at an AIMessage
    turns = []  # (start_idx, end_idx, tokens) in forward order
    cur = []
    for i in range(len(messages) - 1, body_start - 1, -1):
        cur.append(i)
        if isinstance(messages[i], AIMessage):
            toks = sum(estimate_message_tokens(messages[j]) for j in cur)
            turns.append((cur[-1], cur[0] + 1, toks))
            cur = []
    if cur:
        toks = sum(estimate_message_tokens(messages[j]) for j in cur)
        turns.append((cur[-1], cur[0] + 1, toks))
    turns.reverse()

    keep: list[int] = []
    total = 0
    for start, end, toks in reversed(turns):
        if total + toks <= budget_tokens:
            keep.extend(range(start, end))
            total += toks
        else:
            # Partial: fit as many trailing messages of this turn as possible
            remaining = budget_tokens - total
            idx = end - 1
            while idx >= start and remaining > 0:
                t = estimate_message_tokens(messages[idx])
                if t <= remaining:
                    keep.append(idx)
                    remaining -= t
                idx -= 1
            break

    keep_set = _bound_tail_indices(
        messages,
        _expand_tool_protocol_indices(messages, set(keep)),
        budget_tokens,
    )
    if len(keep_set) < min_keep_messages:
        keep_set = _bound_tail_indices(
            messages,
            _expand_tool_protocol_indices(
                messages,
                set(range(max(body_start, len(messages) - min_keep_messages), len(messages))),
            ),
            budget_tokens,
        )
    return keep_set


def _expand_tool_protocol_indices(messages: list, indices: set[int]) -> set[int]:
    """Keep native assistant-tool-result units together during tail selection.

    Providers expect an assistant tool-call message to be immediately followed
    by its ToolMessage results. A token boundary may land inside that unit, so
    expand the selected set to include the owning AIMessage and all adjacent
    ToolMessages. This only adds a few protocol tokens and prevents malformed
    histories after compaction.
    """
    expanded = {i for i in indices if 0 <= i < len(messages)}
    for index in list(expanded):
        message = messages[index]
        if isinstance(message, ToolMessage):
            owner = index - 1
            while owner >= 0 and isinstance(messages[owner], ToolMessage):
                owner -= 1
            if owner >= 0 and isinstance(messages[owner], AIMessage):
                expanded.add(owner)
                cursor = owner + 1
                while cursor < len(messages) and isinstance(messages[cursor], ToolMessage):
                    expanded.add(cursor)
                    cursor += 1
        elif isinstance(message, AIMessage) and getattr(message, "tool_calls", None):
            cursor = index + 1
            while cursor < len(messages) and isinstance(messages[cursor], ToolMessage):
                expanded.add(cursor)
                cursor += 1
    return expanded


def _bound_tail_indices(messages: list, indices: set[int], budget_tokens: int) -> set[int]:
    """Keep the newest complete protocol units within the tail budget.

    Tool-call assistants and their adjacent ToolMessages are indivisible. If
    one such unit is itself larger than the cap, it is retained intact because
    dropping half of a native round would make the transcript invalid; normal
    insertion-time tool offloading keeps that case rare.
    """
    selected = {i for i in indices if i > 0 and i < len(messages)}
    if not selected or budget_tokens <= 0:
        return selected

    def unit_for(index: int) -> set[int]:
        message = messages[index]
        if isinstance(message, ToolMessage):
            owner = index - 1
            while owner >= 0 and isinstance(messages[owner], ToolMessage):
                owner -= 1
            if owner >= 0 and isinstance(messages[owner], AIMessage):
                unit = {owner}
                cursor = owner + 1
                while cursor < len(messages) and isinstance(messages[cursor], ToolMessage):
                    unit.add(cursor)
                    cursor += 1
                return unit
        if isinstance(message, AIMessage) and getattr(message, "tool_calls", None):
            unit = {index}
            cursor = index + 1
            while cursor < len(messages) and isinstance(messages[cursor], ToolMessage):
                unit.add(cursor)
                cursor += 1
            return unit
        return {index}

    groups: list[set[int]] = []
    seen: set[int] = set()
    for index in sorted(selected):
        if index in seen:
            continue
        group = unit_for(index) & selected
        if not group:
            continue
        groups.append(group)
        seen.update(group)

    total = sum(estimate_message_tokens(messages[index]) for index in selected)
    while groups and total > budget_tokens and len(groups) > 1:
        oldest = groups.pop(0)
        selected.difference_update(oldest)
        total -= sum(estimate_message_tokens(messages[index]) for index in oldest)
    return selected


def _latest_full_project_map_index(messages: list) -> int | None:
    """Return the newest full map index, excluding the frozen system message."""
    full_indices = [
        index
        for index, message in enumerate(messages[1:], start=1)
        if _is_full_project_map_message(message)
    ]
    return max(full_indices) if full_indices else None


def strip_thinking(text: str) -> str:
    """Remove <thinking>...</thinking> blocks from an AI message."""
    if not text:
        return text
    return re.sub(r"<thinking>.*?</thinking>", "", text, flags=re.DOTALL)


# ── Single checkpoint compaction ────────────────────────────────────────────

class CheckpointCompactionError(RuntimeError):
    """Raised when a checkpoint cannot be committed without losing context."""


_CHECKPOINT_LOCKS: dict[str, threading.Lock] = {}
_CHECKPOINT_LOCKS_GUARD = threading.Lock()


def _checkpoint_lock(workspace_path: str | None, chat_id: str | None) -> threading.Lock:
    key = f"{os.path.abspath(workspace_path or '.')}: {chat_id or '-'}"
    with _CHECKPOINT_LOCKS_GUARD:
        lock = _CHECKPOINT_LOCKS.get(key)
        if lock is None:
            lock = threading.Lock()
            _CHECKPOINT_LOCKS[key] = lock
        return lock


def build_structured_resume_summary(tool_call_log: list, created: list, modified: list) -> str:
    """Build exact harness facts to append to the semantic checkpoint."""
    fields = []

    def compact_values(values, limit: int = 2_000) -> str:
        unique = list(dict.fromkeys(str(value) for value in (values or []) if str(value).strip()))
        value = ", ".join(unique)
        if len(value) <= limit:
            return value
        return value[:limit].rsplit(",", 1)[0] + ", …"

    artifact_parts = []
    if created:
        artifact_parts.append(f"created={compact_values(created)}")
    if modified:
        artifact_parts.append(f"modified={compact_values(modified)}")
    if artifact_parts:
        fields.append("artifacts: " + "; ".join(artifact_parts))

    tool_types_seen = {}
    for entry in tool_call_log or []:
        if not isinstance(entry, dict):
            continue
        tool_name = str(entry.get("tool", "")).strip()
        if tool_name:
            tool_types_seen[tool_name] = tool_types_seen.get(tool_name, 0) + 1
    if tool_types_seen:
        activity = ", ".join(f"{name}×{count}" for name, count in tool_types_seen.items())
        fields.append(f"facts: tool_activity={activity}")

    return "\n".join(fields)


_FREE_COMPACTION_SYSTEM = """You are a generic transcript compactor inside an agent harness.
You are not the primary agent and must not greet the user or take actions.
Summarize the supplied chronological transcript into durable state. Preserve
user requirements and constraints, decisions, completed outcomes, exact useful
errors, verification evidence, artifact identifiers, unresolved work, and next
actions. Do not invent facts, repeat system rules, include source code, or dump
tool output. The workspace can be coding, household, research, or general.
Return only this compact format, omitting empty fields and keeping each field
brief. Aim for at most 2,048 output tokens and never exceed the requested cap:
[COMPACTION]
goal:
requirements:
state: done|in_progress|blocked|unknown
completed:
decisions:
facts:
artifacts:
failures:
verification:
open:
next:
[/COMPACTION]"""


def _message_role(message) -> str:
    if isinstance(message, AIMessage):
        return "assistant"
    if isinstance(message, ToolMessage) or _is_tool_result(message):
        return "tool"
    if isinstance(message, HumanMessage):
        return "user"
    if isinstance(message, SystemMessage):
        return "system"
    return "message"


def _message_content_text(message) -> str:
    content = getattr(message, "content", "")
    if isinstance(content, (list, tuple, dict)):
        try:
            content = __import__("json").dumps(content, ensure_ascii=False, default=str)
        except Exception:
            content = str(content)
    content = str(content or "")
    if isinstance(message, AIMessage):
        content = strip_thinking(content)
    return content.strip() or "(empty)"


def _render_compactor_message(index: int, message) -> str:
    """Render a complete message without duplicating native tool schemas."""
    role = _message_role(message)
    content = _message_content_text(message)
    lines = [f"[{index} {role}]", content]
    calls = getattr(message, "tool_calls", None) or []
    if calls:
        try:
            encoded = __import__("json").dumps(calls, ensure_ascii=False, default=str, separators=(",", ":"))
        except Exception:
            encoded = str(calls)
        lines.insert(1, f"tool_calls={encoded}")
    if isinstance(message, ToolMessage):
        tool_name = getattr(message, "name", None)
        call_id = getattr(message, "tool_call_id", None)
        if tool_name or call_id:
            lines.insert(1, f"tool={tool_name or '-'} call_id={call_id or '-'}")
    return "\n".join(lines)


def _compactor_records(messages: list) -> list[str]:
    """Return replaceable transcript records, excluding stable system/map data."""
    records = []
    for index, message in enumerate(messages or []):
        if isinstance(message, SystemMessage):
            continue
        # The current full projectMap is rebuilt independently and must not be
        # duplicated inside the history summary.
        if _is_full_project_map_message(message):
            continue
        records.append(_render_compactor_message(index, message))
    return records


def _split_compactor_records(records: list[str], budget_tokens: int) -> list[str]:
    """Split records without dropping a long user/tool message."""
    budget_tokens = max(int(budget_tokens), 1_000)
    max_chars = budget_tokens * 3
    chunks: list[str] = []
    current: list[str] = []
    current_tokens = 0
    for record in records:
        pieces = [record]
        if estimate_tokens(record) > budget_tokens:
            pieces = [record[start:start + max_chars] for start in range(0, len(record), max_chars)]
        for piece in pieces:
            piece_tokens = estimate_tokens(piece)
            if current and current_tokens + piece_tokens > budget_tokens:
                chunks.append("\n\n".join(current))
                current = []
                current_tokens = 0
            current.append(piece)
            current_tokens += piece_tokens
    if current:
        chunks.append("\n\n".join(current))
    return chunks or ["(no replaceable transcript)"]


def _cap_compaction_summary(summary: str, max_chars: int = 12_000) -> str | None:
    """Normalize and cap a compactor response before storing it in history."""
    value = re.sub(r"```(?:text|markdown)?", "", str(summary or ""), flags=re.IGNORECASE)
    value = value.replace("```", "").strip()
    if not value:
        return None
    # Normalize even a partially formed model response so the durable
    # checkpoint always has exactly one bounded canonical envelope.
    if "[COMPACTION]" in value:
        value = value.split("[COMPACTION]", 1)[1]
    if "[/COMPACTION]" in value:
        value = value.split("[/COMPACTION]", 1)[0]
    body = value.strip()
    if not body:
        return None

    opening = "[COMPACTION]\n"
    closing = "\n[/COMPACTION]"
    max_body = max(1, int(max_chars) - len(opening) - len(closing))
    if len(body) > max_body:
        body = body[:max_body].rsplit("\n", 1)[0].rstrip()
    normalized = opening + body + closing
    return normalized if len(body) >= 20 else None


def _compactor_call(
    agent,
    payload: str,
    policy: CompactionPolicy,
    context_sensitive: bool = True,
    abort_check=None,
) -> str:
    """Call the dedicated free compactor with context-aware routing."""
    from agents.free_agent import ContextWindowExceededError

    required_context = (
        estimate_tokens(_FREE_COMPACTION_SYSTEM)
        + estimate_tokens(payload)
        + policy.summary_output_tokens
        + 1_024
    )
    try:
        reply = agent.chat(
            payload,
            system_prompt=_FREE_COMPACTION_SYSTEM,
            capability="text",
            temperature=0.1,
            max_tokens=policy.summary_output_tokens,
            min_context_tokens=required_context,
            prefer_context=True,
            retry_forever=True,
            context_sensitive=context_sensitive,
            abort_check=abort_check,
        )
    except ContextWindowExceededError:
        raise
    normalized = _cap_compaction_summary(
        reply,
        max_chars=max(8_000, policy.summary_output_tokens * 3),
    )
    if not normalized:
        raise CheckpointCompactionError("The free compactor returned an empty summary.")
    return normalized


def _reduce_compactor_summaries(
    agent,
    summaries: list[str],
    policy: CompactionPolicy,
    max_context: int,
    abort_check=None,
) -> str:
    """Reduce chronological partial states until one canonical checkpoint remains."""
    current = list(summaries)
    reserve = estimate_tokens(_FREE_COMPACTION_SYSTEM) + policy.summary_output_tokens + 1_024
    merge_prefix = (
        "Merge these chronological partial checkpoint states. Earlier entries "
        "establish context; later entries supersede them when they conflict. "
        "Return the same canonical [COMPACTION] format.\n\n"
    )
    group_budget = max(1_000, max_context - reserve - estimate_tokens(merge_prefix))
    while len(current) > 1 or estimate_tokens("\n\n---\n\n".join(current)) + reserve > max_context:
        groups: list[list[str]] = []
        group: list[str] = []
        group_tokens = 0
        for summary in current:
            summary_tokens = estimate_tokens(summary)
            if group and group_tokens + summary_tokens > group_budget:
                groups.append(group)
                group = []
                group_tokens = 0
            group.append(summary)
            group_tokens += summary_tokens
        if group:
            groups.append(group)
        next_round = []
        for group in groups:
            payload = merge_prefix + "\n\n---\n\n".join(group)
            next_round.append(_compactor_call(agent, payload, policy, abort_check=abort_check))
        if len(next_round) == len(current) and len(next_round) > 1:
            # A pathological provider/model limit should not create an endless
            # local reduction loop. Each group call already produced a valid
            # compact state, so the final call can merge the bounded results.
            break
        current = next_round
    if not current:
        raise CheckpointCompactionError("No partial compactor state was produced.")
    if len(current) == 1:
        return current[0]
    payload = (
        "Produce the final durable checkpoint by merging these chronological "
        "partial states. Preserve requirements, decisions, failures, verification, "
        "open work, and next actions. Return only the canonical format.\n\n"
        + "\n\n---\n\n".join(current)
    )
    return _compactor_call(agent, payload, policy, abort_check=abort_check)


def _build_free_compaction_summary(messages: list, policy: CompactionPolicy, abort_check=None) -> str:
    """Summarize the full replaceable transcript with the free-only agent."""
    records = _compactor_records(messages)
    if not records:
        return "[COMPACTION]\nstate: unknown\n[/COMPACTION]"

    from agents.free_agent import FreeAgent

    agent = FreeAgent()
    serialized = "\n\n".join(records)
    reserve = estimate_tokens(_FREE_COMPACTION_SYSTEM) + policy.summary_output_tokens + 1_024
    capacities = agent.available_model_contexts("text")
    if not capacities:
        raise CheckpointCompactionError("No configured free text model is available for compaction.")
    max_context = max(capacities)

    full_prefix = (
        "Summarize the complete chronological transcript below. The current full "
        "project map is managed separately; do not reproduce it.\n\n"
    )
    full_payload = full_prefix + serialized
    if estimate_tokens(full_payload) + reserve <= max_context:
        try:
            return _compactor_call(agent, full_payload, policy, abort_check=abort_check)
        except Exception as exc:
            # A provider can advertise a catalog context that rejects the real
            # serialized request. Fall through to chunking for that case; other
            # provider failures are retried indefinitely by FreeAgent.
            from agents.free_agent import ContextWindowExceededError
            if not isinstance(exc, ContextWindowExceededError):
                raise

    chunk_prefix = (
        "Summarize chronological transcript chunk 1/1. Return a partial "
        "[COMPACTION] state; do not omit requirements, decisions, failures, "
        "verification, or next actions present in this chunk.\n\n"
    )
    chunk_budget = max_context - reserve - estimate_tokens(chunk_prefix)
    if chunk_budget < 8_000:
        raise CheckpointCompactionError("Configured free models leave no usable compaction context.")
    from agents.free_agent import ContextWindowExceededError

    while True:
        chunks = _split_compactor_records(records, chunk_budget)
        partials = []
        try:
            for position, chunk in enumerate(chunks, start=1):
                payload = (
                    f"Summarize chronological transcript chunk {position}/{len(chunks)}. "
                    "Return a partial [COMPACTION] state; do not omit requirements, decisions, "
                    "failures, verification, or next actions present in this chunk.\n\n"
                    + chunk
                )
                partials.append(_compactor_call(agent, payload, policy, abort_check=abort_check))
        except ContextWindowExceededError:
            # Providers sometimes advertise a rounded context size. Retry the
            # chunk pass with a smaller payload, while still committing only
            # one final Developer transcript rewrite.
            if chunk_budget <= 8_000:
                raise
            chunk_budget = max(8_000, chunk_budget // 2)
            continue
        break
    return _reduce_compactor_summaries(agent, partials, policy, max_context, abort_check=abort_check)


def _is_compaction_message(message) -> bool:
    return "[COMPACTION]" in str(getattr(message, "content", "") or "")


def _full_map_message(content: str | None) -> HumanMessage | None:
    value = str(content or "").strip()
    if not value:
        return None
    if value.startswith("<current-context>"):
        return HumanMessage(content=value)
    return HumanMessage(content=f"<current-context>\n{value}\n</current-context>")


def checkpoint_compact(
    messages: list,
    tool_call_log: list | None = None,
    created: list | None = None,
    modified: list | None = None,
    keep_last_n: int = 12,
    tail_budget_tokens: int = _DEFAULT_TAIL_TOKENS,
    policy: CompactionPolicy | None = None,
    full_project_map: str | None = None,
    summary_text: str | None = None,
) -> list:
    """Build the one committed checkpoint message layout.

    This pure rebuild function does not refresh the filesystem or call an LLM;
    :func:`compact_history` performs those preparatory steps before invoking it.
    Any old full map and every old ``projectMap+`` event are removed from the
    committed list when a new full map is supplied.
    """
    if not messages or len(messages) <= 2:
        return messages
    policy = policy or get_compaction_policy()
    if tail_budget_tokens:
        tail_budget_tokens = min(max(int(tail_budget_tokens), 4_000), _MAX_TAIL_TOKENS)
        tail_indices = select_tail_indices(messages, tail_budget_tokens)
    else:
        tail_indices = _expand_tool_protocol_indices(
            messages,
            set(range(max(1, len(messages) - max(1, keep_last_n)), len(messages))),
        )
    tail_indices = {index for index in tail_indices if index > 0}
    if not tail_indices:
        return messages

    latest_full_map = _latest_full_project_map_index(messages)
    map_content = full_project_map
    if not map_content and latest_full_map is not None:
        map_content = str(getattr(messages[latest_full_map], "content", "") or "")
    map_message = _full_map_message(map_content)

    structural = build_structured_resume_summary(
        tool_call_log or [],
        created or [],
        modified or [],
    )
    summary = _cap_compaction_summary(summary_text, max_chars=max(8_000, policy.summary_output_tokens * 3))
    if not summary:
        summary = "[COMPACTION]\nstate: history_compacted\n[/COMPACTION]"
    if structural:
        summary = _cap_compaction_summary(
            summary.replace("\n[/COMPACTION]", "") + "\n" + structural,
            max_chars=max(8_000, policy.summary_output_tokens * 3),
        ) or summary

    retained_tail = []
    for index in sorted(tail_indices):
        message = messages[index]
        # The refreshed full map supersedes every old full/delta map event.
        if _is_project_map_message(message) or _is_compaction_message(message):
            continue
        retained_tail.append(message)

    compacted = []
    if messages:
        compacted.append(messages[0])
    if map_message is not None:
        compacted.append(map_message)
    compacted.append(HumanMessage(content=summary))
    compacted.extend(retained_tail)
    return compacted


def _refresh_checkpoint_map(workspace_path: str | None) -> str | None:
    if not workspace_path:
        return None
    try:
        from memory.memory_manager import refresh_code_map_context
        result = refresh_code_map_context(
            workspace_path,
            force_full=True,
            include_cached_full=True,
        )
    except Exception as exc:
        raise CheckpointCompactionError(f"Full projectMap refresh failed: {exc}") from exc
    content = str(result.get("content") or "").strip() if isinstance(result, dict) else ""
    if not content.startswith("projectMap:"):
        raise CheckpointCompactionError("Full projectMap refresh returned no valid snapshot.")
    return content


def compact_history(
    messages: list,
    tool_call_log: list | None = None,
    created: list | None = None,
    modified: list | None = None,
    policy: CompactionPolicy | None = None,
    workspace_path: str | None = None,
    chat_id: str | None = None,
    projected_tokens: int | None = None,
    abort_check=None,
) -> tuple[list, dict]:
    """Append below the checkpoint threshold; otherwise commit one checkpoint.

    No old message is touched below the normal trigger. The 700K guard uses the
    same code path as the 600K trigger and exists only to catch a large
    projected addition before the next model call.
    """
    policy = policy or get_compaction_policy()
    before = estimate_messages(messages)
    projected = max(before, int(projected_tokens or 0))
    due = projected >= policy.checkpoint_trigger
    if not due or len(messages) <= 2:
        return messages, {
            "checkpoint": False,
            "trigger": None,
            "before": before,
            "after": before,
            "map_refreshed": False,
        }

    trigger = "hard_guard" if projected >= policy.hard_limit else "checkpoint"
    lock = _checkpoint_lock(workspace_path, chat_id)
    with lock:
        # Another caller may have compacted while this invocation was waiting.
        # The local list is still the caller's snapshot, so only avoid a second
        # operation when it is already below the threshold.
        current_before = estimate_messages(messages)
        if current_before < policy.checkpoint_trigger and projected < policy.hard_limit:
            return messages, {
                "checkpoint": False,
                "trigger": None,
                "before": current_before,
                "after": current_before,
                "map_refreshed": False,
            }

        full_map = _refresh_checkpoint_map(workspace_path)
        semantic = _build_free_compaction_summary(messages, policy, abort_check=abort_check)
        rebuilt = checkpoint_compact(
            messages,
            tool_call_log=tool_call_log or [],
            created=created or [],
            modified=modified or [],
            tail_budget_tokens=policy.tail_budget_tokens,
            policy=policy,
            full_project_map=full_map,
            summary_text=semantic,
        )
        return rebuilt, {
            "checkpoint": rebuilt is not messages,
            "trigger": trigger,
            "before": current_before,
            "after": estimate_messages(rebuilt),
            "map_refreshed": bool(full_map),
        }


# ─────────────────────────────────────────────────────────────────────────────
