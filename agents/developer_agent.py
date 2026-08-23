import os
import json
import re
import subprocess
import uuid
from datetime import datetime
from langchain_core.messages import SystemMessage, HumanMessage, AIMessage, ToolMessage, RemoveMessage
from sync.state_sync import safe_update_state, safe_append_live_log
from agents.it_department_nodes_base import ITState
from tools import (
    ToolExecutionResult,
    execute_tool_detailed,
    get_native_tools,
    get_native_tool_names,
    get_registry,
    get_plan_for_context,
)
from llm import invoke_messages_with_fallback_stream

# Local helper imports moved to top level
from llm.loop_detector import LoopGuard
from sync.sync_helpers import load_task_tracking, save_task_tracking
from memory.context_compaction import (
    compact_history,
    get_compaction_policy,
)
from agents.developer_prompt import _STATIC_SYSTEM_TEMPLATE

def _bound_tool_result(tool_name: str, tool_args: dict, result) -> str:
    """Keep a tool result small while preserving a VFS path to the full output."""
    # Lazy import avoids the tools -> AddOnTool -> agents -> developer_agent
    # initialization cycle. The helper runs only after the package is loaded.
    from tools import auto_offload_result, _tool_cap

    result_text = str(result)
    site = tool_name
    if tool_name == "run_command" and str(tool_args.get("process_action", "")).lower() == "logs":
        site = "logs"
    cap = _tool_cap(site)
    lower = result_text.lower()
    # run_command and the registry may already have created a retrievable
    # preview. Re-wrapping that preview would waste a second scratch file.
    already_offloaded = "offloaded" in lower and "/scratch/" in lower
    if len(result_text) <= cap or already_offloaded:
        return result_text
    return auto_offload_result(result_text, tool_name, max_chars=cap)

# Dev system prompt lives in developer_prompt.py (imported above).
# Dev system prompt imported from developer_prompt.py



# Dev system prompt lives in developer_prompt.py (imported above).

def _build_dynamic_context(
    task: str,
    project_path: str,
    context: str,
    is_fixing: bool,
    is_first_call: bool = True,
) -> tuple[str, str]:
    """Build the stable prefix and the latest context tail.

    Only project-identity data and fixed capability pointers belong in the
    SystemMessage. The repository map is refreshed before the request and a
    full snapshot or compact delta is placed in the latest HumanMessage so a
    new map cannot invalidate older conversation history. ``task``,
    ``context``, and ``is_fixing`` remain in the signature for callers that
    still pass them; they are intentionally not rendered as generic context
    blocks.
    """
    del task, context, is_fixing
    safe_path = project_path or "."
    workspace_root = project_path or (
        os.environ.get("HARNESS_ROOT") or os.path.dirname(os.path.abspath(__file__))
    )

    stable_parts = [f"## Workspace Root\n{workspace_root}"]
    current_parts = []

    # The template map is a fixed capability pointer. The live catalog and
    # rendered contents stay behind template_list/apply_template.
    try:
        from tools import build_template_map
        template_map = build_template_map()
        if template_map:
            stable_parts.append(template_map)
    except Exception as e:
        print(f"Error loading template map: {e}")

    # project.md is a small, stable identity card. It is intentionally kept in
    # the cached prefix; code navigation belongs in the current tail below.
    try:
        from memory.memory_manager import load_cached_project_identity
        project_card = load_cached_project_identity(safe_path)
        if project_card:
            stable_parts.append(f"## Project Overview\n{project_card}")
    except Exception as e:
        print(f"Error loading project identity: {e}")

    # Refresh only the cache file before assembling this request. Do not mutate
    # an already-running message list: the new snapshot starts the next tail.
    try:
        from memory.memory_manager import (
            load_cached_code_map,
            refresh_code_map_context,
        )
        map_event = refresh_code_map_context(
            safe_path,
            include_cached_full=is_first_call,
        )
        map_context = str(map_event.get("content") or "").strip()
        # If scanning failed but a valid cache exists, a fresh run still needs
        # one full anchor. Continuing runs deliberately wait for a new event;
        # their prior transcript remains the cache source.
        if not map_context and is_first_call:
            map_context = load_cached_code_map(safe_path).strip()
        if map_context:
            current_parts.append(map_context)
    except Exception as e:
        print(f"Error loading cached code map: {e}")

    return "\n\n".join(stable_parts), "\n\n".join(current_parts)

def _build_system_prompt(task: str, project_path: str, context: str = "", valid_tools_list: list[str] = None, is_first_call: bool = True, is_fixing: bool = False) -> tuple[str, str]:
    """
    Build the system prompt with a cache-friendly message layout.

    Stable SystemMessage: base prompt, workspace root, identity, and the fixed
    template pointer.
    Append-only history: prior task/tool turns remain byte-stable.
    Current tail: the first full projectMap or a later projectMap+ delta and
    the current request are appended after history, so only the tail is new
    on a new task.

    This pushes cache hit rate from ~60% to 85%+ across diverse eval tasks
    because the cache break point moves from mid-prompt to the very tail.

    Returns: (static_system, current_context)
    """
    base_prompt = _STATIC_SYSTEM_TEMPLATE

    # Split dynamic context: stable â†’ SystemMessage, current â†’ latest tail
    stable_context, current_context = _build_dynamic_context(
        task,
        project_path,
        context,
        is_fixing,
        is_first_call=is_first_call,
    )

    # The native API tool schemas are passed separately by the LLM adapter; do
    # not duplicate them in this cached prompt.
    static_system_parts = [base_prompt]
    if stable_context:
        static_system_parts.append(stable_context)
    static_system = "\n\n".join(static_system_parts)

    # The current map is wrapped only when present. A new task/request is added
    # after prior history by developer_node.
    current_suffix = ""
    if current_context:
        current_suffix = f"<current-context>\n{current_context}\n</current-context>"

    return static_system, current_suffix


def _latest_current_context(messages: list) -> str:
    """Return the newest injected map block already present in the transcript."""
    for message in reversed(messages[1:] if len(messages) > 1 else []):
        content = getattr(message, "content", "")
        if not isinstance(content, str) or not content.startswith("<current-context>"):
            continue
        end = content.find("</current-context>")
        if end >= 0:
            return content[:end + len("</current-context>")]
    return ""


def _has_full_project_map(messages: list) -> bool:
    """Return whether the transcript still contains a full map anchor."""
    for message in messages[1:] if len(messages) > 1 else []:
        content = getattr(message, "content", "")
        if not isinstance(content, str):
            continue
        if re.search(r"(?m)^projectMap:\s*$", content):
            return True
    return False


def _format_current_context(map_context: str) -> str:
    """Wrap a full map or delta as the compact current-context block."""
    content = str(map_context or "").strip()
    return f"<current-context>\n{content}\n</current-context>" if content else ""


def _task_tail_parts(
    current_context: str,
    task: str,
    messages: list | None = None,
) -> list[str]:
    """Return separate map and task parts for an append-only tail.

    Keeping the map anchor in its own message lets compaction preserve the
    latest full snapshot without pinning the old task that happened to follow
    it. The extra message framing is smaller than repeatedly re-sending a full
    map after every task.
    """
    parts = []
    if current_context and (
        messages is None or _latest_current_context(messages) != current_context
    ):
        parts.append(current_context)
    parts.append(f"## Task\n{task}")
    return parts


def _task_tail_content(
    current_context: str,
    task: str,
    messages: list | None = None,
) -> str:
    """Append a map only when it is a new snapshot, then append the task."""
    return "\n\n".join(_task_tail_parts(current_context, task, messages))

def _extract_text_response(text: str) -> str:
    """Extract the user-facing text from a final native-tool response."""
    if not text:
        return ""
    # Native tool calls are transport metadata, not text in the response.
    cleaned = re.sub(r'<thinking>.*?</thinking>', '', text, flags=re.DOTALL)
    return cleaned.strip()


def _log(msg: str) -> None:
    """Log to console and the shared state live terminal."""
    try:
        print(msg, flush=True)
    except Exception:
        pass
    # Appends under lock instead of snapshot+overwrite — the old pattern could
    # drop lines appended by other threads between the read and the write.
    safe_append_live_log(msg)

def serialize_messages(messages: list) -> list[dict]:
    """Serialize the ordered transcript without flattening tool-call pairs."""
    serialized = []
    for m in messages:
        msg_dict = {"type": type(m).__name__, "content": m.content}
        if hasattr(m, "id") and m.id:
            msg_dict["id"] = m.id
        if hasattr(m, "name") and m.name:
            msg_dict["name"] = m.name
        if hasattr(m, "tool_calls") and m.tool_calls:
            msg_dict["tool_calls"] = m.tool_calls
        if hasattr(m, "tool_call_id") and m.tool_call_id:
            msg_dict["tool_call_id"] = m.tool_call_id
        if hasattr(m, "status") and m.status:
            msg_dict["status"] = m.status
        if hasattr(m, "additional_kwargs") and m.additional_kwargs:
            msg_dict["additional_kwargs"] = m.additional_kwargs
        serialized.append(msg_dict)
    return serialized

def deserialize_messages(serialized_msgs: list[dict]) -> list:
    """Helper to deserialize messages back to LangChain message objects."""
    messages = []
    for msg_data in serialized_msgs:
        m_type = msg_data.get("type")
        content = msg_data.get("content", "")
        m_id = msg_data.get("id")
        name = msg_data.get("name")
        kwargs = {}
        if m_id:
            kwargs["id"] = m_id
        if name:
            kwargs["name"] = name
        add_kwargs = msg_data.get("additional_kwargs")
        if add_kwargs:
            kwargs["additional_kwargs"] = add_kwargs
            
        if m_type == "SystemMessage":
            msg = SystemMessage(content=content, **kwargs)
        elif m_type == "HumanMessage":
            msg = HumanMessage(content=content, **kwargs)
        elif m_type == "AIMessage":
            tool_calls = msg_data.get("tool_calls")
            if tool_calls:
                normalized_calls = []
                for tc in tool_calls:
                    if not isinstance(tc, dict):
                        continue
                    normalized_calls.append({
                        "name": tc.get("name") or tc.get("tool", ""),
                        "args": tc.get("args") or {},
                        "id": tc.get("id") or f"legacy-call-{len(normalized_calls)}",
                        "type": "tool_call",
                    })
                if normalized_calls:
                    kwargs["tool_calls"] = normalized_calls
            msg = AIMessage(content=content, **kwargs)
        elif m_type == "ToolMessage":
            tool_call_id = msg_data.get("tool_call_id") or m_id or f"legacy-tool-{len(messages)}"
            if msg_data.get("status"):
                kwargs["status"] = msg_data["status"]
            kwargs["tool_call_id"] = tool_call_id
            msg = ToolMessage(content=content, **kwargs)
        elif m_type == "RemoveMessage":
            msg = RemoveMessage(**kwargs)
        else:
            msg = HumanMessage(content=content, **kwargs)
        messages.append(msg)
    return messages

def developer_node(s: ITState) -> dict:
    """
    Tool-using Developer agent.
    Loops: LLM decides tool â†’ execute tool â†’ feed result â†’ repeat until done.

    Uses proper LangChain message types for multi-turn conversations.
    """
    # Initialize run-specific state variables to avoid thread-unsafe function attributes
    messages = []

    def make_return(res_dict: dict) -> dict:
        new_msgs = [m for m in messages if getattr(m, "id", None) not in existing_ids]
        res_dict["messages"] = new_msgs
        
        # Capture final git diff and append to messages / task artifacts
        git_diff = ""
        try:
            import subprocess
            diff_res = subprocess.run(["git", "diff", "HEAD"], capture_output=True, text=True, cwd=project_path, timeout=5)
            if diff_res.returncode == 0 and diff_res.stdout and diff_res.stdout.strip():
                git_diff = diff_res.stdout.strip()
        except Exception as e:
            print(f"[DEVELOPER] Error getting git diff: {e}")

        if git_diff:
            # Save to task.json artifacts
            try:
                task_data = load_task_tracking(project_path, chat_id)
                if task_data:
                    artifacts = task_data.setdefault("artifacts", {})
                    artifacts["git_diff"] = git_diff
                    save_task_tracking(task_data, project_path, chat_id)
            except Exception as e:
                print(f"[DEVELOPER] Error saving git diff to task tracking: {e}")

            # The complete diff is already stored in task artifacts. Do not
            # copy it into durable model history; the handoff report contains
            # the compact touched-file summary and the artifact is retrievable.

        # Persist developer state to task.json on return so we can resume if we route back
        try:
            task_data = load_task_tracking(project_path, chat_id)
            if not task_data and chat_id:
                task_data = {
                    "task_id": f"auto_{chat_id}",
                    "chat_id": chat_id,
                    "user_request": client_req,
                    "created_at": datetime.now().isoformat(),
                    "updated_at": datetime.now().isoformat(),
                    "status": "in_progress",
                    "artifacts": {
                        "files_created": [],
                        "files_modified": [],
                    }
                }
            if task_data:
                is_completed = False
                if task_data.get("status") == "completed":
                    is_completed = True
                
                if not is_completed:
                    serialized_msgs = serialize_messages(messages)
                    task_data["developer_state"] = {
                        "messages": serialized_msgs,
                        "iteration": iteration,
                        "tool_call_log": tool_call_log,
                        "step_tool_calls": step_tool_calls,
                        "tracked_files_created": tracked_files_created,
                        "tracked_files_modified": tracked_files_modified,
                        "last_response_text": last_response_text,
                    }
                    save_task_tracking(task_data, project_path, chat_id)
        except Exception as e:
            print(f"[DEVELOPER] Error saving developer_state in make_return: {e}")

        return res_dict

    client_req = s.get("client_request", "")
    tech_spec = s.get("tech_spec", "")
    requirements = s.get("requirements", "")
    test_report = s.get("test_report", "")
    is_fixing = bool(test_report)
    project_path = s.get("project_path", "") or (os.environ.get("HARNESS_ROOT") or os.path.dirname(os.path.abspath(__file__)))
    err_count = s.get("error_count", 0)
    chat_id = s.get("chat_id", "")

    spec_context = ""
    if tech_spec:
        spec_context = f"\n\n## Technical Specification\n{tech_spec[:3000]}\n\n## Requirements\n{requirements[:2000]}"
    if is_fixing and test_report:
        failure_excerpt = compact_failure_report(test_report)
        failure_hints = extract_traceback_file_hints(project_path, test_report)
        if failure_excerpt:
            spec_context += f"\n\n## Failure Report\n{failure_excerpt}"
        if failure_hints:
            spec_context += f"\n\n{failure_hints}"

    task = (
        f"{client_req}{spec_context}\n\n"
        "Adapt your approach to what this request needs.\n"
        "- If it needs investigation: read files, explore, synthesize findings.\n"
        "- If it needs implementation: write code, run tests, verify it works.\n"
        "- If tests are failing: read the error, apply minimal fix, re-run.\n"
        "- If it is vague: propose a concrete plan, then implement it.\n"
        "You have all tools. Decide what to do. You finish when you stop calling tools."
    )
    safe_update_state({"thoughts": {"developer": "Analyzing and executing..."}})

    # The latest request and the live projectMap are the complete task tail.
    # Older task/tool turns remain in the append-only message history; do not
    # duplicate chat transcripts, modified-file lists, or progress journals here.
    context = ""

    # Coding tools exposed to Dev come from the central capability group used
    # by the subagent registry. Native schemas and execution validation share
    # the same registry, so the API tool surface cannot drift from execution.
    valid_tools = get_native_tool_names("developer")
    valid_tool_set = set(valid_tools)
    native_dev_tools = get_native_tools("developer")
    get_registry().restrict_scope("Developer", allow=list(valid_tools))

    # â”€â”€ Load developer state if resuming from suspension â”€â”€
    developer_state = None
    try:
        task_data = load_task_tracking(project_path, chat_id)
        if task_data and "developer_state" in task_data:
            developer_state = task_data["developer_state"]
            # Save without developer_state first so a failed save doesn't lose state
            save_task_tracking({k:v for k,v in task_data.items() if k != "developer_state"}, project_path, chat_id)
            del task_data["developer_state"]
    except Exception as e:
        print(f"[DEVELOPER] Error checking resume state: {e}")

    # Build the stable prefix and the latest map/task tail.
    task_message = task
    static_system, current_context = _build_system_prompt(
        task_message, project_path, context, valid_tools, is_first_call=(developer_state is None), is_fixing=is_fixing
    )

    if developer_state:
        _log(f"[DEVELOPER] ðŸ”„ Resuming from suspended execution state (iteration {developer_state['iteration']})...")
        if developer_state.get("messages"):
            messages = deserialize_messages(developer_state["messages"])
        elif hasattr(s, "get") and s.get("messages") and len(s.get("messages")) > 0:
            messages = list(s.get("messages"))
            _log(f"[DEVELOPER] Loaded {len(messages)} messages natively from Graph State.")
        else:
            messages = []

        # Refresh only the stable system prefix. Never rewrite an old task or
        # tool result: doing so would invalidate every cached history token.
        if messages and isinstance(messages[0], SystemMessage):
            old_id = getattr(messages[0], "id", None)
            messages[0] = SystemMessage(content=static_system, id=old_id) if old_id else SystemMessage(content=static_system)
        else:
            messages.insert(0, SystemMessage(content=static_system))

        # A checkpoint may have compacted the old map anchor. Re-establish one
        # full current snapshot before applying a delta; otherwise the model
        # would receive an update without the base it is meant to modify.
        if not _has_full_project_map(messages):
            try:
                from memory.memory_manager import load_cached_code_map
                cached_map = load_cached_code_map(project_path).strip()
                if cached_map:
                    current_context = _format_current_context(cached_map)
            except Exception as e:
                print(f"Error restoring project map anchor: {e}")

        # Check if we are resuming from suspension vs starting a new repair/implement cycle
        resume_words = {"continue", "resume", "proceed", "go"}
        req_words = client_req.lower().strip().rstrip(".!?").split()
        is_resume_prompt = bool(resume_words & set(req_words)) or client_req.lower().strip().rstrip(".!?") in ["go on", "go"]

        if is_resume_prompt:
            iteration = developer_state["iteration"]
            step_tool_calls = developer_state["step_tool_calls"]
            # The task is already in the transcript, but the refreshed map is
            # new context for this invocation. Append it at the tail so the
            # stable prefix and all prior history remain cacheable.
            if current_context and _latest_current_context(messages) != current_context:
                messages.append(HumanMessage(content=current_context))
        else:
            iteration = 0  # start a fresh repair cycle while retaining durable history
            step_tool_calls = 0
            # Append the new task traceback / client request as a human message to the end of history
            messages.extend(
                HumanMessage(content=part)
                for part in _task_tail_parts(current_context, task_message, messages)
            )
            _log("[DEVELOPER] ðŸ”„ Starting a new repair/implement cycle and appending the new task description.")

        tool_call_log = developer_state["tool_call_log"]
        tracked_files_created = developer_state["tracked_files_created"]
        tracked_files_modified = developer_state["tracked_files_modified"]
        last_response_text = developer_state["last_response_text"]

        # Codex-style plan re-injection: the plan is part of the agent's
        # session state; restore it on resume so the model keeps driving it.
        try:
            plan_block = get_plan_for_context("developer")
            if plan_block:
                messages.append(HumanMessage(content=plan_block))
        except Exception as e:
            print(f"[DEVELOPER] Error restoring plan block: {e}")
        
    else:
        # Stable system prefix followed by a separate full-map anchor and task
        # tail. Keeping these as separate messages lets compaction preserve the
        # map without pinning the task that originally followed it.
        messages = [SystemMessage(content=static_system)]
        messages.extend(
            HumanMessage(content=part)
            for part in _task_tail_parts(current_context, task_message)
        )
        iteration = 0
        tool_call_log = []
        last_response_text = ""
        step_tool_calls = 0
        tracked_files_created = []
        tracked_files_modified = []
    verification_nudged = False

    # Vision: inject user-attached images (from the API stash) before the loop
    # starts — applies to both fresh starts and repair/resume cycles (a new
    # task tail with attachments is exactly when they arrive).
    try:
        from tools.vision_channel import drain_attachment_images
        if drain_attachment_images(messages):
            safe_append_live_log("[VISION] Developer: user attachment image(s) injected")
    except Exception:
        pass

    existing_ids = {m.id for m in messages if getattr(m, "id", None)}

    # One shared append-only/checkpoint policy. There are no intermediate
    # history rewrites or per-run tier counters to restore.
    compaction_policy = get_compaction_policy("developer")

    _log(f"\n{'='*60}\n[DEVELOPER] Starting tool-using agentic loop\n{'='*60}")

    # Reset per-run counters
    last_tool_failed = False  # previous turn: a tool errored or command exited non-zero
    last_test_failed = False  # previous turn: a test/verify command failed
    pending_guidance: list[str] = []  # transient next-call steering; never durable history

    while True:
        iteration += 1
        _log(f"\n[DEV Iteration {iteration}]")
        # Detect topology changes made by the previous tool turn. This is a
        # deterministic filesystem check, not a timed refresh: append one
        # immutable projectMap/projectMap+ event before the next model call.
        # The canonical full map remains untouched until the checkpoint.
        try:
            from memory.memory_manager import refresh_code_map_context
            map_event = refresh_code_map_context(project_path, include_cached_full=False)
            map_content = str(map_event.get("content") or "").strip()
            if map_content:
                map_suffix = _format_current_context(map_content)
                if map_suffix and _latest_current_context(messages) != map_suffix:
                    messages.append(HumanMessage(content=map_suffix))
        except Exception as e:
            _log(f"[DEVELOPER] Project map delta check skipped: {e}")

        # Append-only below the single checkpoint threshold. At the threshold,
        # refresh the full map and commit one transcript rebuild atomically.
        try:
            messages, compaction_info = compact_history(
                messages,
                tool_call_log=tool_call_log,
                created=tracked_files_created,
                modified=tracked_files_modified,
                policy=compaction_policy,
                workspace_path=project_path,
                chat_id=chat_id,
            )
        except Exception as e:
            _log(f"[DEVELOPER] Context checkpoint unavailable: {e}")
            safe_update_state({"thoughts": {"developer": f"Context checkpoint unavailable: {e}"}})
            return make_return({
                "code": f"// ERROR: Context checkpoint unavailable: {e}",
                "agent_report": f"Context checkpoint unavailable; no transcript rewrite was committed. {e}",
                "test_report": f"STATUS: BLOCKED\nContext checkpoint unavailable: {e}",
                "project_path": project_path,
                "code_updated": bool(tracked_files_created or tracked_files_modified),
                "tech_spec_updated": False,
                "error_count": err_count + 1,
            })
        if compaction_info.get("checkpoint"):
            _log(
                f"[DEVELOPER] Single {compaction_info.get('trigger')} checkpoint "
                f"({compaction_info['before']:,} -> {compaction_info['after']:,} est. tokens; "
                f"tail={compaction_policy.tail_budget_tokens:,}; map_refreshed="
                f"{compaction_info.get('map_refreshed', False)})"
            )

        run_messages = list(messages)
        if pending_guidance:
            # Steering is useful for the next decision but is not task history.
            run_messages.append(HumanMessage(content="\n\n".join(pending_guidance)))
            pending_guidance = []
        response_text = None  # Initialize â€” will be set by LLM

        # Call LLM with proper multi-turn messages (unless cache hit)
        if not response_text:
            try:
                # Build concise "where" with file + detail from PREVIOUS tool call
                last_entry = tool_call_log[-1] if tool_call_log else {}
                last_tool = last_entry.get("tool", "")
                last_args = last_entry.get("args", {})
                last_detail = last_entry.get("detail", "")
                fpath = last_args.get("file_path", last_args.get("path", ""))
                fname = os.path.basename(str(fpath)) if fpath else ""

                if last_tool == "read_file" and fname:
                    where_tag = f"Iter {iteration}/unbounded: read {fname} {last_detail}".strip()
                elif last_tool == "edit_file" and fname:
                    where_tag = f"Iter {iteration}/unbounded: edit {fname} {last_detail}".strip()
                elif last_tool == "run_command":
                    cmd = str(last_args.get("command", ""))[:50]
                    where_tag = f"Iter {iteration}/unbounded: run {cmd}"
                elif last_tool == "search_code":
                    pat = str(last_args.get("pattern", ""))[:40]
                    where_tag = f"Iter {iteration}/unbounded: search \"{pat}\""
                elif last_tool and fname:
                    where_tag = f"Iter {iteration}/unbounded: {last_tool} {fname}"
                elif last_tool:
                    where_tag = f"Iter {iteration}/unbounded: {last_tool}"
                elif iteration == 1:
                    where_tag = f"Iter 1/unbounded: {client_req[:80].strip()}"
                else:
                    where_tag = f"Iter {iteration}/unbounded"

                # ── Native Function Calling ──
                # Pass tool schemas via the API tools parameter so the model
                # returns structured tool_calls instead of text we must parse.
                # ── Adaptive reasoning budget ladder ──
                # The reasoning budget adapts to the TURN TYPE, not a fixed level:
                #   iter-1 planning ............ high (plan & diagnose)
                #   failure/repair-triggered ... high (the previous turn failed:
                #     tool error, non-zero exit, failing test, failed edit)
                #   search/discovery ............ low (decide what to search next
                # ── Reasoning ladder (CACHE-AWARE): iter-1 planning uses "high";
                # everything else runs "disabled". Deliberately NO intermediate
                # rung: DeepSeek's prefix cache is keyed per thinking config, so
                # EVERY effort transition (high→low→disabled) re-misses the whole
                # per-project prefix (~4.7k tokens) once per task. The old "low"
                # discovery rung cost that miss while producing ~0 reasoning.
                # Trouble signals flip the NEXT turn back to "high" (cheap: the
                # high-config request is already cached from iter-1).
                # Knobs: REASONING_ITER1 (planning depth, "high"),
                #        REASONING_ROUTINE (routine depth, "disabled"),
                #        FLIP_ON_TEST_ONLY=1 (default) → flip to high only
                #        on real TEST failures; tool errors don't flip. With
                #        Dev self-verification off there are no mid-session
                #        test failures → the session is config-stable
                #        (iter-1 high, everything else disabled, zero flips).
                iter1_effort = os.environ.get("REASONING_ITER1", "high")
                routine_effort = os.environ.get("REASONING_ROUTINE", "disabled")
                flip_on_test_only = os.environ.get("FLIP_ON_TEST_ONLY", "1") == "1"
                think_hard = (
                    iteration <= 1
                    or last_test_failed
                    or (not flip_on_test_only and last_tool_failed)
                )
                if think_hard:
                    reasoning_effort = iter1_effort
                    reasoning_budget = None  # planning/diagnosis: full effort, no cap
                else:
                    reasoning_effort = routine_effort
                    reasoning_budget = None

                llm_result = invoke_messages_with_fallback_stream(
                    role="Developer",
                    messages=run_messages,
                    tools=native_dev_tools,
                    where=where_tag,
                    reasoning_effort=reasoning_effort,
                    reasoning_budget=reasoning_budget,
                )

                # Unpack the provider result. Native calls are kept as
                # structured metadata; text-only results simply finish.
                if isinstance(llm_result, tuple):
                    if len(llm_result) == 3:
                        response_text, native_tool_calls, reasoning_content = llm_result
                    else:
                        response_text, native_tool_calls = llm_result
                        reasoning_content = None
                else:
                    response_text = llm_result
                    native_tool_calls = []
                    reasoning_content = None
                response_text = response_text or ""

                # â”€â”€ Store successful code generation in cache â”€â”€
            except Exception as e:
                _log(f"[DEVELOPER] LLM error after all fallbacks: {e}")
                # Return partial state so supervisor can decide
                safe_update_state({"thoughts": {"developer": f"LLM error: {e}"}})
                return make_return({
                    "code": f"// ERROR: All LLM providers failed: {e}",
                    "test_report": f"STATUS: FAIL\nDeveloper LLM error: {e}",
                    "project_path": project_path,
                    "code_updated": False,
                    "tech_spec_updated": False,
                    "error_count": err_count + 1,
                })

        # Handle empty response (only nudge if both text and native tool calls are empty)
        if (not response_text or not response_text.strip()) and not native_tool_calls:
            _log("[DEVELOPER] âš ï¸ Empty LLM response â€” retrying with explicit nudge")
            ai_msg_id = f"dev-ai-{iteration}-{uuid.uuid4()}"
            human_msg_id = f"dev-human-{iteration}-{uuid.uuid4()}"
            messages.append(AIMessage(content="(empty response)", id=ai_msg_id))
            messages.append(HumanMessage(
                content="You responded with nothing. Please continue working on the task. "
                        "Use a tool to make progress, or if done, explain what you accomplished.",
                id=human_msg_id
            ))
            continue

        last_response_text = response_text
        _log(f"[DEVELOPER] Response ({len(response_text)} chars)")

        # Log native reasoning_content if present
        if reasoning_content:
            _log(f"[THINKING] Developer: {reasoning_content.strip()}")

        # Extract and log thinking block if present
        thinking_match = re.search(r"<thinking>(.*?)</thinking>", response_text, flags=re.DOTALL)
        if thinking_match:
            thinking_content = thinking_match.group(1).strip()
            _log(f"[THINKING] Developer: {thinking_content}")

        # Extract only native API tool calls. Text/XML parser fallbacks are
        # intentionally removed: tool schemas and calls stay in the API
        # protocol so transcript history never contains pseudo-tool syntax.
        tool_calls = []
        if native_tool_calls:
            for call_index, tc in enumerate(native_tool_calls):
                tname = tc.get("tool") or tc.get("name")
                targs = tc.get("args") or {}
                if isinstance(targs, str):
                    try:
                        targs = json.loads(targs)
                    except Exception:
                        pass
                if tname:
                    tool_calls.append({
                        "tool": tname,
                        "args": targs,
                        "id": tc.get("id") or f"dev-call-{iteration}-{call_index}-{uuid.uuid4().hex[:8]}",
                    })
        if not tool_calls:
            # Tier0 DONE-gate: if real files were touched but NO verification
            # command ever ran, force one verify turn before finishing.
            # Honest-report-and-finish: after ONE nudge we accept the finish
            # even if tests fail — the report must state it, not hide it.
            modified_any = bool(tracked_files_modified or tracked_files_created)
            ran_verification = any(
                tc.get("tool") == "run_command"
                and re.search(r"test|pytest|check|lint|verify|coverage|tsc\b|ruff|compile",
                              json.dumps(tc.get("args", {}), default=str), re.IGNORECASE)
                for tc in tool_call_log
            )
            if modified_any and not ran_verification and not verification_nudged:
                verification_nudged = True
                _log("[DEVELOPER] Verification nudge — files were modified but no test/check ran. Forcing one verify turn.")
                messages.append(HumanMessage(
                    content=(
                        f"[NUDGE] You modified {len(tracked_files_modified) + len(tracked_files_created)} file(s) "
                        "but never ran a verification command (tests / lint / checks). "
                        "Run the project's test suite and relevant checks now via run_command, then report the results. "
                        "If a check fails, fix or honestly report the failure — do not claim success."
                    ),
                    id=f"dev-verify-nudge-{iteration}-{uuid.uuid4()}",
                ))
                continue

            # No native tool call — the model has chosen to finish.
            _log(f"[DEVELOPER] No tool call parsed from response (len={len(response_text)}). Preview: {repr(response_text[:300])}")
            clean_response = _extract_text_response(response_text)
            _log(f"\n[DEVELOPER] âœ… Agent finished after {iteration} iterations")
            _log(f"ðŸ¤– Assistant: {clean_response}")

            is_error = clean_response.strip().upper().startswith("ERROR:") or clean_response.strip().upper().startswith("FAILED:")

            try:
                tool_log_val = json.dumps(tool_call_log, indent=2)
            except Exception:
                tool_log_val = str(tool_call_log)

            # Validate exit summary — bare "Task complete." is useless to the
            # orchestrator. The Dev prompt requires: Status, Files, Tests,
            # Spec compliance, Coordination needed. If the LLM didn't produce
            # a proper summary, synthesize one from tool_call_log.
            if clean_response and len(clean_response.strip()) >= 60:
                report = clean_response[:1000]
            else:
                files_created = [t for t in tool_call_log if t.get("tool") in ("edit_file",)]
                report_lines = ["## Dev Summary"]
                report_lines.append(f"Status: {'ERROR' if is_error else 'DONE'}")
                if files_created:
                    report_lines.append(f"Files: {len(files_created)} file(s) created/modified")
                    for t in files_created[:15]:
                        fp = t.get("args", {}).get("file_path", "?")
                        report_lines.append(f"- {fp}")
                if clean_response:
                    report_lines.append(f"Notes: {clean_response[:500]}")
                report = "\n".join(report_lines)

            safe_update_state({
                "thoughts": {"developer": "Execution failed due to blocking error: " + clean_response[:100] if is_error else "Implementation complete."},
                "developer_tool_log": tool_log_val,
                "outputs": {
                    "code": f"// ERROR: {clean_response[:1000]}" if is_error else "Code successfully implemented.",
                    "agent_report": report
                }
            })

            # ── Update task.json: persist artifacts; mark the task completed on
            # success. No harness-side step machine — the plan is LLM-driven via
            # update_plan (full replacement, persisted per scope).
            try:
                task_tracking_data = load_task_tracking(project_path, chat_id)
                if task_tracking_data:
                    artifacts = task_tracking_data.setdefault("artifacts", {})
                    if tracked_files_created:
                        existing = set(artifacts.get("files_created", []))
                        existing.update(tracked_files_created)
                        artifacts["files_created"] = list(existing)
                    if tracked_files_modified:
                        existing = set(artifacts.get("files_modified", []))
                        existing.update(tracked_files_modified)
                        artifacts["files_modified"] = list(existing)

                    if not is_error:
                        task_tracking_data["status"] = "completed"
                        # Context isolation at task granularity: clear
                        # developer_state so the next task starts fresh.
                        if "developer_state" in task_tracking_data:
                            save_task_tracking({k: v for k, v in task_tracking_data.items() if k != "developer_state"}, project_path, chat_id)
                            del task_tracking_data["developer_state"]
                    save_task_tracking(task_tracking_data, project_path, chat_id)
            except Exception as e:
                print(f"[DEVELOPER] Error updating task tracking: {e}")
            # ── END ──

            if is_error:
                return make_return({
                    "code": f"// ERROR: {clean_response[:1000]}",
                    "agent_report": _build_dev_report(clean_response, True),
                    "test_report": f"STATUS: FAIL\nDeveloper reported error: {clean_response[:300]}",
                    "project_path": project_path,
                    "code_updated": True,
                    "tech_spec_updated": False,
                })
            # Build exit report with validation -- bare "Completed." is useless
            def _build_dev_report(raw: str, is_err: bool) -> str:
                import re as _re
                files_touched = sorted(set(
                    t.get("args", {}).get("file_path", "")
                    for t in tool_call_log
                    if t.get("tool") in ("edit_file",)
                    and t.get("args", {}).get("file_path")
                ))
                notes = (raw or "").strip()
                if is_err:
                    status = "FAILED"
                elif any(k in notes.lower() for k in ("unable", "stuck", "blocked", "cannot", "waiting")):
                    status = "BLOCKED"
                else:
                    status = "DONE"
                files_line = ", ".join("`%s`" % f for f in files_touched[:15]) or "(none)"
                out = [f"### AGENT HANDOFF REPORT [{status}]",
                       f"- **Agent:** Dev | **Status:** {status}",
                       f"- **Files:** {files_line}"]
                if status == "DONE":
                    out.append("")
                    out.append("#### 1. OUTCOME")
                    out.append(f"- {notes[:500] if notes else 'Implementation complete, tests passing.'}")
                else:
                    out.append("")
                    out.append("#### 1. ERROR CLASSIFICATION")
                    m_exc = _re.search(r"(\w+(?:Error|Exception|Failure|Warning))", notes)
                    exc = m_exc.group(1) if m_exc else "UNKNOWN"
                    m_loc = _re.search(r"([\w./\\-]+\.(?:py|js|ts|php|go|rs|java))[::](\d+)", notes)
                    loc = f" in `{m_loc.group(1)}:{m_loc.group(2)}`" if m_loc else ""
                    out.append(f"- **Primary Exception:** `{exc}`{loc}")
                    out.append("")
                    out.append("#### 2. FAILURE LOCUS & CONTEXT")
                    out.append(f"- **Trigger:** {notes[:400]}")
                    tail = "\n".join([l.strip() for l in notes.splitlines() if l.strip()][-5:])
                    out.append("")
                    out.append("#### 3. MINIMAL ERROR TRAIL (Tail 5 Lines)")
                    out.append("```text")
                    out.append(tail or "(no error output captured)")
                    out.append("```")
                return "\n".join(out)[:1000]  # 1000-char report cap — matches _HANDOFF_MAX_CHARS

            if is_error:
                return make_return({
                    "code": f"// ERROR: {clean_response[:1000]}",
                    "agent_report": _build_dev_report(clean_response, True),
                    "test_report": f"STATUS: FAIL\nDeveloper reported error: {clean_response[:300]}",
                    "project_path": project_path,
                    "code_updated": True,
                    "tech_spec_updated": False,
                })
            else:
                return make_return({
                    "code": "Code successfully implemented.",
                    "agent_report": _build_dev_report(clean_response, False),
                    "test_report": "",
                    "project_path": project_path,
                    "code_updated": True,
                    "tech_spec_updated": False,
                })

        tool_result_payloads = []
        # Failure flags reflect ONLY the previous turn's tool results: they were set
        # by last iteration's tool loop and consumed by this turn's effort selection
        # (think_hard) above. Reset now so THIS turn's results set them fresh.
        last_tool_failed = False
        last_test_failed = False

        for tool_call in tool_calls:
            tool_name = tool_call.get("tool", "")
            tool_args = tool_call.get("args", {})
            if not tool_args or not isinstance(tool_args, dict):
                tool_args = {k: v for k, v in tool_call.items() if k not in ("tool", "args")}

            if tool_name not in valid_tool_set:
                _log(f"[DEVELOPER] Unknown tool requested: {tool_name} â€” asking LLM to retry")
                tool_result = f"TOOL ERROR: Unknown tool: {tool_name}. Available tools: {', '.join(valid_tools)}"
                tool_result_payloads.append((tool_call.get("id"), tool_name, tool_result))
                continue

            # â”€â”€ LoopGuard Pre-Execution Check â”€â”€
            guard_result = LoopGuard.check_pre_execute(tool_call_log, tool_name, tool_args)
            warning_msg = (
                guard_result[1]
                if guard_result and guard_result[0] == "WARNING"
                else None
            )

            # â”€â”€ Format tool log line: concise, shows file + detail â”€â”€
            fpath = tool_args.get("file_path", tool_args.get("path", ""))
            fname = os.path.basename(fpath) if fpath else ""
            # Detect line range from read_file args
            read_offset = tool_args.get("offset", 0)
            read_limit = tool_args.get("limit", None)

            if tool_name == "read_file":
                tool_label = f"read {fname or fpath}"
                if read_offset:
                    tool_label += f" @{read_offset}"
                if read_limit:
                    tool_label += f"+{read_limit}"
            elif tool_name == "edit_file":
                tool_label = f"edit {fname or fpath}"
            elif tool_name == "run_command":
                cmd = str(tool_args.get("command", ""))[:70]
                tool_label = f"run {cmd}"
            elif tool_name == "search_code":
                pat = str(tool_args.get("pattern", ""))[:60]
                tool_label = f"search \"{pat}\""
            elif tool_name == "task":
                tool_label = f"delegate {str(tool_args.get('name', ''))[:40]}"
            elif tool_name == "start_async_task":
                tool_label = f"async {str(tool_args.get('name', ''))[:40]}"
            else:
                tool_label = f"{tool_name}"

            safe_update_state({"thoughts": {"developer": f"Using {tool_name}..."}})
            _log(f"[DEVELOPER] {tool_label}")
            import json
            _log(f"[TOOL_CALL] {tool_name}({json.dumps(tool_args)})")

            try:
                execution = execute_tool_detailed(tool_name, tool_args, scope="Developer")
                tool_result = _bound_tool_result(tool_name, tool_args, execution.content)
                _log(f"[TOOL OUTPUT] {tool_name}: {tool_result}")
            except Exception as e:
                execution = ToolExecutionResult.from_content(
                    tool_name,
                    tool_args,
                    f"Tool execution error: {e}",
                )
                tool_result = execution.content
                _log(f"[TOOL OUTPUT] {tool_name}: {tool_result}")

            # ── Failure tracking for the thinking ladder ──
            # last_tool_failed / last_test_failed are consumed by the NEXT turn's
            # reasoning-effort selection (think_hard) so a failed action forces a
            # high-effort diagnosis turn.
            if execution.is_error:
                last_tool_failed = True
                cmd_low = str(tool_args.get("command", "")).lower()
                if tool_name == "run_command" and any(
                    k in cmd_low for k in ("test", "verify", "pytest", "jest", "check", "lint")
                ):
                    last_test_failed = True

            # â”€â”€ Build detail string for the where_tag (shown in next LLM invocation) â”€â”€
            tool_detail = ""

            # Enrich read_file with actual line range from result
            if tool_name == "read_file":
                lines_match = re.search(r'lines (\d+)-(\d+) of (\d+)', str(tool_result)[:200])
                if lines_match:
                    r_start, r_end, r_total = lines_match.group(1), lines_match.group(2), lines_match.group(3)
                    tool_detail = f"#{r_start}-{r_end}/{r_total}"
                    _log(f"[DEVELOPER]   => {fname or fpath} {tool_detail}")

            # Enrich edit_file with diff stats
            if tool_name == "edit_file" and not str(tool_result).startswith("Error"):
                added = len(re.findall(r'^\+[^+]', str(tool_result)[:2000], re.MULTILINE))
                removed = len(re.findall(r'^\-[^-]', str(tool_result)[:2000], re.MULTILINE))
                if added or removed:
                    tool_detail = f"+{added} -{removed}"
                    _log(f"[DEVELOPER]   => {fname or fpath} {tool_detail}")

            tool_call_log.append({
                "iteration": iteration,
                "tool": tool_name,
                "args": tool_args,
                "status": "error" if execution.is_error else "ok",
                "exit_code": execution.exit_code,
                "timed_out": execution.timed_out,
                "offloaded": execution.offloaded,
                "result_preview": str(tool_result)[:200],
                "detail": tool_detail,  # for where_tag enrichment
            })
            # Remember the last executed tool — the NEXT turn's reasoning ladder
            # uses it to detect the discovery phase (search/list/read → low+cap).
            last_tool = tool_name

            if warning_msg:
                tool_result = str(tool_result) + warning_msg

            # Track artifacts for task.json
            step_tool_calls += 1
            if tool_name == "edit_file":
                fpath = tool_args.get("file_path", "")
                if fpath and fpath not in tracked_files_modified:
                    tracked_files_modified.append(fpath)
            elif tool_name == "template":
                # template(action="apply") writes files outside edit_file — parse
                # the created-file list from the result so task.json artifacts
                # and the handoff report don't under-report template output.
                t_action = str(tool_args.get("action", "")).strip().lower()
                if t_action in ("apply", "use", "render", "create") and not tool_args.get("dry_run"):
                    import re as _re
                    for line in str(tool_result).splitlines():
                        s = line.strip()
                        if not s or s.startswith(("~", "!", "[", "Composed", "TODO")):
                            continue
                        if _re.match(r"^[\w./\\ -]+\.[A-Za-z0-9]{1,6}$", s):
                            norm = s.replace("\\", "/").lstrip("/")
                            if norm not in tracked_files_created:
                                tracked_files_created.append(norm)

            # Format output for this tool
            tool_result_payloads.append((tool_call.get("id"), tool_name, str(tool_result)))

        # Truncate response_text after the first tool block to prevent hallucinations
        last_tool_idx = response_text.rfind("```")
        if last_tool_idx != -1:
            response_text = response_text[:last_tool_idx + 3]

        history_text = re.sub(r'<thinking>.*?</thinking>', '', response_text, flags=re.DOTALL).strip()
        ai_msg_id = f"dev-ai-{iteration}-{uuid.uuid4()}"
        additional_kwargs = {}
        # Only pass reasoning_content back when THIS turn made a tool call.
        # DeepSeek docs: if the model did NOT perform a tool call, the intermediate
        # reasoning_content does not need to participate in context concatenation
        # (and if passed it is IGNORED) — so skipping it on text-only turns is both
        # allowed and saves re-embedding those tokens as cache-miss input.
        if reasoning_content and tool_calls:
            additional_kwargs["reasoning_content"] = reasoning_content
        native_history_calls = [
            {
                "name": tc.get("tool", ""),
                "args": tc.get("args", {}) if isinstance(tc.get("args", {}), dict) else {},
                "id": tc.get("id") or f"dev-call-{iteration}-{idx}",
                "type": "tool_call",
            }
            for idx, tc in enumerate(tool_calls)
            if tc.get("tool")
        ]
        messages.append(AIMessage(
            content=history_text,
            id=ai_msg_id,
            tool_calls=native_history_calls,
            additional_kwargs=additional_kwargs,
        ))

        # Preserve the provider's required assistant-tool-result protocol. A
        # separate result message per call is smaller and more recoverable than
        # merging every result into one anonymous HumanMessage.
        for result_index, (call_id, tool_name, payload) in enumerate(tool_result_payloads):
            result_id = f"dev-tool-{iteration}-{result_index}-{uuid.uuid4().hex[:8]}"
            messages.append(ToolMessage(
                content=f"[{tool_name}]:\n{payload}",
                tool_call_id=call_id or f"dev-call-{iteration}-{result_index}",
                name=tool_name or None,
                id=result_id,
            ))

        # Vision: tools (browser_screenshot / read_file on images) queue inline
        # pixels; inject them as ONE user message right after the ToolMessages —
        # the API accepts images in user messages only.
        try:
            from tools.vision_channel import drain_into_messages
            if drain_into_messages(messages, header="[Inline image(s) from tool result]"):
                safe_append_live_log(f"[VISION] Dev iter {iteration}: inline image(s) injected")
        except Exception:
            pass

        if not tool_result_payloads:
            pending_guidance = [
                "No tool result was returned. Continue with a different action or finish with an honest blocker."
            ]

        # Update shared state with progress
        last_tool_name = tool_calls[-1].get("tool", "") if tool_calls else ""
        if last_tool_name:
            safe_update_state({"outputs": {"code": f"// Developer iteration {iteration}: {last_tool_name} completed"}})

# â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
# Parser & helper functions merged from dev_utils.py
# â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

def compact_failure_report(test_report: str, max_chars: int = 2200) -> str:
    """Keep failure signal without copying source files into the next prompt."""
    if not test_report:
        return ""
    lines = [line.rstrip() for line in str(test_report).splitlines() if line.strip()]
    if not lines:
        return ""
    signal_markers = (
        "traceback", "error", "exception", "failed", "failure", "assert",
        "expected", "actual", "status", "exit code", "stderr", "cannot",
        "undefined", "invalid", "timeout",
    )
    selected: list[str] = []
    seen: set[str] = set()
    for line in lines:
        low = line.lower()
        if any(marker in low for marker in signal_markers):
            key = line.strip()
            if key not in seen:
                selected.append(line)
                seen.add(key)
    for line in lines[-12:]:
        key = line.strip()
        if key not in seen:
            selected.append(line)
            seen.add(key)
    excerpt = "\n".join(selected)
    if len(excerpt) <= max_chars:
        return excerpt
    head = max_chars // 3
    return excerpt[:head].rstrip() + "\n... [failure report compacted] ...\n" + excerpt[-(max_chars - head):].lstrip()


def extract_traceback_file_hints(project_path: str, test_report: str, max_files: int = 12) -> str:
    """Return bounded file/line locations; never read or inject source contents."""
    if not test_report:
        return ""
    extensions = r"py|js|jsx|ts|tsx|go|rs|java|php|rb|cs|cpp|c|h"
    patterns = (
        re.compile(r'File ["\'](?P<path>[^"\']+)["\'], line (?P<line>\d+)', re.IGNORECASE),
        re.compile(
            rf'(?P<path>(?:[A-Za-z]:[\\/])?[^()\s"\']+\.(?:{extensions})):(?P<line>\d+)(?::\d+)?',
            re.IGNORECASE,
        ),
        re.compile(
            rf'(?P<path>(?:[A-Za-z]:[\\/])?[\w./\\-]+\.(?:{extensions}))\b',
            re.IGNORECASE,
        ),
    )
    hints: list[str] = []
    seen: dict[str, tuple[int, str]] = {}
    root = os.path.abspath(project_path) if project_path else ""
    for pattern in patterns:
        for match in pattern.finditer(str(test_report)):
            raw_path = match.group("path").strip().strip(".,;)")
            if not raw_path:
                continue
            line = match.groupdict().get("line") or ""
            normalized = raw_path.replace("\\", "/")
            if root and os.path.isabs(raw_path):
                try:
                    relative = os.path.relpath(raw_path, root)
                    if not relative.startswith(".."):
                        normalized = relative.replace("\\", "/")
                except (OSError, ValueError):
                    pass
            normalized = normalized.lstrip("./") or raw_path
            key = normalized.lower()
            if key in seen:
                existing_index, existing_line = seen[key]
                if line and not existing_line:
                    hints[existing_index] = f"- {normalized}:{line}"
                    seen[key] = (existing_index, line)
                continue
            seen[key] = (len(hints), line)
            hints.append(f"- {normalized}{':' + line if line else ''}")
            if len(hints) >= max_files:
                break
        if len(hints) >= max_files:
            break
    return "## Failure Locations\n" + "\n".join(hints) if hints else ""
