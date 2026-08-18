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
from memory.context_budget import estimate_tokens


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

# Teaching note appended to truncated outputs — SWE-agent pattern: tell the
# model WHY output was clipped and HOW to avoid re-dumping it.
def truncation_note(orig_len: int, cap: int) -> str:
    """Instructional suffix for truncated tool outputs."""
    return (
        f"\n[Output truncated at {cap:,} chars ({orig_len:,} original) to protect context. "
        "To retrieve specific content, use search_code / grep with a narrow pattern, "
        "or read_file with offset+limit. Do not re-dump whole files or outputs.]"
    )


# ── Tool-Result Sidecar Offloading ──
# Tier-0 upgrades destructive stubbing to RETRIEVABLE offloading: raw output is
# saved to <ws>/.agent-cache/<run-id>/ and the in-context message becomes a
# lightweight pointer (read_file can fetch it back on demand). Zero LLM calls;
# nothing is lost — only moved to disk. SIDECAR_OFFLOAD=0 restores the old
# destructive stub behavior for A/B comparison.

_SIDECAR_SUBDIR = ".agent-cache"
_SIDECAR_MAX_AGE_HOURS = 48

def _get_sidecar_run_id() -> str:
    """Per-process run id for the sidecar subdir (one dir per harness process)."""
    run_id = getattr(_get_sidecar_run_id, "_run_id", None)
    if run_id is None:
        run_id = os.environ.get("RUN_ID") or uuid.uuid4().hex[:8]
        _get_sidecar_run_id._run_id = run_id
    return run_id


def _offload_to_sidecar(ws: str, content: str, msg_key: str, tool_label: str) -> str | None:
    """
    Write raw tool output to the workspace sidecar cache.
    Returns the workspace-relative sidecar path, or None on any failure
    (caller falls back to the destructive stub — offloading is best-effort).
    """
    try:
        run_id = _get_sidecar_run_id()
        sidecar_dir = os.path.join(ws, _SIDECAR_SUBDIR, run_id)
        os.makedirs(sidecar_dir, exist_ok=True)
        safe_key = re.sub(r"[^A-Za-z0-9_.-]", "_", str(msg_key))[:80] or "tool"
        fname = f"tool_{tool_label}_{safe_key}.log"
        fpath = os.path.join(sidecar_dir, fname)
        with open(fpath, "w", encoding="utf-8", errors="replace") as f:
            f.write(content)
        return os.path.join(_SIDECAR_SUBDIR, run_id, fname).replace("\\", "/")
    except Exception:
        return None


def _cleanup_old_sidecars(ws: str) -> None:
    """Best-effort: drop sidecar files older than _SIDECAR_MAX_AGE_HOURS."""
    try:
        root = os.path.join(ws, _SIDECAR_SUBDIR)
        if not os.path.isdir(root):
            return
        import time as _time
        cutoff = _time.time() - _SIDECAR_MAX_AGE_HOURS * 3600
        for dirpath, _, files in os.walk(root):
            for fn in files:
                if not fn.startswith("tool_"):
                    continue
                fp = os.path.join(dirpath, fn)
                try:
                    if os.path.getmtime(fp) < cutoff:
                        os.remove(fp)
                except Exception:
                    pass
    except Exception:
        pass


# ── Git-Anchored Context Compression ──
# When tier-2 condenses many write/edit messages, the working tree itself is the
# authoritative state. If the workspace is a git repo, collapse all intermediate
# edit payloads into ONE <accumulated_changes> block (git diff --stat + truncated
# diff) — byte-exact, zero LLM calls. Non-git workspaces keep the old one-liner
# path untouched. GIT_DIFF_BLOCK=0 disables it.

_GIT_STAT_MAX_CHARS = 1200
_GIT_DIFF_MAX_CHARS = 2600

def _git_changes_block(ws: str) -> str | None:
    """Best-effort <accumulated_changes> block from the working tree, or None."""
    if os.environ.get("GIT_DIFF_BLOCK", "1") != "1":
        return None
    if not ws or not os.path.isdir(ws):
        return None
    try:
        import subprocess
        def _run(cmd: list[str], timeout: int = 10) -> str:
            try:
                r = subprocess.run(
                    ["git", "-C", ws] + cmd,
                    capture_output=True, text=True, timeout=timeout,
                    shell=False, errors="replace",
                )
                return r.stdout.strip() if r.returncode == 0 else ""
            except Exception:
                return ""

        if _run(["rev-parse", "--is-inside-work-tree"]) != "true":
            return None  # not a git repo — keep the one-liner fallback
        # Scope to the workspace subtree: a task dir nested inside a larger repo
        # must only ever report ITS OWN changes, not the parent repo's diff.
        stat = _run(["diff", "--stat", "--", "."])
        if not stat:
            return None  # clean tree — nothing to report
        stat = stat[:_GIT_STAT_MAX_CHARS]

        diff = _run(["diff", "-U1", "--", "."])
        if diff:
            if len(diff) > _GIT_DIFF_MAX_CHARS:
                head, tail = diff[:1600], diff[-1000:]
                diff = f"{head}\n... [{len(diff):,} chars of diff omitted] ...\n{tail}"
        else:
            diff = ""

        lines = [stat]
        if diff:
            lines.append(diff)
        return (
            "<accumulated_changes>\n"
            + "\n".join(lines)
            + "\n</accumulated_changes>\n"
            "[All file edits above are saved on disk; content is the current working tree. "
            "Do not re-apply them. Re-read only the exact lines you still need.]"
        )
    except Exception:
        return None


# ── Tail selection (B5): keep the last N TURNS up to a token budget ──
# Replaces fixed keep_last_n message counts with a token-budget-aware tail that
# preserves recency proportionally to window size (opencode select() pattern).

def select_tail_indices(
    messages: list,
    budget_tokens: int,
    keep_recent_turns: int = 2,
    min_keep_messages: int = 6,
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
    if budget_tokens <= 0:
        budget_tokens = estimate_tokens(" ".join(str(m.content) for m in messages[-min_keep_messages:]))

    # Segment into turns scanning backward: a turn ends at an AIMessage
    turns = []  # (start_idx, end_idx, tokens) in forward order
    cur = []
    for i in range(len(messages) - 1, -1, -1):
        cur.append(i)
        if isinstance(messages[i], AIMessage):
            toks = sum(estimate_tokens(str(messages[j].content)) for j in cur)
            turns.append((cur[-1], cur[0] + 1, toks))
            cur = []
    if cur:
        toks = sum(estimate_tokens(str(messages[j].content)) for j in cur)
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
                t = estimate_tokens(str(messages[idx].content))
                if t <= remaining:
                    keep.append(idx)
                    remaining -= t
                idx -= 1
            break

    keep_set = set(keep)
    if len(keep_set) < min_keep_messages:
        keep_set = set(range(len(messages) - min_keep_messages, len(messages)))
    return keep_set


# ── Prune stage (B6): erase bodies of old tool outputs, keep stubs ──

_TOOL_RESULT_HEAD = re.compile(r"^\[([a-zA-Z0-9_-]+)\]")
_PRUNE_STUB_KEEP = 300  # head chars kept in the stub
_PRUNE_MIN_CHARS = 60_000  # only commit if at least this much is freed
_PRUNE_PROTECT_CHARS = 120_000  # protect the most recent this-many chars of tool outputs


def prune_stale_tool_outputs(
    messages: list,
    protect_chars: int = _PRUNE_PROTECT_CHARS,
    min_prune_chars: int = _PRUNE_MIN_CHARS,
    keep_recent_turns: int = 2,
) -> tuple:
    """
    Incremental tier-0 compaction: walk backward from the tail, erase the BODIES
    of old tool-result messages (keeping a stub + tool name) so context frees up
    without an LLM call or a full history rebuild. Keeps recent turns intact,
    never touches error-signal messages or file reads.

    Raw output is offloaded to a workspace sidecar (<ws>/.agent-cache/<run-id>/)
    before stubbing — retrievable via read_file — so nothing is lost
    (SIDECAR_OFFLOAD=0 restores the old destructive stub).

    Returns (new_messages, pruned_chars).
    """
    if len(messages) < 8:
        return messages, 0

    # Boundary: keep the most recent N turns untouched
    ai_seen = 0
    boundary = 0
    for i in range(len(messages) - 1, -1, -1):
        if isinstance(messages[i], AIMessage):
            ai_seen += 1
            if ai_seen >= keep_recent_turns:
                boundary = i
                break
    if boundary <= 0:
        boundary = len(messages) - 6

    # Collect pruneable tool-result messages in the OLD region (before boundary),
    # newest-first (descending index). Skip error-signal messages, file reads,
    # and the Zone-1/Zone-2 header ({0,1} — CF2: the task message must never be
    # stubbed; index 1 sits at the prefix boundary, any rewrite kills the cache).
    header_indices = get_header_indices(messages)
    candidates: list[tuple[int, int]] = []
    for i in range(boundary - 1, -1, -1):
        if i in header_indices:
            continue
        msg = messages[i]
        if not isinstance(msg, HumanMessage):
            continue
        content = msg.content if hasattr(msg, "content") else str(msg)
        low = content.lower()
        if any(e in low for e in _ERROR_SIGNALS):
            continue  # never prune error/assertion content
        if "[FILE]" in content:
            continue  # file reads are knowledge — never pruned
        if len(content) < 500:
            continue  # not worth stubbing
        candidates.append((i, len(content)))

    # Protect the most recent protect_chars worth of tool output; prune the rest
    prunable: list[tuple[int, int]] = []
    acc = 0
    for i, n in candidates:  # newest-first
        acc += n
        if acc > protect_chars:
            prunable.append((i, n))

    total_pruned = sum(n for _, n in prunable)
    if total_pruned < min_prune_chars or not prunable:
        return messages, 0

    sidecar_on = os.environ.get("SIDECAR_OFFLOAD", "1") == "1"
    if sidecar_on:
        _cleanup_old_sidecars(_get_active_ws())

    result = list(messages)
    for idx, n in prunable:
        msg = result[idx]
        content = msg.content if hasattr(msg, "content") else str(msg)
        head_match = _TOOL_RESULT_HEAD.match(content)
        tool_label = head_match.group(1) if head_match else "tool"
        sidecar_rel = None
        if sidecar_on:
            # Offload the full output first; the stub then points at it.
            msg_key = getattr(msg, "id", None) or f"m{idx}"
            sidecar_rel = _offload_to_sidecar(_get_active_ws(), content, msg_key, tool_label)
        if sidecar_rel:
            stub = (
                f"[{tool_label}] output offloaded to sidecar: {sidecar_rel} "
                f"({n:,} chars). Use read_file to inspect if needed."
            )
        else:
            stub = content[:_PRUNE_STUB_KEEP]
            stub = re.sub(r"^\[[^\]]+\]\s*", "", stub)  # avoid double tool label in the stub
            stub = f"[{tool_label}] output pruned ({n:,} chars) — was:\n{stub}"
        result[idx] = HumanMessage(
            content=stub,
            id=getattr(msg, "id", None),
            additional_kwargs=getattr(msg, "additional_kwargs", {}),
        )
    return result, total_pruned


def get_header_indices(messages: list) -> set:
    """Indices of the frozen header (SystemMessage + task HumanMessage).

    Messages 0..1 are never compacted away — they carry the system prompt and
    the original task. Mirrors checkpoint_compact's header_end=2 boundary.
    """
    if not messages:
        return set()
    return {0, 1}


def strip_thinking(text: str) -> str:
    """Remove <thinking>...</thinking> blocks from an AI message."""
    if not text:
        return text
    return re.sub(r"<thinking>.*?</thinking>", "", text, flags=re.DOTALL)


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
                from agents.developer_agent import parse_all_tool_calls
                tool_calls = parse_all_tool_calls(content) if content else []
            except Exception:
                pass

            for tc in tool_calls:
                tool_name = tc.get("tool", "")
                if tool_name in (
                    "edit_file",
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

            write_tools = {"edit_file", "batch_edit", "write_to_file", "replace_file_content", "multi_replace_file_content"}
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

def tier1_compact(messages: list, keep_last_n: int = 8, tail_budget_tokens: int = None) -> list:
    """
    Tier 1 (Light Compaction):
      - Strips <thinking> blocks from AI messages.
      - Deduplicates superseded file reads (keeps only the latest read of each file).
      - Truncates tool outputs >40K chars in older messages.
      - Protects headers and the tail (last N messages, or a token-budgeted tail).

    Does NOT destroy semantic content. Does NOT use [TOOL OK] markers.
    File reads are never compacted away — only superseded duplicates are removed.
    """
    header_indices = get_header_indices(messages)
    if tail_budget_tokens:
        protected = select_tail_indices(messages, tail_budget_tokens)
    else:
        protected = set(range(len(messages) - keep_last_n, len(messages)))

    # Backwards scan to find superseded reads
    seen_reads = set()
    superseded_indices = set()
    for idx in range(len(messages) - 1, -1, -1):
        if idx in header_indices or idx in protected:
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
        if i in header_indices or i in protected:
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
                orig_len = len(content)
                head = content[:2000]
                tail = content[-1000:]
                content = f"{head}\n... [{orig_len} chars total — truncated at {_TOOL_TRUNCATE_CHARS} limit] ...\n{tail}"
                content += truncation_note(orig_len, _TOOL_TRUNCATE_CHARS)

            compacted.append(HumanMessage(
                content=content,
                id=getattr(msg, "id", None),
                additional_kwargs=getattr(msg, "additional_kwargs", {})
            ))
            continue

        compacted.append(msg)

    return compacted


# ── Tier 2: Medium Compaction ──

def tier2_compact(messages: list, keep_last_n: int = 8, tail_budget_tokens: int = None) -> list:
    """
    Tier 2 (Medium Compaction):
      - Runs Tier 1 compaction first (strip thinking, dedup reads, truncate >40K).
      - For successful write/edit tool outputs: condense to one line with file path.
      - When many edits were condensed AND the workspace is a git repo, they are
        replaced by a single <accumulated_changes> block (git diff --stat + diff).
      - For large run_command outputs (>40K): truncate, keep small ones intact.
      - File reads are NEVER compacted — the model needs the content.

    No [TOOL OK] markers. No content destruction.
    When more aggressive compaction is needed, use checkpoint_compact instead.
    """
    # 1. Run Tier 1 first
    messages = tier1_compact(messages, keep_last_n, tail_budget_tokens)

    header_indices = get_header_indices(messages)
    if tail_budget_tokens:
        protected = select_tail_indices(messages, tail_budget_tokens)
    else:
        protected = set(range(len(messages) - keep_last_n, len(messages)))

    compacted = []
    action_count = 0
    git_block = None
    git_inserted = False
    action_tools = {
        "edit_file", "batch_edit",
        "write_to_file", "replace_file_content",
        "multi_replace_file_content",
    }
    # First pass: count condenseable write/edit messages in the old region so the
    # git-anchored block (when present) can replace them wholesale.
    for i, msg in enumerate(messages):
        if i in header_indices or i in protected:
            continue
        if not isinstance(msg, HumanMessage):
            continue
        content = str(msg.content) if hasattr(msg, "content") else str(msg)
        if any(e in content.lower() for e in _ERROR_SIGNALS):
            continue
        name_match = re.search(r"Tool '([^']+)'|\[([a-zA-Z0-9_-]+)\]", content[:200])
        tool_name = name_match.group(1) or name_match.group(2) if name_match else ""
        if tool_name in action_tools:
            action_count += 1

    git_block = None
    if action_count >= 3:
        try:
            git_block = _git_changes_block(_get_active_ws())
        except Exception:
            git_block = None

    for i, msg in enumerate(messages):
        if i in header_indices or i in protected:
            if git_block is not None and not git_inserted and i >= 2:
                compacted.append(HumanMessage(content=git_block))
                git_inserted = True
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

            # Write/edit actions — can condense since content is on disk.
            # Checked BEFORE the [FILE] guard: edit results often embed "[FILE]",
            # but the tool name is authoritative (an edit result is not a read).
            if tool_name in action_tools:
                if git_block is not None:
                    # The accumulated_changes block carries the full state —
                    # individual edit payloads are dropped entirely.
                    continue
                # Extract file path from the output
                fp = None
                for ptn in [r"\[FILE\]\s+(\S+)", r"wrote\s+(\S+)", r"edited\s+(\S+)", r"file_path['\"]?\s*[:=]\s*['\"]?([^\s,;)'\"]+)", r"path['\"]?\s*[:=]\s*['\"]?([^\s,;)'\"]+)"]:
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

            # File reads — NEVER compact. Content is knowledge, not side effect.
            if tool_name in ("read_file", "safe_read_file") or "[FILE]" in content:
                # Keep the read intact, but truncate if >40K
                if len(content) > _TOOL_TRUNCATE_CHARS:
                    orig_len = len(content)
                    file_match = re.search(r"\[FILE\]\s+(\S+)", content)
                    fname = file_match.group(1) if file_match else "file"
                    head = content[:2000]
                    tail = content[-1000:]
                    content = f"[read_file]:\n[FILE] {fname}\n{head}\n... [{orig_len} chars total — truncated] ...\n{tail}"
                    content += truncation_note(orig_len, _TOOL_TRUNCATE_CHARS)
                compacted.append(HumanMessage(
                    content=content,
                    id=getattr(msg, "id", None),
                    additional_kwargs=getattr(msg, "additional_kwargs", {})
                ))
                continue

            # run_command — keep if small (<40K), truncate if large
            if tool_name in ("run_command", "exec_command"):
                if len(content) > _TOOL_TRUNCATE_CHARS:
                    orig_len = len(content)
                    head = content[:1000]
                    tail = content[-500:]
                    content = f"[{tool_name}]:\n{head}\n... [{orig_len} chars total — truncated] ...\n{tail}"
                    content += truncation_note(orig_len, _TOOL_TRUNCATE_CHARS)
                compacted.append(HumanMessage(
                    content=content,
                    id=getattr(msg, "id", None),
                    additional_kwargs=getattr(msg, "additional_kwargs", {})
                ))
                continue

            # Everything else — keep intact, truncate if >40K
            if len(content) > _TOOL_TRUNCATE_CHARS:
                orig_len = len(content)
                head = content[:2000]
                tail = content[-1000:]
                content = f"{head}\n... [{orig_len} chars total — truncated] ...\n{tail}"
                content += truncation_note(orig_len, _TOOL_TRUNCATE_CHARS)

        compacted.append(msg)
    return compacted


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


def _build_semantic_resume_summary(messages: list, header_end: int, tail_start: int) -> str | None:
    """B2: one cheap flash call summarizing the DROPPED region semantically.

    The structural summary (file lists + tool counts) preserves WHAT changed;
    this preserves WHY — decisions, failures, open items. Best-effort: any
    error falls back to the structural summary (caller keeps it).
    Gated by T3_LLM_SUMMARY (default 1) and a min dropped-size floor
    (T3_LLM_SUMMARY_MIN_CHARS, default 150K) so small compactions never
    pay for a call.
    """
    if os.environ.get("T3_LLM_SUMMARY", "1") != "1":
        return None
    try:
        dropped = messages[header_end:tail_start]
        dropped_chars = sum(len(str(getattr(m, "content", ""))) for m in dropped)
        min_chars = int(os.environ.get("T3_LLM_SUMMARY_MIN_CHARS", "150000"))
        if dropped_chars < min_chars:
            return None

        # Compact transcript sample: last ~12 dropped messages, ~300 chars each
        sample = []
        for m in dropped[-12:]:
            c = str(getattr(m, "content", ""))
            c = re.sub(r"\s+", " ", c)[:300]
            label = "AI" if isinstance(m, AIMessage) else "tool-result"
            sample.append(f"[{label}] {c}")
        transcript = "\n".join(sample)

        from llm import invoke_with_fallback
        summary = invoke_with_fallback(
            role="MemoryArchivist",
            sys_inst=(
                "You are a context-compaction summarizer. From the agent work "
                "transcript below, write a tight 5-8 line summary capturing: "
                "what was accomplished, what FAILED or is BLOCKED (with the "
                "exact error/failure), decisions made and why, and what remains "
                "to do next. Output ONLY the summary text — no headers, no "
                "markdown, no preamble."
            ),
            prompt=f"Transcript of agent work (older history, being compacted):\n\n{transcript}",
            temp=0.3,
            where="context compaction summary",
        )
        s = str(summary).strip()
        if len(s) >= 40:
            return s
    except Exception:
        pass
    return None


def checkpoint_compact(
    messages: list,
    tool_call_log: list,
    created: list,
    modified: list,
    keep_last_n: int = 12,
    tail_budget_tokens: int = None,
) -> list:
    """
    Checkpoint-Based Compaction.
    Collapses everything between the header and the tail into a structured
    resume summary. Preserves system prompt + task + tail (last N messages, or
    a token-budgeted tail).

    The resume summary is CHAINED: any prior compaction summary found in the
    dropped region is folded into the new summary instead of being lost (M3).
    A semantic LLM summary of the dropped region (B2) is prepended when the
    dropped region is large — best-effort, structural fallback on any failure.

    This is the heavy compaction for when we're approaching the context limit.
    Unlike [TOOL OK] approach, the resume summary preserves dense factual
    information about what was accomplished.
    """
    if tail_budget_tokens:
        tail_indices = select_tail_indices(messages, tail_budget_tokens)
        tail_start = min(tail_indices) if tail_indices else len(messages)
        if tail_start < 3:
            return messages  # conversation fits the budget — nothing to collapse
    else:
        tail_start = len(messages) - keep_last_n
        if len(messages) <= keep_last_n + 2:
            return messages
    if tail_start >= len(messages):
        return messages

    header_end = 2
    summary_msg_content = build_structured_resume_summary(tool_call_log, created, modified)

    # B2: semantic LLM summary of the dropped region, prepended when available
    semantic = _build_semantic_resume_summary(messages, header_end, tail_start)
    if semantic:
        summary_msg_content = (
            "SEMANTIC SESSION SUMMARY (what happened and why):\n"
            f"{semantic}\n\n"
            "STRUCTURAL STATE (files + tool activity):\n"
            f"{summary_msg_content}"
        )

    # M3: chain prior compaction summaries from the dropped region
    prior_summaries = []
    for msg in messages[header_end:tail_start]:
        content = msg.content if hasattr(msg, "content") else str(msg)
        if isinstance(content, str) and content.startswith("[SYSTEM INFO] History compacted"):
            prior_summaries.append(content)
    if prior_summaries:
        last_prior = prior_summaries[-1]
        summary_msg_content = (
            "PREVIOUS SUMMARY (from earlier compaction):\n"
            f"{last_prior[:1500]}\n\n"
            "NEW ACTIVITY SINCE THEN:\n"
            f"{summary_msg_content}"
        )

    # Preserve decisions from the tail messages
    decisions = []
    for msg in list(messages)[tail_start:]:
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
    compacted.extend(messages[tail_start:])
    return compacted


# ─────────────────────────────────────────────────────────────────────────────
# Pi-style session-log compaction (P3): summary entry + retainedTail checkpoint
# ─────────────────────────────────────────────────────────────────────────────

def _entry_char_size(entry) -> int:
    """Rough char size of a session entry for cut-point math."""
    try:
        from session_log import CompactionEntry, BranchSummaryEntry, MessageEntry, message_text
        if isinstance(entry, MessageEntry):
            total = len(message_text(entry.content))
            if isinstance(entry.content, list):
                for block in entry.content:
                    if getattr(block, "type", "") == "toolCall":
                        total += len(str(block.arguments))
            return total
        if isinstance(entry, CompactionEntry):
            return len(entry.summary)
        if isinstance(entry, BranchSummaryEntry):
            return len(entry.summary)
        return 0
    except Exception:
        return 0


def _entries_chars(entries: list) -> int:
    return sum(_entry_char_size(e) for e in entries)


def _log_cut_valid(entry) -> bool:
    """Cut points must be user/assistant messages — never a tool result
    (it must stay with its tool call)."""
    try:
        from session_log import MessageEntry
        return isinstance(entry, MessageEntry) and entry.role in ("user", "assistant")
    except Exception:
        return False


def find_log_cut_point(entries: list, keep_recent_chars: int) -> tuple[int, int]:
    """Walk backwards from the newest entry accumulating char size; cut at the
    first valid user/assistant boundary that fits the budget. Returns
    (cut_index, accumulated_chars); entries[:cut] are summarized."""
    if not entries:
        return 0, 0
    acc = 0
    for i in range(len(entries) - 1, -1, -1):
        acc += _entry_char_size(entries[i])
        if acc >= keep_recent_chars:
            j = i
            while j >= 0 and not _log_cut_valid(entries[j]):
                j -= 1
            return max(j, 0), acc
    return 0, acc


def serialize_entries(entries: list, tool_result_cap: int = 2000) -> str:
    """Serialize session entries to plain text for summarization (Pi-style).

    The flat [User]/[Assistant]/[Tool result] framing keeps the summarizer from
    treating the transcript as a conversation to continue.
    """
    from session_log import (CompactionEntry, BranchSummaryEntry, MessageEntry,
                             message_text)
    lines = []
    for e in entries:
        try:
            if isinstance(e, CompactionEntry):
                lines.append(f"[Context summary]: {e.summary[:2000]}")
            elif isinstance(e, BranchSummaryEntry):
                lines.append(f"[Branch summary]: {e.summary[:2000]}")
            elif isinstance(e, MessageEntry):
                if e.role == "user":
                    lines.append(f"[User]: {message_text(e.content)[:4000]}")
                elif e.role == "assistant":
                    thinking = ""
                    calls = []
                    text = ""
                    if isinstance(e.content, list):
                        for block in e.content:
                            bt = getattr(block, "type", "")
                            if bt == "thinking":
                                thinking = block.thinking
                            elif bt == "toolCall":
                                calls.append(f"{block.name}({block.arguments})")
                            elif bt == "text":
                                text = block.text
                    else:
                        text = str(e.content)
                    if thinking:
                        lines.append(f"[Assistant thinking]: {thinking[:1500]}")
                    if calls:
                        lines.append(f"[Assistant tool calls]: {'; '.join(calls)[:1500]}")
                    if text:
                        lines.append(f"[Assistant]: {text[:4000]}")
                elif e.role == "toolResult":
                    t = message_text(e.content)
                    if len(t) > tool_result_cap:
                        t = f"{t[:tool_result_cap]}... [{len(t)} chars truncated]"
                    lines.append(f"[Tool result]: {t}")
        except Exception:
            continue
    return "\n".join(lines)


def extract_log_file_ops(entries: list) -> tuple[list, list]:
    """Cumulative read/modified file tracking from toolCall blocks."""
    from session_log import MessageEntry
    read_files: list[str] = []
    modified_files: list[str] = []
    seen_r, seen_m = set(), set()

    def _add(dst, seen, path):
        if path and path not in seen:
            seen.add(path)
            dst.append(path)

    for e in entries:
        if not isinstance(e, MessageEntry) or not isinstance(e.content, list):
            continue
        for block in e.content:
            bt = getattr(block, "type", "")
            if bt != "toolCall":
                continue
            args = block.arguments or {}
            path = str(args.get("file_path") or args.get("path") or "")
            if not path:
                continue
            if block.name in ("read_file", "ls"):
                _add(read_files, seen_r, path)
            elif block.name in ("edit_file", "revert_last_change", "apply_patch"):
                _add(modified_files, seen_m, path)
    return read_files, modified_files


_LOG_SUMMARY_SYSTEM = (
    "You are a context-compaction summarizer. From the agent work transcript "
    "below, produce a summary in this exact structure:\n"
    "## Goal\nWhat the user is trying to accomplish\n"
    "## Constraints & Preferences\n- requirements mentioned\n"
    "## Progress\n### Done\n- completed items\n### In Progress\n- current work\n"
    "### Blocked\n- issues with exact errors\n"
    "## Key Decisions\n- **decision**: rationale\n"
    "## Next Steps\n1. what should happen next\n"
    "## Critical Context\n- data needed to continue\n"
    "Include a <read-files> and <modified-files> block at the end. "
    "Output ONLY the summary."
)


def _summarize_entries(entries: list, previous_summaries: list) -> str | None:
    """One flash call summarizing the dropped region (best-effort)."""
    if os.environ.get("LOG_LLM_SUMMARY", "1") != "1":
        return None
    try:
        transcript = serialize_entries(entries)
        if len(transcript) < 2000:
            return None
        prev = ""
        if previous_summaries:
            prev = "PREVIOUS SUMMARIES (fold into context):\n" + \
                "\n---\n".join(s[:1500] for s in previous_summaries[-2:]) + "\n\n"
        from llm import invoke_with_fallback
        summary = invoke_with_fallback(
            role="MemoryArchivist",
            sys_inst=_LOG_SUMMARY_SYSTEM,
            prompt=f"{prev}Transcript of agent work (older history, being compacted):\n\n{transcript}",
            temp=0.3,
            where="session log compaction summary",
        )
        s = str(summary).strip()
        return s if len(s) >= 40 else None
    except Exception:
        return None


def compact_session_log(session_mgr, keep_recent_chars: int | None = None) -> str | None:
    """Pi-style compaction on a session log (P3).

    Finds a safe cut point (never at a tool result), summarizes the dropped
    region, and appends a ``compaction`` entry whose ``retainedTail`` is a
    self-contained checkpoint: ``build_context_entries()`` then rebuilds
    context without walking the older entries. Returns the summary, or None
    when nothing was compacted.
    """
    if os.environ.get("LOG_COMPACT", "1") != "1":
        return None
    if keep_recent_chars is None:
        keep_recent_chars = int(os.environ.get("LOG_KEEP_RECENT_CHARS", "80000"))
    try:
        from session_log import CompactionEntry, MessageEntry
        entries = session_mgr.build_context_entries()
        if len(entries) < 8:
            return None
        cut, acc = find_log_cut_point(entries, keep_recent_chars)
        if cut <= 0:
            return None  # log fits the budget — nothing to compact

        to_summarize = entries[:cut]
        retained = entries[cut:]
        read_files, modified_files = extract_log_file_ops(entries)
        previous_summaries = [e.summary for e in entries if isinstance(e, CompactionEntry)]

        summary = _summarize_entries(to_summarize, previous_summaries)
        if not summary:
            files = []
            if modified_files:
                files.append("Modified: " + ", ".join(modified_files[:20]))
            if read_files:
                files.append("Read: " + ", ".join(read_files[:20]))
            summary = (
                "## Progress\n### Done\n- session progress compacted at checkpoint\n"
                + ("## Critical Context\n" + "\n".join(f"- {f}" for f in files) if files else "")
            )

        retained_tail = [e for e in retained if isinstance(e, MessageEntry)]
        first_kept = retained[0].id if retained else None
        tokens_before = _entries_chars(to_summarize) // 3  # ~3.5 chars/token
        session_mgr.append_compaction(
            summary,
            first_kept_entry_id=first_kept,
            retained_tail=retained_tail,
            tokens_before=tokens_before,
            read_files=read_files,
            modified_files=modified_files,
        )
        return summary
    except Exception:
        return None
