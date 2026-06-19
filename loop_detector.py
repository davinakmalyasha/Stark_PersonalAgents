import json
import difflib
from typing import List, Dict, Any, Optional

def _get_similarity(a: str, b: str) -> float:
    """Calculates text similarity ratio using SequenceMatcher."""
    return difflib.SequenceMatcher(None, a, b).ratio()

def _clean_args(args: Any) -> str:
    """Canonical string representation of arguments, ignoring transient keys."""
    if not isinstance(args, dict):
        return str(args)
    return json.dumps({k: v for k, v in sorted(args.items()) if k != "timeout"}, sort_keys=True)

def _detect_read_stagnation(log: List[Dict[str, Any]]) -> Optional[str]:
    """Checks for repeating reads/searches without edits. Two-tier:
    Tier 1: 4+ reads on <= 2 unique targets → stuck on the same files.
    Tier 2: 12+ consecutive reads on any targets → exploring without acting.
    Tier 3: overall read/write ratio > 5:1 after 10+ calls → indecisive.
    Tier 4: scan-then-open — a search/list followed by 3+ file opens with
            no write/edit between → opened everything the search found
            instead of ranking candidates (the $10k-token navigation anti-pattern).
    """
    if len(log) < 4:
        return None

    # Tier 4: scan-then-open (check first — it's the most actionable)
    scans = ("search_code", "search_codebase", "list_files")
    opens = ("read_file", "view_signatures")
    consecutive_reads = 0
    for entry in reversed(log):
        tool = entry.get("tool", "")
        if tool in ("write_file", "edit_file", "apply_diff", "run_command"):
            break
        if tool in opens:
            consecutive_reads += 1
        elif tool in scans and consecutive_reads >= 3:
            return (
                f"Scan-then-open: searched/listed then opened {consecutive_reads} files with no write between. "
                "Rank the candidates and read ONLY the top 1-2 (use view_signatures first), "
                "then write — do not open every search hit."
            )
        elif tool in scans:
            consecutive_reads = 0

    # Tier 1 & 2: consecutive reads from the end backward
    reads = 0
    read_targets = set()
    for entry in reversed(log):
        tool = entry.get("tool", "")
        if tool in ("write_file", "edit_file", "apply_diff", "run_command"):
            break
        if tool in ("read_file", "search_code", "list_files", "view_signatures"):
            reads += 1
            read_targets.add((tool, _clean_args(entry.get("args"))))
    if reads >= 4 and len(read_targets) <= 2:
        return f"Stuck reading/searching same 1-2 targets {reads}x without modifying codebase."
    if reads >= 12:
        return f"Stuck in read-only loop: {reads} consecutive reads/searches across {len(read_targets)} targets with zero writes/edits."

    # Tier 3: overall indecision — too many reads vs writes across whole log
    total_reads = sum(1 for e in log if e.get("tool") in ("read_file", "search_code", "list_files", "view_signatures"))
    total_writes = sum(1 for e in log if e.get("tool") in ("write_file", "edit_file", "apply_diff", "run_command"))
    total_actions = total_reads + total_writes
    if total_actions >= 10 and total_reads > total_writes * 5:
        return f"Indecisive: {total_reads} reads vs {total_writes} writes ({total_actions} calls) — ratio > 5:1."

    return None

def _detect_cmd_failure_loop(log: List[Dict[str, Any]]) -> Optional[str]:
    """Detects repeating command executions yielding similar failing outputs."""
    cmd_runs: List[Dict[str, Any]] = []
    for entry in reversed(log):
        if entry.get("tool") == "run_command":
            cmd_runs.append(entry)
        if len(cmd_runs) >= 3:
            break
    if len(cmd_runs) < 3:
        return None
    
    first = cmd_runs[0]
    first_cmd = _clean_args(first.get("args"))
    for item in cmd_runs[1:]:
        if _clean_args(item.get("args")) != first_cmd:
            return None
            
    # Check if outcomes are highly similar (e.g. repeating test crashes)
    sim1 = _get_similarity(str(first.get("result_preview")), str(cmd_runs[1].get("result_preview")))
    sim2 = _get_similarity(str(first.get("result_preview")), str(cmd_runs[2].get("result_preview")))
    if sim1 > 0.90 and sim2 > 0.90:
        return f"Executing command {first.get('args', {}).get('command')} repeatedly with >90% identical outcome."
    return None

def detect_stagnation_or_loop(log: List[Dict[str, Any]]) -> Optional[str]:
    """
    Analyzes agent execution history for loops or stagnation.
    Returns: A string description of the stuck state, or None if OK.
    """
    if len(log) < 3:
        return None

    # 1. Check for 3 consecutive identical tool calls
    last_3 = log[-3:]
    if all(c.get("tool") == last_3[0].get("tool") and _clean_args(c.get("args")) == _clean_args(last_3[0].get("args")) for c in last_3):
        return f"Repeated tool '{last_3[0].get('tool')}' with identical arguments 3 times consecutively."

    # 2. Check for alternating loop (A -> B -> A -> B)
    if len(log) >= 4:
        l4 = log[-4:]
        if (l4[0].get("tool") == l4[2].get("tool") and _clean_args(l4[0].get("args")) == _clean_args(l4[2].get("args")) and
            l4[1].get("tool") == l4[3].get("tool") and _clean_args(l4[1].get("args")) == _clean_args(l4[3].get("args"))):
            return f"Alternating loop detected: '{l4[0].get('tool')}' and '{l4[1].get('tool')}'."

    # 3. Check for read/search stagnation
    read_stg = _detect_read_stagnation(log)
    if read_stg:
        return read_stg

    # 4. Check for repeating failing commands
    cmd_fail = _detect_cmd_failure_loop(log)
    if cmd_fail:
        return cmd_fail

    return None


class LoopGuard:
    @staticmethod
    def check_pre_execute(tool_call_log: List[Dict[str, Any]], tool_name: str, tool_args: Dict[str, Any]) -> Optional[tuple[str, str]]:
        """
        Checks for loops/stale reads BEFORE executing a tool.
        Returns:
            ("STALE", msg) -> If the tool execution should be intercepted and skipped.
            ("ABORT", msg) -> If a hard loop is detected and execution should abort.
            ("WARNING", msg) -> If execution can proceed, but a warning should be appended to the tool result.
            None -> If execution is okay to proceed.
        """
        # 1. Stale-read detection for read_file
        if tool_name == "read_file":
            target_file = tool_args.get("file_path", "")
            read_key = (target_file, tool_args.get("offset"), tool_args.get("limit"))
            
            same_reads = 0
            for item in reversed(tool_call_log[-20:]):
                # Reset check if there was a successful write/edit on the same file
                if item.get("tool") in ("write_file", "edit_file") and item.get("args", {}).get("file_path") == target_file:
                    res_preview = str(item.get("result_preview", ""))
                    if not res_preview.startswith("Error") and not res_preview.startswith("TOOL ERROR"):
                        break
                if item.get("tool") == "read_file":
                    item_key = (item.get("args", {}).get("file_path"), item.get("args", {}).get("offset"), item.get("args", {}).get("limit"))
                    if item_key == read_key:
                        same_reads += 1
            if same_reads >= 2:
                return (
                    "STALE",
                    f"This file was already read {same_reads + 1} times with the same parameters. "
                    f"Content has NOT changed. Do NOT re-read this file — use the previous output. "
                    f"If you need different content, change offset/limit or read a different file."
                )

        # 2. Identical calls check since state change
        same_count = 0
        canonical_args = _clean_args(tool_args)
        for item in reversed(tool_call_log):
            item_tool = item.get("tool")
            item_args = item.get("args", {})
            if item_tool == tool_name and _clean_args(item_args) == canonical_args:
                same_count += 1
                continue
            if item_tool in ("write_file", "edit_file", "apply_diff"):
                break
            if item_tool == "run_command":
                break

        is_read_tool = tool_name in ("read_file", "list_files", "search_code")
        abort_limit = 4 if is_read_tool else 5
        if same_count >= abort_limit:
            return (
                "ABORT",
                f"Stuck in an execution loop. The tool '{tool_name}' with arguments {canonical_args} "
                f"was called {same_count + 1} times and repeatedly failed or returned the same output."
            )
        elif same_count >= 3:
            if is_read_tool:
                warning_msg = (
                    f"\n\n[WARNING] You have executed the read tool '{tool_name}' with these exact arguments {same_count + 1} times. "
                    "If you are not finding what you need, please change your search query, read a different file, "
                    "check if the information is already in your system/project prompt, or proceed with writing the plan/report."
                )
            else:
                warning_msg = (
                    f"\n\n[WARNING] You have executed the tool '{tool_name}' with these exact arguments {same_count + 1} times. "
                    "If this command is repeatedly failing or yielding the same result, you are likely stuck in a loop. "
                    "Please change your approach, try a different command, or if this is a blocking issue requiring user "
                    "intervention (like system configuration or version mismatch), stop and output an 'ERROR: <description>' response."
                )
            return ("WARNING", warning_msg)

        return None

    @staticmethod
    def check_metacognitive(tool_call_log: List[Dict[str, Any]]) -> Optional[str]:
        """
        Periodically checks overall agent history for loops and stagnation.
        """
        return detect_stagnation_or_loop(tool_call_log)


