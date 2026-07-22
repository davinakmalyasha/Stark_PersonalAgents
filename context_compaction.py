"""
Context Compaction module.
Two-tier compaction for multi-agent context window management.

Tier 1 (Light): strip thinking blocks, deduplicate superseded file reads,
                truncate outputs >40K chars. Never destroys semantic content.
Tier 2 (Heavy): collapse everything before the last N messages into a
                structured resume summary. Used when approaching context limit.

No [TOOL OK] markers. File reads are never compacted away — the model needs
the content it learned, not a reminder that it read something.
"""
import os
import re
import uuid
from langchain_core.messages import HumanMessage, AIMessage


def _get_active_ws():
    """Import tools lazily and compute the active workspace path."""
    from tools import shared_state, WORKSPACE
    return os.path.abspath(shared_state.get("project_path") or WORKSPACE or ".")


# ── Thresholds ──
# OpenCode uses PRUNE_PROTECT=40K tokens for tool output pruning.
# We use 40K chars (~13K tokens for mixed code/text) — output smaller than
# this is ~1.3% of a 1M token window and not worth touching.
_TOOL_TRUNCATE_CHARS = 40_000

# Error/failure indicators — never truncate or compact messages containing these
_ERROR_SIGNALS = (
    "error", "failed", "traceback", "exception", "exit code: 1",
    "status: fail", "permission denied", "not found", "timeout",
    "command not found", "cannot", "denied", "stack trace",
    "syntaxerror", "indentationerror", "modulenotfounderror",
    "importerror", "attributeerror", "typeerror", "valueerror",
    "keyerror", "indexerror", "runtimeerror", "connectionerror",
)


def extract_paths_from_text(text: str) -> list[str]:
    """Helper to extract file paths from various write/edit tool result formats."""
    active_ws = _get_active_ws()

    paths = []
    # 1. Look for uncompacted write/edit markers
    for m in re.finditer(r"(?:[Ww]rote|[Ee]dited|[Aa]pplied diff to)\s+(\S+)", text):
        paths.append(m.group(1))

    # 2. Look for explicit JSON-like or Key-value properties
    for m in re.finditer(r"(?:file_path|path|TargetFile|target_file)[:=]\s*([^\s,;\"'\(\)]+)", text):
        paths.append(m.group(1))

    # 3. Look for structured summary lists
    for line in text.split("\n"):
        if line.startswith("Files Modified:") or line.startswith("Files Created:"):
            parts = line.split(":", 1)[1].split(",")
            for p in parts:
                p_clean = p.strip()
                if p_clean:
                    paths.append(p_clean)

    # Clean and resolve to absolute paths
    resolved_paths = []
    for p in paths:
        clean = p.strip("[](),;'\"")
        if clean:
            if not os.path.isabs(clean):
                abs_p = os.path.abspath(os.path.join(active_ws, clean))
            else:
                abs_p = os.path.abspath(clean)
            resolved_paths.append(os.path.normpath(abs_p).lower())
    return list(set(resolved_paths))


def invalidate_stale_reads(messages: list) -> list:
    """
    Mark read_file results as stale if the file was later modified.
    Preserves the file path metadata so the model knows to re-read.
    Does NOT destroy content — only marks it.
    """
    result = list(messages)
    modified_files: set[str] = set()
    active_ws = _get_active_ws()

    for idx in range(len(result) - 1, -1, -1):
        msg = result[idx]

        # Detect write/edit tool calls in AI messages (backwards)
        if isinstance(msg, AIMessage):
            content = msg.content if hasattr(msg, "content") else str(msg)
            tool_calls = []
            try:
                from developer_agent import parse_all_tool_calls
                tool_calls = parse_all_tool_calls(content) if content else []
            except Exception:
                pass

            for tc in tool_calls:
                tool_name = tc.get("tool", "")
                if tool_name in (
                    "write_file", "write_planning_file", "edit_file", "apply_diff",
                    "batch_edit", "write_to_file", "replace_file_content",
                    "multi_replace_file_content"
                ):
                    args = tc.get("args") or {}
                    path = (
                        args.get("file_path") or args.get("path") or
                        args.get("TargetFile") or args.get("target_file")
                    )
                    if path:
                        if not os.path.isabs(path):
                            abs_p = os.path.abspath(os.path.join(active_ws, path))
                        else:
                            abs_p = os.path.abspath(path)
                        modified_files.add(os.path.normpath(abs_p).lower())

        # Detect write/edit results in Human messages
        elif isinstance(msg, HumanMessage):
            content = msg.content if hasattr(msg, "content") else str(msg)
            content_lower = content.lower()

            is_write_edit_result = False
            tool_name = None
            name_match = re.search(r"Tool '([^']+)'|\[([a-zA-Z0-9_-]+)\]", content[:200])
            if name_match:
                tool_name = name_match.group(1) or name_match.group(2)

            write_tools = {"write_file", "write_planning_file", "edit_file", "apply_diff", "batch_edit", "write_to_file", "replace_file_content", "multi_replace_file_content"}
            if (tool_name in write_tools):
                is_write_edit_result = True

            if is_write_edit_result:
                paths = extract_paths_from_text(content)
                for p in paths:
                    modified_files.add(p)

            # Mark stale reads
            if not is_write_edit_result and "[FILE]" in content:
                m = re.search(r"\[FILE\]\s+([^\s\n]+)", content)
                if m:
                    file_path_str = m.group(1).strip()
                    if not os.path.isabs(file_path_str):
                        abs_read_p = os.path.abspath(os.path.join(active_ws, file_path_str))
                    else:
                        abs_read_p = os.path.abspath(file_path_str)
                    norm_read_path = os.path.normpath(abs_read_p).lower()

                    if norm_read_path in modified_files:
                        stale_content = f"[read_file]:\n[FILE] {file_path_str} (content stale - file was modified later)"
                        result[idx] = type(msg)(
                            content=stale_content,
                            id=getattr(msg, "id", None),
                            additional_kwargs=getattr(msg, "additional_kwargs", {})
                        )

    return result


# ── Adaptive Compaction Threshold ──



# ── Tier 1: Light Compaction ──

def tier1_compact(messages: list, keep_last_n: int = 8) -> list:
    """
    Tier 1 (Light Compaction):
      - Strips <thinking> blocks from AI messages.
      - Deduplicates superseded file reads (keeps only the latest read of each file).
      - Truncates tool outputs >40K chars in older messages.
      - Protects headers and last N messages.

    Does NOT destroy semantic content. Does NOT use [TOOL OK] markers.
    File reads are never compacted away — only superseded duplicates are removed.
    """
    header_indices = get_header_indices(messages)
    mid_idx = len(messages) - keep_last_n

    # Backwards scan to find superseded reads
    seen_reads = set()
    superseded_indices = set()
    for idx in range(len(messages) - 1, -1, -1):
        if idx in header_indices or idx >= mid_idx:
            continue
        msg = messages[idx]
        if isinstance(msg, HumanMessage):
            content = msg.content if hasattr(msg, "content") else str(msg)
            if "[FILE]" in content:
                m = re.search(r"\[FILE\]\s+([^\s\n]+)", content)
                if m:
                    file_path = os.path.normpath(m.group(1).strip()).lower()
                    if file_path in seen_reads:
                        superseded_indices.add(idx)
                    else:
                        seen_reads.add(file_path)

    compacted = []
    for i, msg in enumerate(messages):
        if i in header_indices or i >= mid_idx:
            compacted.append(msg)
            continue

        content = msg.content if hasattr(msg, "content") else str(msg)

        if isinstance(msg, AIMessage):
            # Strip thinking blocks, keep everything else
            content = strip_thinking(content)
            # Retain tool blocks if present (the actionable parts)
            tool_blocks = re.findall(r"```tool\s*\n.*?\n```", content, flags=re.DOTALL)
            if tool_blocks:
                content = "\n\n".join(tool_blocks)
            compacted.append(AIMessage(
                content=content,
                id=getattr(msg, "id", None),
                tool_calls=getattr(msg, "tool_calls", []),
                additional_kwargs=getattr(msg, "additional_kwargs", {})
            ))
            continue

        if isinstance(msg, HumanMessage):
            # Superseded read → keep the path metadata so model knows it was read
            if i in superseded_indices:
                read_match = re.search(r"\[FILE\]\s+([^\s\n]+)", content)
                file_name = read_match.group(1) if read_match else "file"
                compacted.append(HumanMessage(
                    content=f"[read_file]:\n[FILE] {file_name} (content superseded by a later read)",
                    id=getattr(msg, "id", None),
                    additional_kwargs=getattr(msg, "additional_kwargs", {})
                ))
                continue

            content_lower = content.lower()
            # Error messages stay intact
            if any(e in content_lower for e in _ERROR_SIGNALS):
                compacted.append(msg)
                continue

            # Truncate only outputs >40K chars to prevent context overflow
            if len(content) > _TOOL_TRUNCATE_CHARS:
                head = content[:2000]
                tail = content[-1000:]
                content = f"{head}\n... [{len(content)} chars total — truncated at {_TOOL_TRUNCATE_CHARS} limit] ...\n{tail}"

            compacted.append(HumanMessage(
                content=content,
                id=getattr(msg, "id", None),
                additional_kwargs=getattr(msg, "additional_kwargs", {})
            ))
            continue

        compacted.append(msg)

    return compacted


# ── Tier 2: Medium Compaction ──

def tier2_compact(messages: list, keep_last_n: int = 8) -> list:
    """
    Tier 2 (Medium Compaction):
      - Runs Tier 1 compaction first (strip thinking, dedup reads, truncate >40K).
      - For successful write/edit tool outputs: condense to one line with file path.
      - For large run_command outputs (>40K): truncate, keep small ones intact.
      - File reads are NEVER compacted — the model needs the content.

    No [TOOL OK] markers. No content destruction.
    When more aggressive compaction is needed, use checkpoint_compact instead.
    """
    # 1. Run Tier 1 first
    messages = tier1_compact(messages, keep_last_n)

    header_indices = get_header_indices(messages)
    mid_idx = len(messages) - keep_last_n

    compacted = []
    for i, msg in enumerate(messages):
        if i in header_indices or i >= mid_idx:
            compacted.append(msg)
            continue

        content = msg.content if hasattr(msg, "content") else str(msg)
        if isinstance(msg, HumanMessage):
            content_lower = content.lower()

            # Always keep error messages intact
            if any(e in content_lower for e in _ERROR_SIGNALS):
                compacted.append(msg)
                continue

            # Extract tool name
            tool_name = "tool"
            name_match = re.search(r"Tool '([^']+)'|\[([a-zA-Z0-9_-]+)\]", content[:200])
            if name_match:
                tool_name = name_match.group(1) or name_match.group(2)

            # File reads — NEVER compact. Content is knowledge, not side effect.
            if tool_name in ("read_file", "safe_read_file") or "[FILE]" in content:
                # Keep the read intact, but truncate if >40K
                if len(content) > _TOOL_TRUNCATE_CHARS:
                    file_match = re.search(r"\[FILE\]\s+(\S+)", content)
                    fname = file_match.group(1) if file_match else "file"
                    head = content[:2000]
                    tail = content[-1000:]
                    content = f"[read_file]:\n[FILE] {fname}\n{head}\n... [{len(content)} chars total — truncated] ...\n{tail}"
                compacted.append(HumanMessage(
                    content=content,
                    id=getattr(msg, "id", None),
                    additional_kwargs=getattr(msg, "additional_kwargs", {})
                ))
                continue

            # Write/edit actions — can condense since content is on disk
            action_tools = {
                "write_file", "edit_file", "apply_diff", "batch_edit",
                "write_planning_file", "write_to_file", "replace_file_content",
                "multi_replace_file_content",
            }
            if tool_name in action_tools:
                # Extract file path from the output
                fp = None
                for ptn in [r"wrote\s+(\S+)", r"edited\s+(\S+)", r"file_path['\"]?\s*[:=]\s*['\"]?([^\s,;)'\"]+)", r"path['\"]?\s*[:=]\s*['\"]?([^\s,;)'\"]+)"]:
                    m = re.search(ptn, content[:500], re.IGNORECASE)
                    if m:
                        fp = m.group(1)
                        break
                if fp:
                    content = f"[{tool_name}]:\nWrote/edited {fp} — file content is on disk."
                else:
                    # Fallback: keep first 200 chars
                    content = content[:200]
                compacted.append(HumanMessage(
                    content=content,
                    id=getattr(msg, "id", None),
                    additional_kwargs=getattr(msg, "additional_kwargs", {})
                ))
                continue

            # run_command — keep if small (<40K), truncate if large
            if tool_name in ("run_command", "exec_command"):
                if len(content) > _TOOL_TRUNCATE_CHARS:
                    head = content[:1000]
                    tail = content[-500:]
                    content = f"[{tool_name}]:\n{head}\n... [{len(content)} chars total — truncated] ...\n{tail}"
                compacted.append(HumanMessage(
                    content=content,
                    id=getattr(msg, "id", None),
                    additional_kwargs=getattr(msg, "additional_kwargs", {})
                ))
                continue

            # Everything else — keep intact, truncate if >40K
            if len(content) > _TOOL_TRUNCATE_CHARS:
                head = content[:2000]
                tail = content[-1000:]
                content = f"{head}\n... [{len(content)} chars total — truncated] ...\n{tail}"

        compacted.append(msg)
    return compacted


# ── Anchor Pivoting ──

def pivot_anchor_to_stable_checkpoint(messages: list, plan_checkpoint_idx: int) -> list:
    """
    Drop intermediate messages between start and the stable plan checkpoint,
    keeping only system + dynamic context + plan checkpoint + a compact summary.
    Called when transitioning from planning to execution phases.
    """
    if plan_checkpoint_idx <= 3 or plan_checkpoint_idx >= len(messages):
        return messages

    dropped = messages[3:plan_checkpoint_idx]
    dropped_summary_parts = []
    for m in dropped:
        c = m.content if hasattr(m, "content") else str(m)
        dropped_summary_parts.append(c[:200])

    summary_text = (
        f"[ANCHOR PIVOT] Planning phase completed. "
        f"{len(dropped)} intermediate messages compacted.\n"
        f"Summary of planning activity:\n" +
        "\n".join(f"- {s}" for s in dropped_summary_parts[:5]) +
        (f"\n... and {len(dropped_summary_parts) - 5} more" if len(dropped_summary_parts) > 5 else "")
    )

    return [messages[0], messages[1], messages[2],
            HumanMessage(content=summary_text),
            messages[plan_checkpoint_idx]] + messages[plan_checkpoint_idx + 1:]


# ── Checkpoint Compaction (Nuclear) ──

def build_structured_resume_summary(tool_call_log: list, created: list, modified: list) -> str:
    """Build structured summary preserving decisions, errors, and outcomes."""
    sections = []
    if created:
        sections.append(f"Files Created: {', '.join(created)}")
    if modified:
        sections.append(f"Files Modified: {', '.join(modified)}")

    # Extract key milestones from tool call log
    tool_types_seen = {}
    for entry in tool_call_log:
        tn = entry.get("tool", "")
        tool_types_seen[tn] = tool_types_seen.get(tn, 0) + 1

    if tool_types_seen:
        summary = ", ".join(f"{k} ({v}x)" for k, v in tool_types_seen.items())
        sections.append(f"Tool Activity: {summary}")

    return "\n".join(sections)


def checkpoint_compact(
    messages: list,
    tool_call_log: list,
    created: list,
    modified: list,
    keep_last_n: int = 12,
) -> list:
    """
    Checkpoint-Based Compaction.
    Collapses everything between the header and the last N messages into a
    structured resume summary. Preserves system prompt + task + recent N turns.

    This is the heavy compaction for when we're approaching the context limit.
    Unlike [TOOL OK] approach, the resume summary preserves dense factual
    information about what was accomplished.
    """
    if len(messages) <= keep_last_n + 2:
        return messages

    header_end = 2
    summary_msg_content = build_structured_resume_summary(tool_call_log, created, modified)

    # Preserve decisions from the last handful of messages
    decisions = []
    for msg in messages[-keep_last_n:]:
        if isinstance(msg, AIMessage):
            c = msg.content if hasattr(msg, "content") else str(msg)
            # Extract any tool calls mentioned
            call_match = re.findall(r'```tool\s*{\s*"tool":\s*"([^"]+)"', c)
            if call_match:
                decisions.append(f"Called: {', '.join(call_match[:3])}")

    if decisions:
        summary_msg_content += "\n\nRecent Actions:\n" + "\n".join(f"- {d}" for d in decisions)

    summary_msg = HumanMessage(content=f"[SYSTEM INFO] History compacted. Summary of progress so far:\n\n{summary_msg_content}")

    compacted = list(messages[:header_end])
    compacted.append(summary_msg)
    compacted.extend(messages[-keep_last_n:])
    return compacted
