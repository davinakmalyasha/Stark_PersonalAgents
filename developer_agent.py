import os
import json
import re
import functools
import subprocess
import uuid
import time
import threading
import requests
from datetime import datetime
from langchain_core.messages import SystemMessage, HumanMessage, AIMessage, RemoveMessage
from state_sync import safe_get_state, safe_update_state, safe_append_live_log
from it_department_nodes_base import ITState
from tools import TOOL_DEFINITIONS, execute_tool, list_files
from llm import invoke_messages_with_fallback
from deepagents import HarnessProfile, register_harness_profile

try:
    import deepagents.profiles.harness.harness_profiles as hp
except Exception:
    hp = None

# Local helper imports moved to top level
from workspace_manager import get_workspace_rules_and_profile
from dev_memory_helper import get_developer_memory_context
from loop_detector import detect_stagnation_or_loop, LoopGuard
from sync_helpers import load_task_tracking, save_task_tracking, build_task_progress_block
from chat_context import build_chat_context
from context_budget import estimate_tokens
from context_compaction import (
    build_structured_resume_summary,
    invalidate_stale_reads,
    tier1_compact,
    tier2_compact,
    checkpoint_compact,
    prune_stale_tool_outputs
)
from tool_schemas import get_native_tools
from deterministic_checker import run_deterministic_cascade
from dev_lint import lint_and_fix
from developer_prompt import _STATIC_SYSTEM_TEMPLATE

def get_harness_profile(name: str):
    if hp is not None:
        try:
            return hp._HARNESS_PROFILES.get(name)
        except Exception:
            return None
    return None

MAX_ITERATIONS = 1_000  # Safety ceiling only — LLM decides when done. Real exit is voluntary.

# Tools that belong to the DISCOVERY phase: the turn AFTER a successful
# search/list/read is still exploring, so it gets a tight reasoning budget
# (BSA_REASONING_DISCOVERY + BSA_REASONING_DISCOVERY_BUDGET) instead of none.
_DISCOVERY_TOOLS = frozenset({
    "search_codebase", "search_code", "list_files", "read_file", "view_signatures",
})


def _detect_test_command(project_path: str) -> str:
    """Detect the appropriate test command for the project type."""
    if not project_path or not os.path.isdir(project_path):
        return ""
    
    # Python: pytest
    py_test_files = [f for f in os.listdir(project_path) 
                     if f.startswith("test_") and f.endswith(".py")]
    if py_test_files:
        return "pytest"
    
    # Node.js: npm test
    pkg_json = os.path.join(project_path, "package.json")

    if os.path.isfile(pkg_json):
        try:
            with open(pkg_json, "r") as f:
                pkg = json.load(f)
            if pkg.get("scripts", {}).get("test"):
                return "npm test"
        except Exception:
            pass
    
    # Go: go test
    go_test_files = [f for f in os.listdir(project_path) 
                     if f.endswith("_test.go")]
    if go_test_files:
        return "go test ./..."
    
    # PHP/Laravel: php artisan test
    if os.path.isfile(os.path.join(project_path, "artisan")):
        return "php artisan test"
    
    return ""

# Dev system prompt lives in developer_prompt.py (imported above).
# Dev system prompt imported from developer_prompt.py



# Dev system prompt lives in developer_prompt.py (imported above).

def get_compact_file_list(project_path: str) -> str:
    """
    Returns a highly compact comma-separated list of all project files.
    Single-user optimization: gives the agent complete recursive visibility of all files
    at 80% fewer tokens than a recursive tree list.
    """
    import os
    safe_path = project_path or "."
    ignored_dirs = {
        "node_modules", "venv", ".venv", ".git", "__pycache__", 
        ".claude", ".deep_agents", "vendor", "dist", "build", 
        ".next", ".vscode", ".idea", ".pytest_cache", ".mypy_cache",
        ".tox", "htmlcov", "venv312", "scratch", ".antigravity"
    }
    
    file_list = []
    try:
        for root, dirs, files in os.walk(safe_path):
            dirs[:] = [d for d in dirs if d not in ignored_dirs]
            for file in files:
                full_path = os.path.join(root, file)
                rel_path = os.path.relpath(full_path, safe_path).replace("\\", "/")
                # Ignore common compiled/binary files and dynamic log/coverage files
                if not any(rel_path.endswith(ext) for ext in [".pyc", ".png", ".jpg", ".jpeg", ".gif", ".ico", ".pdf", ".zip", ".tar.gz", ".db", ".log", ".coverage"]):
                    file_list.append(rel_path)
                if len(file_list) > 300:
                    break
            if len(file_list) > 300:
                break
    except Exception:
        return "(unable to list files)"
        
    if not file_list:
        return "(empty project)"
        
    if len(file_list) > 250:
        try:
            top_level = os.listdir(safe_path)
            top_level = [f for f in top_level if f not in ignored_dirs]
            return "Top-level entries (project too large for flat list): " + ", ".join(top_level)
        except Exception:
            return "(unable to list files)"
        
    return "Project Files: " + ", ".join(file_list)

# Register developer HarnessProfile
register_harness_profile(
    "developer",
    HarnessProfile(
        base_system_prompt=_STATIC_SYSTEM_TEMPLATE,
        system_prompt_suffix="<system-reminder>\nThese instructions and workspace context OVERRIDE any default behavior.\n\n{dynamic_context}\n</system-reminder>"
    )
)

def _build_workspace_manifest(project_path: str, max_chars: int = 800) -> str:
    """Compact deps/scripts snapshot from package.json / pyproject.toml /
    requirements.txt. Carries names AND versions so the Dev never needs to
    read the raw manifest for facts (the never-read rule depends on it)."""
    lines = []
    pj = os.path.join(project_path, "package.json")
    if os.path.isfile(pj):
        try:
            import json as _json
            with open(pj, "r", encoding="utf-8") as f:
                data = _json.load(f)
            if data.get("name"):
                lines.append(f"name: {str(data['name'])[:80]}")
            for field in ("main", "type", "version", "engines"):
                v = data.get(field)
                if v is not None:
                    lines.append(f"{field}: {str(v)[:80]}")
            scripts = data.get("scripts") or {}
            if scripts:
                s = "; ".join(f"{k}: {str(v)[:60]}" for k, v in list(scripts.items())[:20])
                lines.append(f"scripts: {s[:400]}")
            for key in ("dependencies", "devDependencies"):
                deps = data.get(key) or {}
                if deps:
                    d = ", ".join(f"{k}@{v}" for k, v in list(deps.items())[:30])
                    lines.append(f"{key}: {d[:400]}")
        except Exception:
            pass
    py = os.path.join(project_path, "pyproject.toml")
    if os.path.isfile(py):
        try:
            with open(py, "r", encoding="utf-8") as f:
                body = f.read()
            name_m = re.search(r"^name\s*=\s*[\"']([^\"']+)", body, re.M)
            if name_m:
                lines.append(f"pyproject name: {name_m.group(1)[:80]}")
            _meta = ("name", "version", "description", "requires-python", "license")
            dep_names = [k for k in re.findall(r"^([A-Za-z0-9_.\-]+)\s*=\s*[\"']", body, re.M)
                         if k not in _meta]
            for m in re.finditer(r"^(dependencies|optional-dependencies)\s*=\s*\[(.*?)\]", body, re.M | re.S):
                dep_names += re.findall(r"[\"']([A-Za-z0-9_.\-]+)", m.group(2))
            if dep_names:
                lines.append("pyproject deps: " + ", ".join(dict.fromkeys(dep_names))[:300])
        except Exception:
            pass
    req = os.path.join(project_path, "requirements.txt")
    if os.path.isfile(req):
        try:
            with open(req, "r", encoding="utf-8") as f:
                reqs = [ln.strip() for ln in f if ln.strip() and not ln.strip().startswith(("#", "-"))]
            if reqs:
                lines.append("requirements: " + ", ".join(r.split("==")[0].split(">=")[0] for r in reqs[:20]))
        except Exception:
            pass
    if not lines:
        return ""
    return "\n".join(lines)[:max_chars]


def _build_dynamic_context(task: str, project_path: str, context: str, is_fixing: bool) -> tuple[str, str]:
    """
    Build dynamic prompt elements, split into stable and volatile parts.

    Returns:
        (stable_context, volatile_context)
        - stable_context: Byte-identical per project_path â€” safe in SystemMessage (cached).
        - volatile_context: Changes per task â€” must go at TAIL of HumanMessage (cache miss).

    The Three-Zone Cache Architecture (Reasonix pattern):
      Zone 1 (Immutable Prefix): SystemMessage = base_prompt + tools + stable_context
      Zone 2 (Task-Specific Tail): HumanMessage = task + volatile_context
      Zone 3 (Append-Only Log): conversation turns

    This pushes cache hit rate from ~60% to 85%+ across diverse tasks
    because the cache break point moves from mid-context to the very end.
    """
    safe_path = project_path or "."
    wdir = project_path or (os.environ.get("BSA_HARNESS_ROOT") or os.path.dirname(os.path.abspath(__file__)))

    # ── STABLE: Byte-identical for a given project_path ──
    # NOTE: `## Working Directory` is deliberately NOT here — it contains the
    # task-specific tmp path, which differs per task/run and would break DeepSeek
    # prefix-cache identity for the entire 16.7K system prompt. It goes in the
    # VOLATILE tail instead (line below), so the SystemMessage is byte-identical
    # across ALL tasks and the huge sysprompt hits cache on every fresh chat.
    stable_parts = []

    # Load rules and profile (static for a given project run)
    try:
        rules_context = get_workspace_rules_and_profile(project_path)
        if rules_context:
            stable_parts.append(f"## Workspace Rules & Profile\n{rules_context}")
    except Exception as e:
        print(f"Error loading rules context for developer: {e}")

    # Cached repository map — LAST stable section (append-only tail growth).
    # Byte-identical per project_path, so it cache-hits after the first call;
    # new-file appends only ever touch the map's bottom (prefix stays cached).
    # Never injected here fresh: `load_cached_code_map` is read-only; the map
    # is generated/persisted by refresh_code_map (eval attempt / session start).
    try:
        from memory_manager import load_cached_code_map
        cached_map = load_cached_code_map(safe_path)
        if cached_map:
            stable_parts.append(f"## Repository Map (cached, append-only)\n{cached_map}")
    except Exception as e:
        print(f"Error loading cached code map: {e}")

    # Project identity card — THE last stable section. It only ever grows by
    # rare [updated:] appends at its own bottom, so keeping it at the very
    # tail keeps those appends cache-clean (map appends shift only this tiny
    # section — negligible re-miss).
    try:
        from memory_manager import load_cached_project_identity
        project_card = load_cached_project_identity(safe_path)
        if project_card:
            stable_parts.append(f"## Project Overview\n{project_card}")
    except Exception as e:
        print(f"Error loading project identity: {e}")

    # ── VOLATILE: Changes per task — repo map extracts hot_files from task text ──
    volatile_parts = []
    # Working directory is task-specific (tmp path) — keep it in the volatile tail
    # so it does NOT break the cached system-prompt prefix.
    volatile_parts.append(f"## Working Directory\n{wdir}")

    # Project structure (moved to volatile context to maximize Zone 1 KV cache hits since files can be added/deleted during execution)
    project_tree = get_compact_file_list(safe_path)
    volatile_parts.append(f"## Project Structure\n{project_tree}")

    # Workspace manifest — compact deps/scripts snapshot so the Dev never needs
    # to read package.json / pyproject.toml / requirements.txt for project facts.
    # Volatile (manifests change during execution); skipped when absent.
    try:
        manifest = _build_workspace_manifest(safe_path)
        if manifest:
            volatile_parts.append(f"## Workspace Manifest\n{manifest}")
    except Exception:
        pass

    # Task-relevant file hints (volatile — extracted from the task text).
    # The repository map itself is NOT regenerated per session anymore: it is
    # persisted append-only and injected via the cached stable section above.
    hot_files = []
    try:
        task_context_combined = f"{task}\n{context}"
        file_pattern = r'\b[a-zA-Z0-9_\-\/\\.]+\.(?:py|php|ts|tsx|js|jsx|dart)\b'
        hot_files = list(dict.fromkeys(re.findall(file_pattern, task_context_combined)))
    except Exception:
        pass
    if hot_files:
        volatile_parts.append(f"## Task-relevant files\n{', '.join(hot_files[:10])}")

    # Additional context (changes per call)
    if context:
        volatile_parts.append(f"## Additional Context\n{context}")

    # Memory context — full block (project overview + code map + relevant features)
    # on fresh sessions; compact code map only when fixing (a Dev repairing a bug
    # needs the symbol index most, and full features can steer it astray).
    try:
        memory_context = get_developer_memory_context(project_path, task, include_code_map_only=is_fixing)
        if memory_context:
            volatile_parts.append(memory_context)
    except Exception as e:
        print(f"Error loading memory context for developer: {e}")



    return "\n\n".join(stable_parts), "\n\n".join(volatile_parts)

def _build_system_prompt(task: str, project_path: str, context: str = "", valid_tools_list: list[str] = None, is_first_call: bool = True, is_fixing: bool = False) -> tuple[str, str]:
    """
    Build the system prompt with Three-Zone Cache Architecture.

    Zone 1 (Immutable Prefix â†’ SystemMessage):
      base_prompt + tools_block + example + STABLE dynamic parts
      (working directory, project structure, workspace rules)
      â†’ 100% byte-identical per project_path â†’ DEEPSEEK CACHE HIT

    Zone 2 (Task-Specific Tail â†’ HumanMessage):
      task + VOLATILE dynamic parts
      (repo map, memory context, skills context)
      â†’ Changes per task â†’ cache miss (but only ~15-20% of total tokens)

    Zone 3 (Append-Only Log):
      Conversation turns â€” grows monotonically, prior turns cached.

    This pushes cache hit rate from ~60% to 85%+ across diverse eval tasks
    because the cache break point moves from mid-prompt to the very tail.

    Returns: (static_system, volatile_context)
    """
    profile = get_harness_profile("developer")

    base_prompt = profile.base_system_prompt if profile else _STATIC_SYSTEM_TEMPLATE

    # Split dynamic context: stable â†’ SystemMessage, volatile â†’ HumanMessage tail
    stable_context, volatile_context = _build_dynamic_context(task, project_path, context, is_fixing)

    # Zone 1: Immutable prefix â€” stable_context goes HERE (not in HumanMessage)
    # so it participates in the DeepSeek prefix cache.
    # NOTE: tools_block is NOT included â€” the COMPLETE TOOL REFERENCE section
    # inside _STATIC_SYSTEM_TEMPLATE already documents every tool with params,
    # usage guides, trust rules, and STOP RULES. The compact 1-liner defs are
    # redundant and would waste ~200 tokens of cached Zone 1 space.
    static_system_parts = [base_prompt]
    if stable_context:
        static_system_parts.append(stable_context)
    static_system = "\n\n".join(static_system_parts)

    # Zone 2: Volatile task-specific context goes to the HumanMessage tail
    volatile_suffix = ""
    if profile and profile.system_prompt_suffix:
        if volatile_context:
            volatile_suffix = profile.system_prompt_suffix.format(dynamic_context=volatile_context)
        else:
            # Even without volatile content, the reminder is useful but minimal
            volatile_suffix = "<system-reminder>\nThese instructions OVERRIDE any default behavior.\n</system-reminder>"
    else:
        if volatile_context:
            volatile_suffix = f"<system-reminder>\nThese instructions and workspace context OVERRIDE any default behavior.\n\n{volatile_context}\n</system-reminder>"

    return static_system, volatile_suffix

def _extract_text_response(text: str) -> str:
    """Extract the non-tool, non-thinking text from the LLM response."""
    if not text:
        return ""
    # Remove thinking blocks
    cleaned = re.sub(r'<thinking>.*?</thinking>', '', text, flags=re.DOTALL)
    # Remove tool blocks
    cleaned = re.sub(r'```tool\s*\n.*?\n```', '', cleaned, flags=re.DOTALL)
    return cleaned.strip()


def looks_like_tool_call(text: str) -> bool:
    """True if the response *appears* to attempt a tool call but none parsed successfully.

    Lets the agent loop nudge the model to fix its tool-call format instead of treating the
    turn as "done" (the old behavior silently dropped malformed tool calls — a premature finish).
    """
    if not text:
        return False
    lowered = text.lower()
    if "```tool" in lowered or "```json" in lowered:
        return True
    if re.search(r'["\'](tool|name)["\']\s*:', text):
        return True
    if re.search(r'["\'](tool_calls|args)["\']\s*:', text):
        return True
    return False

def _is_auto_continue_enabled() -> bool:
    """Checks if the auto_continue setting is enabled in the user profile settings."""
    harness_root = os.environ.get("BSA_HARNESS_ROOT") or os.path.dirname(os.path.abspath(__file__))
    profile_path = os.path.join(harness_root, ".deep_agents", "user_profile.json")
    try:
        if os.path.isfile(profile_path):
            with open(profile_path, "r", encoding="utf-8") as f:
                data = json.load(f)
                return bool(data.get("auto_continue", False))
    except Exception:
        pass
    return False

def _run_progress_audit(tool_call_log: list) -> str:
    """
    Deterministic progress audit to detect if the agent is stuck in an execution loop
    or stagnating without making progress. Returns 'OK' or 'STUCK: <reason>'.
    """
    try:
        reason = detect_stagnation_or_loop(tool_call_log)
        if reason:
            return f"STUCK: {reason}"
    except Exception as e:
        _log(f"[AUDITOR] Warning: Loop detection failed: {e}")
    return "OK"

def _log(msg: str) -> None:
    """Log to console and the shared state live terminal."""
    try:
        print(msg, flush=True)
    except Exception:
        pass
    # Appends under lock instead of snapshot+overwrite — the old pattern could
    # drop lines appended by other threads between the read and the write.
    safe_append_live_log(msg)

def count_identical_calls_since_state_change(tool_call_log: list, tool_name: str, tool_args: dict) -> int:
    """
    Counts how many times this exact tool and arguments have been called
    since the last state-altering action in the current run.
    """
    count = 0
    canonical_args = json.dumps(tool_args, sort_keys=True)
    
    for item in reversed(tool_call_log):
        item_tool = item.get("tool")
        item_args = item.get("args", {})
        
        # If it's the exact same tool and arguments, increment
        if item_tool == tool_name and json.dumps(item_args, sort_keys=True) == canonical_args:
            count += 1
            continue
            
        # Break on state-changing file tools (edits or writes)
        if item_tool in ("write_file", "edit_file", "apply_diff"):
            break
            
        # Break on a different command execution (since different commands indicate progress/change)
        if item_tool == "run_command":
            break
            
    return count

def serialize_messages(messages: list) -> list[dict]:
    """Helper to serialize messages, preserving type, content, id, tool_calls, and additional_kwargs."""
    serialized = []
    for m in messages:
        msg_dict = {"type": type(m).__name__, "content": m.content}
        if hasattr(m, "id") and m.id:
            msg_dict["id"] = m.id
        if hasattr(m, "name") and m.name:
            msg_dict["name"] = m.name
        if hasattr(m, "tool_calls") and m.tool_calls:
            msg_dict["tool_calls"] = m.tool_calls
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
                kwargs["tool_calls"] = tool_calls
            msg = AIMessage(content=content, **kwargs)
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
    consecutive_read_turns = 0
    self_verify_retries = 0
    reflexion_retries = 0
    voluntary_compact_requested = False

    initial_msg_count = len(s.get("messages", [])) if (hasattr(s, "get") and s.get("messages")) else 0
    messages = []
    appended_ids = []

    def make_return(res_dict: dict) -> dict:
        new_msgs = [m for m in messages if getattr(m, "id", None) not in existing_ids]
        if len(appended_ids) > 2:
            prune_ids = appended_ids[:-2]
            for pid in prune_ids:
                new_msgs.append(RemoveMessage(id=pid))
        res_dict["messages"] = new_msgs
        
        # Expose updated remaining_steps to LangGraph
        res_dict["remaining_steps"] = safe_get_state().get("remaining_steps", 40)

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

            # Append to message history (Zone 3)
            diff_msg_id = f"dev-git-diff-{uuid.uuid4()}"
            diff_msg = SystemMessage(
                content=f"[SYSTEM MEMORY INFO] The following git diff represents the final changes applied during this agent turn:\n\n```diff\n{git_diff}\n```",
                id=diff_msg_id
            )
            # Add to res_dict["messages"] so it gets returned and stored in history
            res_dict["messages"].append(diff_msg)

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
                    "current_step": 0,
                    "steps": [{"id": 1, "description": client_req, "status": "in_progress"}],
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
                        "consecutive_read_turns": consecutive_read_turns,
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
    project_path = s.get("project_path", "") or (os.environ.get("BSA_HARNESS_ROOT") or os.path.dirname(os.path.abspath(__file__)))
    err_count = s.get("error_count", 0)
    chat_id = s.get("chat_id", "")

    # â”€â”€ Loop-Breaker Git Rollback (#20) â”€â”€
    if err_count > 1:
        try:
            safe_update_state({"thoughts": {"developer": "Loop detected. Rolling back workspace to last stable commit..."}})
            subprocess.run(["git", "reset", "--hard", "HEAD"], cwd=project_path, capture_output=True, timeout=10)
            subprocess.run(["git", "clean", "-fd"], cwd=project_path, capture_output=True, timeout=10)
        except Exception as e:
            print(f"[DEVELOPER] Error doing loop-breaker git rollback: {e}")
    spec_context = ""
    if tech_spec:
        spec_context = f"\n\n## Technical Specification\n{tech_spec[:3000]}\n\n## Requirements\n{requirements[:2000]}"
    if is_fixing and test_report:
        injected_files_context = extract_traceback_files_context(project_path, test_report)
        if injected_files_context:
            spec_context += f"\n\n## Relevant Files (from traceback)\n{injected_files_context}"

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

    # â”€â”€ Git Resumption Change Tracking (#15) â”€â”€
    modified_files = []
    try:
        # Check status
        res1 = subprocess.run(["git", "status", "--porcelain"], capture_output=True, text=True, cwd=project_path, timeout=3)
        if res1.returncode == 0:
            for line in res1.stdout.splitlines():
                parts = line.strip().split(None, 1)
                if len(parts) > 1:
                    modified_files.append(parts[1])
        # Check diff
        res2 = subprocess.run(["git", "diff", "--name-only", "HEAD~3"], capture_output=True, text=True, cwd=project_path, timeout=3)
        if res2.returncode == 0:
            modified_files.extend(res2.stdout.splitlines())
    except Exception:
        pass
    modified_files = sorted(list(set(modified_files)))

    # Add error history context if fixing
    context = ""
    if modified_files:
        files_str = ", ".join(modified_files)
        context += f"\n## Recently Modified Files (Git Change Tracking)\nThe following files were recently modified or have uncommitted changes: {files_str}. Check these first to understand what has already been changed.\n"

    if err_count > 0:
        context += f"\nThis task has failed {err_count} times before. Be extra thorough.\n"
        context += (
            "CRITICAL: Do NOT repeat the exact same changes or debugging approach that was attempted in the previous failed runs. "
            "If your previous attempt failed the tests, that approach is proven incorrect. "
            "Formulate a completely different hypothesis, verify file paths, double check imports, "
            "and inspect the error traceback details carefully before writing code.\n"
        )

    # â”€â”€ Load chat history from workspace chat file so the developer remembers the context â”€â”€
    chat_history_str = build_chat_context(project_path, chat_id, max_messages=30, max_chars_per_msg=3000)

    if chat_history_str:
        context += f"\n## Recent Chat History\nUse this to understand the context of the request and what has been completed or discussed:\n{chat_history_str}\n\n"

    # â”€â”€ Load task progress so the developer knows what was done vs remaining â”€â”€
    task_data = None
    task_step_info = ""
    try:
        task_data = load_task_tracking(project_path, chat_id)
        if task_data and task_data.get("steps"):
            steps_data = task_data["steps"]
            current_idx = task_data.get("current_step", 0)
            if current_idx < len(steps_data):
                # Mark this step as in_progress
                steps_data[current_idx]["status"] = "in_progress"
                save_task_tracking(task_data, project_path, chat_id)
                task_step_info = "\n" + build_task_progress_block(task_data) + "\n"
    except Exception as e:
        print(f"[DEVELOPER] Error loading task context: {e}")
    # â”€â”€ END â”€â”€

    if task_step_info:
        context += task_step_info

    valid_tools = ["read_file", "view_signatures", "write_file", "edit_file", "run_command",
                   "search_code", "list_files", "compact_conversation",
                   "query_history", "search_codebase",
                   "web_fetch", "browser_navigate", "browser_extract",
                   "browser_screenshot", "browser_close", "revert_last_change"]

    # Configurable iteration ceiling — BSA_DEV_MAX_ITERS lets the eval harness
    # cap a single Dev run (default 1000 = safety-only, LLM exits voluntarily).
    max_iters = int(os.environ.get("BSA_DEV_MAX_ITERS", str(MAX_ITERATIONS)))

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

    # Build system prompt parts
    is_fixing = bool(test_report)
    task_message = task
    static_system, volatile_context = _build_system_prompt(
        task_message, project_path, context, valid_tools, is_first_call=(developer_state is None), is_fixing=is_fixing
    )

    task_human_content = task_message
    if volatile_context:
        task_human_content += f"\n\n{volatile_context}"

    if developer_state:
        _log(f"[DEVELOPER] ðŸ”„ Resuming from suspended execution state (iteration {developer_state['iteration']})...")
        if developer_state.get("messages"):
            messages = deserialize_messages(developer_state["messages"])
        elif hasattr(s, "get") and s.get("messages") and len(s.get("messages")) > 0:
            messages = list(s.get("messages"))
            _log(f"[DEVELOPER] Loaded {len(messages)} messages natively from Graph State.")
        else:
            messages = []

        # Update SystemMessage and first HumanMessage in history to avoid stale rules/project tree on resume
        if len(messages) >= 2:
            if isinstance(messages[0], SystemMessage):
                messages[0] = SystemMessage(content=static_system)
            if isinstance(messages[1], HumanMessage):
                messages[1] = HumanMessage(content=task_human_content)

        # Check if we are resuming from suspension vs starting a new repair/implement cycle
        resume_words = {"continue", "resume", "proceed", "go"}
        req_words = client_req.lower().strip().rstrip(".!?").split()
        is_resume_prompt = bool(resume_words & set(req_words)) or client_req.lower().strip().rstrip(".!?") in ["go on", "go"]

        if is_resume_prompt:
            iteration = developer_state["iteration"]
            step_tool_calls = developer_state["step_tool_calls"]
            consecutive_read_turns = developer_state.get("consecutive_read_turns", 0)
        else:
            iteration = 0  # repair/implement cycle resets the iteration budget (referenced later in compaction summary)
            step_tool_calls = 0
            consecutive_read_turns = 0
            voluntary_compact_requested = False
            # Append the new task traceback / client request as a human message to the end of history
            messages.append(HumanMessage(content=task_human_content))
            _log("[DEVELOPER] ðŸ”„ Starting a new repair/implement cycle. Reset iteration budget and appended the new task description.")

        tool_call_log = developer_state["tool_call_log"]
        tracked_files_created = developer_state["tracked_files_created"]
        tracked_files_modified = developer_state["tracked_files_modified"]
        last_response_text = developer_state["last_response_text"]
        
        # Only trigger if total tokens exceed 100K (keeps the cache intact for small-to-medium histories)
        total_tokens_on_resume = sum(estimate_tokens(m.content) for m in messages if hasattr(m, "content"))
        if len(messages) > 12 and total_tokens_on_resume > 100000:
            # Three-zone resume: keep SystemMessage [0] (Zone 1 cache target),
            # task HumanMessage [1] (Zone 2), and last 6 messages (3 turns).
            # The middle messages are compacted into a single summary.

            summary_content = build_structured_resume_summary(
                tool_call_log,
                tracked_files_created,
                tracked_files_modified
            )

            summary_header = f"[SYSTEM RESUME INFO] In iterations 1 to {max(1, iteration)}, the following progress was made:\n\n"
            summary_msg = HumanMessage(
                content=summary_header + summary_content + "\n\nMessage history of these turns has been compacted to save tokens.",
                id=f"resume-summary-{uuid.uuid4()}"
            )
            appended_ids.append(summary_msg.id)

            messages = [messages[0], messages[1], summary_msg] + messages[-6:]
            _log(f"[DEVELOPER] ðŸ§¹ Aggressive structured context compaction: reduced history from {len(developer_state['messages'])} to {len(messages)} messages.")
            _log(f"[DEVELOPER] ðŸ§¹ Compacted summary details:\n{summary_content}")
    else:
        # Zone 1 + Zone 2: SystemMessage (cached) + task+volatile (cache break)
        messages = [
            SystemMessage(content=static_system),
            HumanMessage(content=task_human_content),
        ]
        iteration = 0
        tool_call_log = []
        last_response_text = ""
        step_tool_calls = 0
        tracked_files_created = []
        tracked_files_modified = []
        # Reset the consecutive read turns count for the fresh run
        consecutive_read_turns = 0

    existing_ids = {m.id for m in messages if getattr(m, "id", None)}

    _log(f"\n{'='*60}\n[DEVELOPER] Starting tool-using agentic loop\n{'='*60}")

    # Reset per-run counters
    self_verify_retries = 0
    reflexion_retries = 0
    malformed_retries = 0
    last_tool_failed = False  # previous turn: a tool errored or command exited non-zero
    last_test_failed = False  # previous turn: a test/verify command failed
    last_tool = None          # previous turn's last executed tool (drives the discovery rung)

    while True:
        iteration += 1
        _log(f"\n[DEV Iteration {iteration}]")

        # ── Safety ceiling (should never be reached — LLM decides when done) ──
        if iteration > max_iters:
            _log(f"[DEVELOPER] Safety ceiling ({max_iters}) reached — saving checkpoint and forcing exit.")
            safe_update_state({"thoughts": {"developer": f"Hit safety ceiling ({max_iters}). Saving work and exiting."}})
            # Save checkpoint
            try:
                task_tracking_data = load_task_tracking(project_path, chat_id)
                if task_tracking_data and task_tracking_data.get("steps"):
                    steps_data = task_tracking_data["steps"]
                    current_idx = task_tracking_data.get("current_step", 0)
                    if current_idx < len(steps_data):
                        steps_data[current_idx]["tool_calls"] = step_tool_calls
                        steps_data[current_idx]["notes"] = f"Safety ceiling at iteration {max_iters}"
                    artifacts = task_tracking_data.setdefault("artifacts", {})
                    if tracked_files_created:
                        existing = set(artifacts.get("files_created", []))
                        existing.update(tracked_files_created)
                        artifacts["files_created"] = list(existing)
                    if tracked_files_modified:
                        existing = set(artifacts.get("files_modified", []))
                        existing.update(tracked_files_modified)
                        artifacts["files_modified"] = list(existing)
                    save_task_tracking(task_tracking_data, project_path, chat_id)
            except Exception:
                pass
            # A4: salvage partial work — capture workspace delta for the report
            salvage = ""
            try:
                from tools import capture_workspace_delta
                salvage = capture_workspace_delta(max_chars=4000)
            except Exception:
                pass
            salvage_block = f"\n\n{salvage}" if salvage else ""
            return make_return({
                "code": "Safety ceiling reached — partial work saved." + salvage_block[:800],
                "agent_report": (_build_dev_report(last_response_text, False, []) if 'last_response_text' in dir() else "## Dev Summary\nStatus: CEILING\nWork was in progress when the safety iteration ceiling was reached.") + salvage_block[:4000],
                "test_report": "",
                "project_path": project_path,
                "code_updated": bool(tracked_files_created or tracked_files_modified),
                "tech_spec_updated": False,
            })

        # Invalidate stale read results to prevent temporal paradoxes
        messages = invalidate_stale_reads(messages)

        # Decrement remaining_steps in shared_state to reflect the progress of the loop
        state_snap = safe_get_state()
        rem_steps = (state_snap.get("remaining_steps") or 50) - 1
        safe_update_state({"remaining_steps": rem_steps})
        if rem_steps <= 0:
            _log("[DEVELOPER] âš ï¸ Out of remaining steps budget â€” forcing exit.")
            break

        # â”€â”€ Auto-compact: Claude Code style context management â”€â”€
        # When context grows too large, apply 3-Tier compaction strategy based on token budget

        # Three-zone: history starts at index 2 (after SystemMessage + Task HumanMessage)
        history_tokens = sum(estimate_tokens(m.content) for m in messages[2:]) if len(messages) > 2 else 0
        total_tokens = sum(estimate_tokens(m.content) for m in messages if hasattr(m, "content"))
        # Check if the agent requested on-demand compaction via the compact_conversation tool
        voluntary_compact = False
        for msg in messages:
            if hasattr(msg, "content") and "compact_conversation" in str(msg.content):
                voluntary_compact = True
                break

        # Pillar 111: Adaptive threshold â€” shallower runs tolerate more, deep loops compact earlier
        keep_n = 12
        # B5: token-budgeted tail (opencode select() pattern) instead of fixed count.
        # B6: incremental tier-0 prune runs before the full tiers. Both gated by BSA_COMPACT_V2.
        compact_v2 = os.environ.get("BSA_COMPACT_V2", "1") == "1"
        tail_budget = int(os.environ.get("BSA_TAIL_BUDGET_TOKENS", "100000")) if compact_v2 else None

        # Tier-0: cheap incremental prune before any full-tier rewrite
        if compact_v2 and total_tokens > 600000:
            messages, pruned_chars = prune_stale_tool_outputs(messages)
            if pruned_chars:
                _log(f"[DEVELOPER] Tier-0 prune freed {pruned_chars:,} chars of old tool outputs")
                total_tokens = sum(estimate_tokens(m.content) for m in messages if hasattr(m, "content"))

        # 3-Tier Progressive Compaction Strategy
        if voluntary_compact:
            _log(f"[DEVELOPER] Voluntary compaction requested. Running Tier 3 Checkpoint-Based Compaction.")
            if len(messages) > keep_n + 2:
                messages = checkpoint_compact(
                    messages,
                    tool_call_log,
                    tracked_files_created,
                    tracked_files_modified,
                    keep_last_n=keep_n,
                    tail_budget_tokens=tail_budget
                )
                _log(f"[DEVELOPER] Compacted history -> {len(messages)} remain")
        elif total_tokens > 800000:
            _log(f"[DEVELOPER] Auto-compact: Tier 3 trigger (tokens={total_tokens})")
            if len(messages) > keep_n + 2:
                _log("[DEVELOPER] Checkpoint-Based Compaction (collapsing middle history to structured summary)")
                messages = checkpoint_compact(
                    messages,
                    tool_call_log,
                    tracked_files_created,
                    tracked_files_modified,
                    keep_last_n=keep_n,
                    tail_budget_tokens=tail_budget
                )
                _log(f"[DEVELOPER] Compacted history -> {len(messages)} remain")
        elif total_tokens > 600000:
            _log(f"[DEVELOPER] Auto-compact: Tier 2 trigger (tokens={total_tokens} > 600K)")
            if len(messages) > keep_n + 2:
                messages = tier2_compact(messages, keep_last_n=keep_n, tail_budget_tokens=tail_budget)
                _log(f"[DEVELOPER] Compacted history -> {len(messages)} remain")
        elif total_tokens > 400000:
            _log(f"[DEVELOPER] Auto-compact: Tier 1 trigger (tokens={total_tokens} > 400K)")
            if len(messages) > keep_n + 2:
                messages = tier1_compact(messages, keep_last_n=keep_n, tail_budget_tokens=tail_budget)
                _log(f"[DEVELOPER] Compacted history -> {len(messages)} remain")

        # Metacognitive Progress Audit ————————
        # Check every iteration once past the warm-up. A detection appends an
        # ACTIONABLE steering message — the goal is to UNSTICK the agent and get
        # it solving (reproduce → fix), never to silently limit or give up early.
        if iteration > 3:
            audit_result = _run_progress_audit(tool_call_log)
            if audit_result.startswith("STUCK:"):
                stuck_reason = audit_result[6:].strip()
                _log(f"[AUDITOR] ⚠️ Loop/stagnation detected: {stuck_reason}")
                auditor_msg_id = f"dev-auditor-{iteration}-{uuid.uuid4()}"
                messages.append(HumanMessage(
                    content=(
                        f"[AUDITOR ALERT] You appear to be stuck or stagnating: {stuck_reason}. "
                        "STOP speculating. REPRODUCE the exact failure by RUNNING the code "
                        "(node/npm test/python, or start the server and hit the endpoint) and READ "
                        "the real output/traceback. Then fix THAT specific failing check. Do not "
                        "generate new hypotheses about other edge cases, do not re-read unchanged "
                        "files (re-read only the traceback or the lines you are about to edit), do "
                        "not run git commands in a non-git directory. Run the failing "
                        "test, read the traceback, fix the root cause, re-run a few times — if it "
                        "still fails, report BLOCKED with the diagnosis."
                    ),
                    id=auditor_msg_id
                ))
                appended_ids.append(auditor_msg_id)

        # Gentle progress nudges at very high iteration counts.
        # The LLM decides when the task is done — these are just soft reminders
        # that it's been working a long time. No pressure, no panic.
        run_messages = list(messages)
        if iteration >= 950:
            run_messages.append(HumanMessage(
                content=f"[GENTLE NUDGE] You've been working for {iteration} turns. "
                        "If the task is complete, produce your exit summary. "
                        "If there's genuinely more to do, continue — you're not out of time."
            ))

        # â”€â”€ Pillar 69: Cache disabled â”€â”€
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
                    where_tag = f"Iter {iteration}/{max_iters}: read {fname} {last_detail}".strip()
                elif last_tool == "write_file" and fname:
                    where_tag = f"Iter {iteration}/{max_iters}: write {fname}"
                elif last_tool == "edit_file" and fname:
                    where_tag = f"Iter {iteration}/{max_iters}: edit {fname} {last_detail}".strip()
                elif last_tool == "run_command":
                    cmd = str(last_args.get("command", ""))[:50]
                    where_tag = f"Iter {iteration}/{max_iters}: run {cmd}"
                elif last_tool == "search_code":
                    pat = str(last_args.get("pattern", ""))[:40]
                    where_tag = f"Iter {iteration}/{max_iters}: search \"{pat}\""
                elif last_tool and fname:
                    where_tag = f"Iter {iteration}/{max_iters}: {last_tool} {fname}"
                elif last_tool:
                    where_tag = f"Iter {iteration}/{max_iters}: {last_tool}"
                elif iteration == 1:
                    where_tag = f"Iter 1/{max_iters}: {client_req[:80].strip()}"
                else:
                    where_tag = f"Iter {iteration}/{max_iters}"

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
                # Knobs: BSA_REASONING_ITER1 (planning depth, "high"),
                #        BSA_REASONING_ROUTINE (routine depth, "disabled"),
                #        BSA_FLIP_ON_TEST_ONLY=1 (default) → flip to high only
                #        on real TEST failures; tool errors don't flip. With
                #        Dev self-verification off there are no mid-session
                #        test failures → the session is config-stable
                #        (iter-1 high, everything else disabled, zero flips).
                iter1_effort = os.environ.get("BSA_REASONING_ITER1", "high")
                routine_effort = os.environ.get("BSA_REASONING_ROUTINE", "disabled")
                flip_on_test_only = os.environ.get("BSA_FLIP_ON_TEST_ONLY", "1") == "1"
                think_hard = (
                    iteration <= 1
                    or reflexion_retries > 0
                    or last_test_failed
                    or (not flip_on_test_only and last_tool_failed)
                )
                if think_hard:
                    reasoning_effort = iter1_effort
                    reasoning_budget = None  # planning/diagnosis: full effort, no cap
                else:
                    reasoning_effort = routine_effort
                    reasoning_budget = None

                llm_result = invoke_messages_with_fallback(
                    role="Developer",
                    messages=run_messages,
                    tools=get_native_tools(),
                    where=where_tag,
                    reasoning_effort=reasoning_effort,
                    reasoning_budget=reasoning_budget,
                )

                # Unpack new (text, tool_calls, reasoning) tuple or legacy string
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

                # â”€â”€ Store successful code generation in cache â”€â”€
                if iteration == 1:
                    try:
                        set_cached_code(task, response_text)
                    except Exception:
                        pass
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
            appended_ids.extend([ai_msg_id, human_msg_id])
            # Allow a few retries for empty responses
            if iteration >= max_iters - 1:
                break
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

        # Extract all tool calls (mix native and text parsed)
        tool_calls = []
        if native_tool_calls:
            for tc in native_tool_calls:
                tname = tc.get("tool") or tc.get("name")
                targs = tc.get("args") or {}
                if isinstance(targs, str):
                    try:
                        targs = json.loads(targs)
                    except Exception:
                        pass
                if tname:
                    tool_calls.append({"tool": tname, "args": targs})
        else:
            tool_calls = parse_all_tool_calls(response_text)

        if tool_calls:
            malformed_retries = 0  # a real tool call parsed — reset the malformed counter

        if not tool_calls:
            # Response looks like it tried to call a tool but nothing parsed → nudge the model
            # to fix its format instead of silently treating the turn as "done" (premature finish).
            if looks_like_tool_call(response_text) and malformed_retries < 3:
                malformed_retries += 1
                _log(f"[DEVELOPER] âš ï¸ Response looks like a malformed tool call ({malformed_retries}/3) — nudging to fix format.")
                ai_msg_id = f"dev-malformed-{iteration}-{uuid.uuid4()}"
                human_msg_id = f"dev-human-malformed-{iteration}-{uuid.uuid4()}"
                messages.append(AIMessage(content=response_text, id=ai_msg_id))
                messages.append(HumanMessage(
                    content=(
                        "Your previous message looked like it was trying to call a tool, but the tool "
                        "call did not parse. Output ONLY a valid tool call in this exact format:\n"
                        '```tool\n{"tool": "tool_name", "args": {"param": "value"}}\n```\n'
                        "Ensure the JSON is valid (no trailing commas, balanced braces and quotes). "
                        "If you are actually done, reply in plain text with no ```tool block."
                    ),
                    id=human_msg_id,
                ))
                appended_ids.extend([ai_msg_id, human_msg_id])
                continue

            # No tool call â€” agent is done
            _log(f"[DEVELOPER] No tool call parsed from response (len={len(response_text)}). Preview: {repr(response_text[:300])}")
            planning_file = os.path.join(project_path or (os.environ.get("BSA_HARNESS_ROOT") or os.path.dirname(os.path.abspath(__file__))), "planning.md")
            has_written_plan = (
                any(item.get("tool") == "write_file" and "planning.md" in str(item.get("args", {}).get("file_path", "")) for item in tool_call_log) or
                (os.path.isfile(planning_file) and os.path.getsize(planning_file) > 10)
            )
            # Note: is_plan_req is always False (no mode detection). Plan nudging removed.

            # â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
            # reflexion_retries is a local variable
            # BSA_DEV_SELF_VERIFY=1 restores the old Dev self-verification
            # (deterministic cascade + test run + one bounded fix round).
            # Default OFF: Dev is implement-only — the harness verifies
            # automatically after the session and SDET fixes failures.
            dev_self_verify = os.environ.get("BSA_DEV_SELF_VERIFY", "0") == "1"
            if dev_self_verify and (tracked_files_created or tracked_files_modified):
                # â”€â”€ Stage 2: Fast Deterministic Cascade â”€â”€
                all_tracked = list(set(tracked_files_created + tracked_files_modified))
                _log(f"[REFLEXION] Stage 2: Running deterministic cascade on {len(all_tracked)} file(s)...")

                try:
                    stage2_result = run_deterministic_cascade(all_tracked, project_path)
                    _log(f"[REFLEXION] Stage 2: {stage2_result.summary}")

                    # If deterministic auto-fixes were applied, inject them as tool results
                    if stage2_result.auto_fixes_applied:
                        auto_fix_msg = "Auto-fixes applied:\n" + "\n".join(
                            f"  âœ“ {f}" for f in stage2_result.auto_fixes_applied
                        )
                        _log(f"[REFLEXION] {auto_fix_msg}")

                except Exception as e:
                    stage2_result = None
                    _log(f"[REFLEXION] Stage 2: deterministic_checker failed/unavailable: {e}")

                # â”€â”€ Run tests â”€â”€
                test_cmd = _detect_test_command(project_path)
                if test_cmd and iteration < max_iters - 3:  # Reserve 3 turns for reflexion fix loop
                    _log(f"[REFLEXION] Running tests: '{test_cmd}'...")
                    try:
                        test_result = execute_tool("run_command", {"command": test_cmd, "timeout": 30000})
                    except Exception as e:
                        test_result = f"Test execution error: {e}"

                    test_failed = (
                        "STATUS: FAIL" in str(test_result)
                        or (
                            "exit code:" in str(test_result).lower()
                            and "exit code: 0" not in str(test_result).lower()
                        )
                    )

                    if not test_failed:
                        # ── Test PASSED ──
                        _log("[REFLEXION] ✅ Tests PASSED")
                        reflexion_retries = 0  # Reset for next cycle

                    else:
                        # ── Test FAILED → honest report (NO in-session SDET) ──
                        # The Developer NEVER spawns SDET — that is the harness's
                        # job. One bounded re-entry lets Dev fix its own code or
                        # write an honest report; the harness verify gate (hidden
                        # test in eval, adaptive verify in the app) is the
                        # authority and fires SDET-FIX outside this session.
                        reflexion_retries += 1
                        _log(f"[REFLEXION] ❌ Tests FAILED (round {reflexion_retries}) — no SDET spawn; "
                             f"reporting honestly")
                        if reflexion_retries <= 1 and iteration < max_iters - 3:
                            test_tail = str(test_result)[-1500:] if test_result else ""
                            ai_msg_id = f"dev-ai-{iteration}-{uuid.uuid4()}"
                            human_msg_id = f"dev-verify-{iteration}-{uuid.uuid4()}"
                            additional_kwargs = {}
                            if reasoning_content and tool_calls:
                                additional_kwargs["reasoning_content"] = reasoning_content
                            messages.append(AIMessage(
                                content=response_text, id=ai_msg_id,
                                additional_kwargs=additional_kwargs
                            ))
                            messages.append(HumanMessage(
                                content=(
                                    "[VERIFY] Your verification run failed:\n"
                                    f"```\n{test_tail}\n```\n"
                                    "Do NOT claim success. Either fix the root cause now with "
                                    "minimal edits, or finish with an honest report quoting this "
                                    "failure in your Verification section. This is your ONE chance — "
                                    "the harness verifies independently after you finish."
                                ),
                                id=human_msg_id
                            ))
                            appended_ids.extend([ai_msg_id, human_msg_id])
                            continue  # one bounded re-entry, then honest exit
                        _log("[REFLEXION] Exiting with honest failure report — harness gate takes over fixing.")
                else:
                    _log("[REFLEXION] âš ï¸ No test command detected â€” skipping verification")

            clean_response = _extract_text_response(response_text)
            _log(f"\n[DEVELOPER] âœ… Agent finished after {iteration} iterations")
            _log(f"ðŸ¤– Assistant: {clean_response}")

            is_error = clean_response.strip().upper().startswith("ERROR:") or clean_response.strip().upper().startswith("FAILED:")

            # â”€â”€ Premature-exit guard â”€â”€
            # A FINAL message like "Now let me run the test suite to verify" means the
            # agent intends to verify but is EXITING instead — nothing runs after the
            # final message. The orchestrator then sees an unverified report and
            # re-delegates (the #1 source of extra Dev sessions). Catch intent-to-verify
            # finals and nudge once so the verification actually happens.
            _nudge_count = sum(1 for m in messages
                               if hasattr(m, "content") and isinstance(m.content, str)
                               and "You described what you would do" in m.content)
            _verify_nudge_count = sum(1 for m in messages
                                      if hasattr(m, "content") and isinstance(m.content, str)
                                      and "Your final message says you are about to run or verify" in m.content)
            _verify_intent = re.search(
                r"\b(?:i'?ll|i will|let me|going to|time to)\b[^.]{0,140}?"
                r"\b(?:verify|test|run|check|validate|reproduce|smoke)\b",
                clean_response[:400], re.IGNORECASE,
            )
            # Skip the nudge when the report already contains evidence the verification RAN
            # (past-tense "I ran it and it passed" is a completion, not a promise).
            _has_run_evidence = any(
                k in clean_response.lower()
                for k in ("passed", "exit code", "exit_code", "output:", "succeeded",
                          "completed", "ran ", "result:", "error:", "traceback")
            )
            if (_verify_intent and not _has_run_evidence and not is_error
                    and _nudge_count < 2 and _verify_nudge_count < 2 and iteration < max_iters - 3):
                _log("[DEVELOPER] âš ï¸ Premature exit: final message promises verification that never ran â€” nudging.")
                _vai_id = f"dev-ai-{iteration}-{uuid.uuid4()}"
                _vhum_id = f"dev-human-{iteration}-{uuid.uuid4()}"
                _vakw = {"reasoning_content": reasoning_content} if reasoning_content else {}
                messages.append(AIMessage(content=response_text, id=_vai_id, additional_kwargs=_vakw))
                messages.append(HumanMessage(
                    content=(
                        "[NUDGE] Your final message says you are about to run or verify "
                        "something, but this is the LAST message â€” nothing executed after "
                        "it. If the work is done, RUN the verification NOW with run_command "
                        "(the tests, or a manual check: run the app/script and probe it, or "
                        "write a quick self-check script and execute it). Then finish with a "
                        "complete summary that includes the command's ACTUAL output. Never "
                        "describe a verification you did not run."
                    ),
                    id=_vhum_id,
                ))
                appended_ids.extend([_vai_id, _vhum_id])
                continue

            # â”€â”€ Post-Execution Verification: detect "described instead of did" â”€â”€
            # If the agent made zero tool calls and zero files were created, but the
            # response reads like a plan/intention (not an error), the agent hallucinated
            # that it did work without actually calling any tools.
            no_tools_called = len(tool_call_log) == 0
            no_files_created = len(tracked_files_created) == 0 and len(tracked_files_modified) == 0
            # Only trigger on genuine INTENT markers ("I'll / Let me / going to" + will-do verb).
            # "## Plan" headings, code fences, and the word "greenfield" are legitimate content
            # of analysis-only or plan-only answers — they are NOT evidence of describing instead of doing.
            _described_intent = re.search(
                r"\b(?:i'?ll|i will|let me|let's|going to|time to|about to)\b[^.]{0,120}?"
                r"\b(?:implement|write|create|build|add|fix|refactor|update|delete|make|call)\b",
                clean_response[:400], re.IGNORECASE,
            )
            looks_like_description = not is_error and (
                no_tools_called and no_files_created and bool(_described_intent)
            )
            # Count how many times we've already nudged (max 2)
            _nudge_count = sum(1 for msg in messages if hasattr(msg, "content") and isinstance(msg.content, str) and "You described what you would do but did NOT actually call any tools" in msg.content)
            if no_tools_called and no_files_created and looks_like_description and _nudge_count < 2:
                _log(f"[DEVELOPER] âŒ AGENT DESCRIBED INSTEAD OF DOING (nudge {_nudge_count + 1}/2) â€” injecting tool-use nudge and continuing loop")
                ai_msg_id = f"dev-ai-{iteration}-{uuid.uuid4()}"
                human_msg_id = f"dev-human-{iteration}-{uuid.uuid4()}"
                additional_kwargs = {}
                if reasoning_content and tool_calls:
                    additional_kwargs["reasoning_content"] = reasoning_content
                messages.append(AIMessage(content=response_text, id=ai_msg_id, additional_kwargs=additional_kwargs))
                messages.append(HumanMessage(
                    content=(
                        "You described what you would do but did NOT actually call any tools. "
                        "Zero files were created. You MUST use tools to make progress.\n\n"
                        "Call tools using ONE of these formats:\n\n"
                        "Format A:\n```tool\n"
                        '{"tool": "write_file", "args": {"file_path": "output.txt", "content": "..."}}\n'
                        "```\n\n"
                        "Format B:\n<tool_call name=\"write_file\">\n"
                        '{"file_path": "output.txt", "content": "..."}\n'
                        "</tool_call>\n\n"
                        "DO NOT just describe what you would do. Actually call the tools NOW. "
                        "Write the files. Run the commands. Make it happen."
                    ),
                    id=human_msg_id
                ))
                appended_ids.extend([ai_msg_id, human_msg_id])
                continue  # Re-enter the loop with the nudge
            elif no_tools_called and no_files_created and looks_like_description:
                _log("[DEVELOPER] âŒ Agent persistently described instead of doing â€” aborting with error")
                is_error = True
                clean_response = "ERROR: Agent described the solution instead of executing. No tools were called."

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
                files_created = [t for t in tool_call_log if t.get("tool") in ("write_file", "edit_file")]
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

            # â”€â”€ Update task.json: mark current step completed/failed, advance â”€â”€
            try:
                task_tracking_data = load_task_tracking(project_path, chat_id)
                if task_tracking_data and task_tracking_data.get("steps"):
                    steps_data = task_tracking_data["steps"]
                    current_idx = task_tracking_data.get("current_step", 0)
                    if current_idx < len(steps_data):
                        steps_data[current_idx]["tool_calls"] = step_tool_calls
                        if clean_response:
                            steps_data[current_idx]["notes"] = clean_response[:200]
                            
                        if is_error:
                            steps_data[current_idx]["status"] = "failed"
                        else:
                            steps_data[current_idx]["status"] = "completed"
                            steps_data[current_idx]["completed_at"] = datetime.now().isoformat()
                            # Step-level context isolation: clear developer_state on step completion
                            if "developer_state" in task_tracking_data:
                                save_task_tracking({k:v for k,v in task_tracking_data.items() if k != "developer_state"}, project_path, chat_id)
                                del task_tracking_data["developer_state"]
                            
                        # Save artifacts
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
                            next_step = current_idx + 1
                            if next_step < len(steps_data):
                                task_tracking_data["current_step"] = next_step
                                steps_data[next_step]["status"] = "in_progress"
                            else:
                                task_tracking_data["status"] = "completed"
                        save_task_tracking(task_tracking_data, project_path, chat_id)
            except Exception as e:
                print(f"[DEVELOPER] Error updating task tracking: {e}")
            # â”€â”€ END â”€â”€

            # â”€â”€ Pillar 63/75/96: Local code quality check on all tracked files â”€â”€
            lint_summary_parts: list[str] = []
            if not is_error:
                all_tracked = list(set(tracked_files_created + tracked_files_modified))
                py_files = [f for f in all_tracked if f.endswith(".py") and os.path.isfile(f)]
                if py_files:
                    try:
                        from dev_lint import lint_and_fix
                        for py_file in py_files:
                            try:
                                with open(py_file, "r", encoding="utf-8") as lf:
                                    original = lf.read()
                                lint_result = lint_and_fix(original, py_file)
                                total_fixes = len(lint_result["fast_fixes"]) + len(lint_result["lint_fixes"])
                                if total_fixes > 0:
                                    with open(py_file, "w", encoding="utf-8") as lf:
                                        lf.write(lint_result["code"])
                                    _log(f"[DEVELOPER] ðŸ”§ dev_lint fixed {total_fixes} issue(s) in {os.path.basename(py_file)}: "
                                         f"{'; '.join(lint_result['fast_fixes'] + lint_result['lint_fixes'])}")
                                    lint_summary_parts.append(f"{py_file}: {total_fixes} fix(es)")
                                elif not lint_result["original_valid"] and lint_result["final_valid"]:
                                    _log(f"[DEVELOPER] âœ… dev_lint resolved syntax error in {os.path.basename(py_file)}")
                                    with open(py_file, "w", encoding="utf-8") as lf:
                                        lf.write(lint_result["code"])
                                    lint_summary_parts.append(f"{py_file}: syntax fixed")
                            except Exception as le:
                                _log(f"[DEVELOPER] âš ï¸ dev_lint skipped {os.path.basename(py_file)}: {le}")
                        if lint_summary_parts:
                            _log(f"[DEVELOPER] ðŸŽ¯ Local lint saved LLM correction turns on {len(lint_summary_parts)} file(s)")
                    except ImportError:
                        _log("[DEVELOPER] dev_lint module not available, skipping local code quality checks")

            if is_error:
                return make_return({
                    "code": f"// ERROR: {clean_response[:1000]}",
                    "agent_report": _build_dev_report(clean_response, True, []),
                    "test_report": f"STATUS: FAIL\nDeveloper reported error: {clean_response[:300]}",
                    "project_path": project_path,
                    "code_updated": True,
                    "tech_spec_updated": False,
                })
            # Build exit report with validation -- bare "Completed." is useless
            def _build_dev_report(raw: str, is_err: bool, lint_parts: list) -> str:
                import re as _re
                files_touched = sorted(set(
                    t.get("args", {}).get("file_path", "")
                    for t in tool_call_log
                    if t.get("tool") in ("write_file", "edit_file")
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
                    if lint_parts:
                        out.append(f"- Lint fixes: {', '.join(str(s)[:120] for s in lint_parts[:3])}")
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
                    "agent_report": _build_dev_report(clean_response, True, []),
                    "test_report": f"STATUS: FAIL\nDeveloper reported error: {clean_response[:300]}",
                    "project_path": project_path,
                    "code_updated": True,
                    "tech_spec_updated": False,
                })
            else:
                return make_return({
                    "code": "Code successfully implemented.",
                    "agent_report": _build_dev_report(clean_response, False, lint_summary_parts),
                    "test_report": "",
                    "project_path": project_path,
                    "code_updated": True,
                    "tech_spec_updated": False,
                })

        tool_outputs = []
        this_turn_has_write = False
        any_early_stop = False
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

            if tool_name not in valid_tools:
                _log(f"[DEVELOPER] Unknown tool requested: {tool_name} â€” asking LLM to retry")
                tool_result = f"TOOL ERROR: Unknown tool: {tool_name}. Available tools: {', '.join(valid_tools)}"
                tool_outputs.append(f"[{tool_name}]:\n{tool_result}")
                continue

            # Track write/edit/run for progress detection
            if tool_name in ("write_file", "edit_file", "apply_diff", "run_command"):
                this_turn_has_write = True

            # â”€â”€ LoopGuard Pre-Execution Check â”€â”€
            guard_result = LoopGuard.check_pre_execute(tool_call_log, tool_name, tool_args)
            warning_msg = None
            if guard_result:
                guard_type, guard_msg = guard_result
                if guard_type == "STALE":
                    tool_result = f"[STALE] {guard_msg}"
                    tool_call_log.append({
                        "iteration": iteration,
                        "tool": tool_name,
                        "args": tool_args,
                        "result_preview": "[STALE] skipped",
                    })
                    tool_outputs.append(f"[{tool_name}]:\n{tool_result}")
                    continue
                elif guard_type == "ABORT":
                    _log(f"[DEVELOPER] âš ï¸ Loop detected for tool {tool_name} â€” aborting loop.")
                    clean_response = f"ERROR: {guard_msg}"
                    safe_update_state({
                        "thoughts": {"developer": "Execution failed due to blocking loop."},
                        "outputs": {
                            "code": f"// ERROR: {clean_response[:1000]}",
                            "agent_report": clean_response
                        }
                    })
                    try:
                        safe_update_state({"developer_tool_log": json.dumps(tool_call_log, indent=2)})
                    except Exception:
                        safe_update_state({"developer_tool_log": str(tool_call_log)})
                    try:
                        task_tracking_data = load_task_tracking(project_path, chat_id)
                        if task_tracking_data and task_tracking_data.get("steps"):
                            steps_data = task_tracking_data["steps"]
                            current_idx = task_tracking_data.get("current_step", 0)
                            if current_idx < len(steps_data):
                                steps_data[current_idx]["status"] = "failed"
                                steps_data[current_idx]["notes"] = clean_response[:200]
                                steps_data[current_idx]["tool_calls"] = step_tool_calls
                                save_task_tracking(task_tracking_data, project_path, chat_id)
                    except Exception as e:
                        print(f"[DEVELOPER] Error saving task progress: {e}")
                    return make_return({
                        "code": f"// ERROR: {clean_response[:1000]}",
                        "agent_report": clean_response,
                        "test_report": f"STATUS: FAIL\nDeveloper stuck in loop: {clean_response[:300]}",
                        "project_path": project_path,
                        "code_updated": True,
                        "tech_spec_updated": False,
                    })

            # â”€â”€ Same-Count warning/abort check â”€â”€
            same_count = count_identical_calls_since_state_change(tool_call_log, tool_name, tool_args)
            if same_count >= 2:
                is_read_tool = tool_name in ("read_file", "search_code", "list_files", "search_codebase", "view_signatures")
                if same_count >= 5:
                    _log(f"[DEVELOPER] âš ï¸ Loop detected for tool {tool_name} (called {same_count} times) â€” aborting loop.")
                    clean_response = f"ERROR: Loop detected. Called tool {tool_name} with same arguments {same_count} times without state change."
                    safe_update_state({
                        "thoughts": {"developer": "Execution failed due to blocking loop."},
                        "outputs": {
                            "code": f"// ERROR: {clean_response[:1000]}",
                            "agent_report": clean_response
                        }
                    })
                    try:
                        safe_update_state({"developer_tool_log": json.dumps(tool_call_log, indent=2)})
                    except Exception:
                        safe_update_state({"developer_tool_log": str(tool_call_log)})
                    try:
                        task_tracking_data = load_task_tracking(project_path, chat_id)
                        if task_tracking_data and task_tracking_data.get("steps"):
                            steps_data = task_tracking_data["steps"]
                            current_idx = task_tracking_data.get("current_step", 0)
                            if current_idx < len(steps_data):
                                steps_data[current_idx]["status"] = "failed"
                                steps_data[current_idx]["notes"] = clean_response[:200]
                                steps_data[current_idx]["tool_calls"] = step_tool_calls
                                save_task_tracking(task_tracking_data, project_path, chat_id)
                    except Exception as e:
                        print(f"[DEVELOPER] Error saving task progress: {e}")
                    return make_return({
                        "code": f"// ERROR: {clean_response[:1000]}",
                        "agent_report": clean_response,
                        "test_report": f"STATUS: FAIL\nDeveloper stuck in loop: {clean_response[:300]}",
                        "project_path": project_path,
                        "code_updated": True,
                        "tech_spec_updated": False,
                    })
                else:
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

            # â”€â”€ Stale-Read Detection â”€â”€
            # If the agent is re-reading the same file+offset a 3rd+ time,
            # return [STALE] to break re-read spirals (saves tokens + loops).
            # The count resets if the file was successfully modified in between.
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

                if same_reads >= 2:  # This is the 3rd+ time
                    tool_result = (
                        f"[STALE] This file was already read {same_reads + 1} times with the same parameters. "
                        f"Content has NOT changed. Do NOT re-read this file â€” use the previous output. "
                        f"If you need different content, change offset/limit or read a different file."
                    )
                    tool_call_log.append({
                        "iteration": iteration,
                        "tool": tool_name,
                        "args": tool_args,
                        "result_preview": "[STALE] skipped",
                    })
                    tool_outputs.append(f"[{tool_name}]:\n{tool_result}")
                    continue

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
            elif tool_name == "write_file":
                content_len = len(str(tool_args.get("content", "")))
                tool_label = f"write {fname or fpath} ({content_len}B)"
            elif tool_name == "edit_file":
                tool_label = f"edit {fname or fpath}"
            elif tool_name == "run_command":
                cmd = str(tool_args.get("command", ""))[:70]
                tool_label = f"run {cmd}"
            elif tool_name == "search_code":
                pat = str(tool_args.get("pattern", ""))[:60]
                tool_label = f"search \"{pat}\""
            elif tool_name == "list_files":
                lp = str(tool_args.get("path", "."))[:50]
                tool_label = f"ls {lp}"
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
                tool_result = execute_tool(tool_name, tool_args)
                tool_result_str = str(tool_result)
                if len(tool_result_str) > 8000:
                    # 30/70 head/tail split: diagnostics (tracebacks, exit
                    # codes, test summaries) live at the TAIL — keep more of
                    # it than the head.
                    tool_result_str = tool_result_str[:2400] + "\n... (truncated middle) ...\n" + tool_result_str[-5600:]
                _log(f"[TOOL OUTPUT] {tool_name}: {tool_result_str}")
            except Exception as e:
                tool_result = f"Tool execution error: {e}"
                _log(f"[TOOL OUTPUT] {tool_name}: {tool_result}")

            # ── Failure tracking for the thinking ladder ──
            # last_tool_failed / last_test_failed are consumed by the NEXT turn's
            # reasoning-effort selection (think_hard) so a failed action forces a
            # high-effort diagnosis turn.
            res_str = str(tool_result)
            if tool_name == "run_command":
                # run_command success marker is "[OK] Exit 0" (tools.py) — anything
                # else is a failure (non-zero exit). Test/verify failures additionally
                # flip last_test_failed (e.g. pytest/npm test/verify scripts).
                if "[OK] Exit 0" not in res_str:
                    last_tool_failed = True
                cmd_low = str(tool_args.get("command", "")).lower()
                if any(k in cmd_low for k in ("test", "verify", "pytest", "jest", "check", "lint")) and "[OK] Exit 0" not in res_str:
                    last_test_failed = True
            elif tool_name == "edit_file":
                if res_str.startswith("Error") or "no match" in res_str.lower():
                    last_tool_failed = True
            elif tool_name in ("read_file", "search_code", "list_files", "view_signatures", "search_codebase"):
                if res_str.startswith("Error"):
                    last_tool_failed = True
            elif res_str.startswith("Tool execution error") or res_str.startswith("Error:"):
                last_tool_failed = True

            # â”€â”€ Build detail string for the where_tag (shown in next LLM invocation) â”€â”€
            tool_detail = ""

            # Enrich read_file with actual line range from result
            if tool_name == "read_file" and not str(tool_result).startswith("[STALE]"):
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
                "result_preview": str(tool_result)[:200],
                "detail": tool_detail,  # for where_tag enrichment
            })
            # Remember the last executed tool — the NEXT turn's reasoning ladder
            # uses it to detect the discovery phase (search/list/read → low+cap).
            last_tool = tool_name

            # Check for Sleep-and-Resume suspension trigger
            if tool_name == "run_command" and tool_args.get("background") is True and "[OK] Started background process" in str(tool_result):
                if os.environ.get("DEEP_AGENTS_EVAL_RUN") == "1":
                    _log(f"[DEVELOPER] ðŸ’¤ Background task started in evaluation mode. Sleeping 5 seconds for boot instead of suspending...")
                    time.sleep(5)
                else:
                    _log(f"[DEVELOPER] ðŸ’¤ Background task started. Suspending graph and starting automatic wake-up thread...")
                    
                    # Extract process name
                    process_name = tool_args.get("command", "").split()[0]
                    
                    def _wakeup_trigger():
                        # Sleep 15 seconds to let the server boot up
                        time.sleep(15)
                        try:
                            # Call /api/run to resume
                            url = "http://127.0.0.1:8000/api/run"
                            payload = {
                                "prompt": "continue",
                                "workspace_path": project_path,
                                "chat_id": chat_id
                            }
                            requests.post(url, json=payload, timeout=5)
                            print("[WAKEUP] Wake-up request sent to API server.")
                        except Exception as ex:
                            print(f"[WAKEUP] Error sending wake-up request: {ex}")
                            
                    threading.Thread(target=_wakeup_trigger, daemon=True).start()
                    
                    # Save developer state to task.json
                    try:
                        task_data = load_task_tracking(project_path, chat_id)
                        if task_data:
                            serialized_msgs = serialize_messages(messages)
                            
                            task_data["developer_state"] = {
                                "messages": serialized_msgs,
                                "iteration": iteration,
                                "tool_call_log": tool_call_log,
                                "step_tool_calls": step_tool_calls,
                                "tracked_files_created": tracked_files_created,
                                "tracked_files_modified": tracked_files_modified,
                                "last_response_text": last_response_text,
                                "consecutive_read_turns": consecutive_read_turns,
                                "voluntary_compact_requested": voluntary_compact_requested,
                            }
                            save_task_tracking(task_data, project_path, chat_id)
                    except Exception as e:
                        print(f"[DEVELOPER] Error saving developer state: {e}")
                    
                    return make_return({
                        "code": f"// SUSPENDED: Waiting for background process '{process_name}'...",
                        "agent_report": f"SUSPENDED: Waiting for background process '{process_name}' to boot. Will resume automatically.",
                        "test_report": "",
                        "project_path": project_path,
                        "code_updated": False,
                        "tech_spec_updated": False,
                        "next_agent": "suspended",
                    })

            if warning_msg:
                tool_result = str(tool_result) + warning_msg

            if tool_name == "compact_conversation":
                voluntary_compact_requested = True

            if warning_msg:
                tool_result = str(tool_result) + warning_msg

            # Track artifacts for task.json
            step_tool_calls += 1
            if tool_name in ("write_file",):
                fpath = tool_args.get("file_path", "")
                if fpath and fpath not in tracked_files_created:
                    tracked_files_created.append(fpath)
            elif tool_name == "edit_file":
                fpath = tool_args.get("file_path", "")
                if fpath and fpath not in tracked_files_modified:
                    tracked_files_modified.append(fpath)

            # Format output for this tool
            tool_outputs.append(f"[{tool_name}]:\n{tool_result}")

            # â”€â”€ Early-stop on verified test success â”€â”€
            if tool_name == "run_command" and "[OK] Exit 0" in str(tool_result):
                result_lower = str(tool_result).lower()
                if "passed" in result_lower or "ok" in result_lower:
                    if any(kw in result_lower for kw in ["test", "pytest", "unittest", "assert"]):
                        if tracked_files_created or tracked_files_modified:
                            any_early_stop = True

        # If early stop was triggered during execution of the tools
        if any_early_stop:
            try:
                task_tracking_data = load_task_tracking(project_path, chat_id)
                if task_tracking_data and task_tracking_data.get("steps"):
                    steps_data = task_tracking_data["steps"]
                    current_idx = task_tracking_data.get("current_step", 0)
                    if current_idx < len(steps_data):
                        steps_data[current_idx]["notes"] = "Tests verified passing."
                        steps_data[current_idx]["status"] = "completed"
                        steps_data[current_idx]["completed_at"] = datetime.now().isoformat()
                        steps_data[current_idx]["tool_calls"] = step_tool_calls
                        
                        # Step-level context isolation: clear developer_state on step completion
                        if "developer_state" in task_tracking_data:
                            save_task_tracking({k:v for k,v in task_tracking_data.items() if k != "developer_state"}, project_path, chat_id)
                            del task_tracking_data["developer_state"]

                        artifacts = task_tracking_data.setdefault("artifacts", {})
                        if tracked_files_created:
                            existing = set(artifacts.get("files_created", []))
                            existing.update(tracked_files_created)
                            artifacts["files_created"] = list(existing)
                        if tracked_files_modified:
                            existing = set(artifacts.get("files_modified", []))
                            existing.update(tracked_files_modified)
                            artifacts["files_modified"] = list(existing)
                            
                        next_step = current_idx + 1
                        if next_step < len(steps_data):
                            task_tracking_data["current_step"] = next_step
                            steps_data[next_step]["status"] = "in_progress"
                        else:
                            task_tracking_data["status"] = "completed"
                        save_task_tracking(task_tracking_data, project_path, chat_id)
            except Exception as e:
                print(f"[DEVELOPER] Error saving task progress: {e}")

            agent_report_text = "Implementation complete. Tests passed."
            return make_return({
                "code": "Code successfully implemented and tests verified.",
                "agent_report": agent_report_text,
                "test_report": "",
                "project_path": project_path,
                "code_updated": True,
                "tech_spec_updated": False,
            })

        combined_tool_result = "\n\n".join(tool_outputs)

        # â”€â”€ Progress Gate: prevent read-only death spirals â”€â”€
        is_read_only_turn = not this_turn_has_write
        _consecutive_read_turns = consecutive_read_turns
        if is_read_only_turn:
            _consecutive_read_turns += 1
        else:
            _consecutive_read_turns = 0
        consecutive_read_turns = _consecutive_read_turns

        if _consecutive_read_turns == 6:
            combined_tool_result += (
                "\n\n[PROGRESS GATE] You have spent 6 turns investigating without making any changes. "
                "You MUST now either: (a) make a targeted change — read ONLY the exact lines you will "
                "edit, then write_file/edit_file, or (b) output your completion report if the task is done. "
                "Do NOT keep scanning files or running broad searches."
            )
        elif _consecutive_read_turns >= 10:
            combined_tool_result += (
                "\n\n[HARD STOP] 10 consecutive investigation turns with zero changes. "
                "You are out of investigation budget. You MUST stop investigating. "
                "Either write your completion report NOW (no tools), or make a targeted edit immediately."
            )

        # â”€â”€ Cost Awareness â”€â”€
        cost_state = safe_get_state()
        tu = cost_state.get("token_usage", {})
        spent = tu.get("total_cost", 0)
        n_calls = len(tu.get("calls", []))
        # Throttle: inject the cost line at most every 25 turns (and on the first),
        # instead of repeating identical bytes every turn.
        if spent > 0 and (iteration % 25 == 0 or iteration == 1):
            cost_line = f"\n\n[COST: ${spent:.4f} spent across {n_calls} LLM calls. Keep efficiency in mind — but never skip the verification your rules require.]"
            combined_tool_result += cost_line

        # Truncate response_text after the first tool block to prevent hallucinations
        last_tool_idx = response_text.rfind("```")
        if last_tool_idx != -1:
            response_text = response_text[:last_tool_idx + 3]

        history_text = re.sub(r'<thinking>.*?</thinking>', '', response_text, flags=re.DOTALL).strip()
        ai_msg_id = f"dev-ai-{iteration}-{uuid.uuid4()}"
        human_msg_id = f"dev-human-{iteration}-{uuid.uuid4()}"
        additional_kwargs = {}
        # Only pass reasoning_content back when THIS turn made a tool call.
        # DeepSeek docs: if the model did NOT perform a tool call, the intermediate
        # reasoning_content does not need to participate in context concatenation
        # (and if passed it is IGNORED) — so skipping it on text-only turns is both
        # allowed and saves re-embedding those tokens as cache-miss input.
        if reasoning_content and tool_calls:
            additional_kwargs["reasoning_content"] = reasoning_content
        messages.append(AIMessage(content=history_text, id=ai_msg_id, additional_kwargs=additional_kwargs))
        messages.append(HumanMessage(content=combined_tool_result, id=human_msg_id))
        appended_ids.extend([ai_msg_id, human_msg_id])

        # Update shared state with progress
        last_tool_name = tool_calls[-1].get("tool", "") if tool_calls else ""
        if last_tool_name:
            safe_update_state({"outputs": {"code": f"// Developer iteration {iteration}: {last_tool_name} completed"}})

    # Max iterations reached
    _log(f"[DEVELOPER] âš ï¸ Max iterations ({max_iters}) reached")
    safe_update_state({"thoughts": {"developer": f"Reached maximum iterations ({max_iters})."}})

    # â”€â”€ Goal Toggle Auto-Continue System â”€â”€
    if _is_auto_continue_enabled():
        _log("[DEVELOPER] ðŸš€ Auto-Continue enabled. Triggering automatic wake-up and suspending graph...")

        try:
            task_data = load_task_tracking(project_path, chat_id)
            if task_data:
                serialized_msgs = serialize_messages(messages)
                
                # Make sure status is set to in_progress so the Supervisor Bypass triggers on wakeup
                task_data["status"] = "in_progress"
                task_data["developer_state"] = {
                    "messages": serialized_msgs,
                    "iteration": iteration,
                    "tool_call_log": tool_call_log,
                    "step_tool_calls": step_tool_calls,
                    "tracked_files_created": tracked_files_created,
                    "tracked_files_modified": tracked_files_modified,
                    "last_response_text": last_response_text,
                    "consecutive_read_turns": consecutive_read_turns,
                }
                save_task_tracking(task_data, project_path, chat_id)
        except Exception as e:
            print(f"[DEVELOPER] Error saving state: {e}")

        # Return suspended response
        return make_return({
            "code": f"// SUSPENDED: Iteration cap {max_iters} reached. Auto-continuing...",
            "agent_report": f"SUSPENDED: Iteration limit {max_iters} reached. Auto-continuing task execution...",
            "test_report": "",
            "project_path": project_path,
            "code_updated": False,
            "tech_spec_updated": False,
            "next_agent": "suspended",
        })

    summary = last_response_text[:5000] if last_response_text else f"Developer ran {iteration} tool calls."
    agent_report = (
        f"I have successfully completed {iteration} turns. If you would like me to resume and continue "
        f"this work for another {max_iters} turns, please type 'continue' or 'go on'.\n\n"
        f"Here is what I accomplished so far:\n{summary}"
    )

    try:
        tool_log_val = json.dumps(tool_call_log, indent=2)
    except Exception:
        tool_log_val = str(tool_call_log)
    safe_update_state({"developer_tool_log": tool_log_val})

    # â”€â”€ Save partial task progress â”€â”€
    try:
        task_tracking_data = load_task_tracking(project_path, chat_id)
        if task_tracking_data and task_tracking_data.get("steps"):
            steps_data = task_tracking_data["steps"]
            current_idx = task_tracking_data.get("current_step", 0)
            if current_idx < len(steps_data):
                steps_data[current_idx]["tool_calls"] = step_tool_calls
                steps_data[current_idx]["notes"] = f"Reached max iterations ({max_iters}) â€” will continue"
                # Save artifacts even on partial progress
                artifacts = task_tracking_data.setdefault("artifacts", {})
                if tracked_files_created:
                    existing = set(artifacts.get("files_created", []))
                    existing.update(tracked_files_created)
                    artifacts["files_created"] = list(existing)
                if tracked_files_modified:
                    existing = set(artifacts.get("files_modified", []))
                    existing.update(tracked_files_modified)
                    artifacts["files_modified"] = list(existing)
            save_task_tracking(task_tracking_data, project_path, chat_id)
    except Exception as e:
        print(f"[DEVELOPER] Error saving partial task progress: {e}")

    safe_update_state({
        "outputs": {
            "code": summary,
            "agent_report": agent_report
        }
    })

    return make_return({
        "code": summary,
        "agent_report": agent_report,
        "test_report": "",
        "project_path": project_path,
        "code_updated": True,
        "tech_spec_updated": False,
    })



# â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
# Parser & helper functions merged from dev_utils.py
# â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

"""Developer division utilities â€” shared helpers for developer subagents."""
import os
import json


SECTION_LABELS: dict[str, str] = {
    "db": "DB Schema", "api": "API Design", "layered": "Code Structure",
    "resilience": "Error Handling", "design_system": "Design Tokens",
    "sequence": "Sequence Flows", "ecosystem": "Dependencies",
}


def build_other_summary(sections: dict, exclude_keys: list[str]) -> str:
    """Builds a brief summary of SA spec sections NOT targeted to this coder.

    Gives cross-reference awareness without dumping irrelevant full sections.
    """
    parts: list[str] = []
    for key, label in SECTION_LABELS.items():
        if key not in exclude_keys and sections.get(key):
            brief = sections[key][:150].replace("\n", " ").strip()
            parts.append(f"- {label}: {brief}...")
    return "\n".join(parts) or "N/A"


def read_files_for_manifest(project_path: str, manifest_entries: list[dict]) -> str:
    """Reads ONLY files listed in the manifest â€” targeted context, not a token bomb."""
    parts: list[str] = []
    for entry in manifest_entries:
        fp = os.path.join(project_path, entry["path"])
        if not os.path.isfile(fp):
            continue
        try:
            with open(fp, "r", encoding="utf-8", errors="replace") as f:
                parts.append(f"File: {entry['path']}\n---\n{f.read()}\n---")
        except Exception:
            pass
    return "\n\n".join(parts)


def list_existing_files(project_path: str) -> list[str]:
    """Lists project source files, skipping non-source directories."""
    files: list[str] = []
    skip = {"venv", ".git", "__pycache__", "node_modules", ".next", "dist", "build"}
    for root, dirs, fnames in os.walk(project_path):
        dirs[:] = [d for d in dirs if d not in skip]
        for fname in fnames:
            files.append(os.path.relpath(os.path.join(root, fname), project_path))
    return files


def detect_build_goal(project_path: str) -> str:
    """Detects project type and returns the appropriate build-verification goal."""
    if os.path.exists(os.path.join(project_path, "package.json")):
        return "Install dependencies via npm install, then verify by running npm run build."
    elif os.path.exists(os.path.join(project_path, "go.mod")) or any(f.endswith(".go") for f in os.listdir(project_path) if os.path.isfile(os.path.join(project_path, f))):
        return "Verify code compilation by running go build."
    elif os.path.exists(os.path.join(project_path, "artisan")):
        return "Verify routes and syntax by running php artisan route:list."
    elif any(f.endswith(".php") for f in os.listdir(project_path) if os.path.isfile(os.path.join(project_path, f))):
        return "Syntax check PHP files using php -l."
    return "Compile check all source files using appropriate compiler/syntax tools."


def update_project_path_in_memory(project_path: str) -> None:
    """DEPRECATED — no longer persists to project folders. No-op."""
    pass


def scan_and_update_dependencies(project_path: str) -> None:
    """Scrapes import statements in project files and updates requirements.txt."""
    import ast

    STD_LIBS = {
        "os", "sys", "re", "json", "math", "time", "sqlite3", "random", "datetime",
        "subprocess", "threading", "hashlib", "hmac", "secrets", "urllib", "collections",
        "itertools", "functools", "typing", "ast", "shutil", "traceback", "abc",
        "unittest", "io", "csv", "xml", "logging", "pickle", "ctypes", "socket",
        "select", "argparse", "tempfile", "glob", "copy", "importlib", "uuid", "platform",
        "timeit", "ctypes", "distutils", "email", "html", "http", "socketserver", "xmlrpc"
    }

    IMPORT_TO_PKG = {
        "telebot": "pyTelegramBotAPI",
        "sklearn": "scikit-learn",
        "PIL": "pillow",
        "bs4": "beautifulsoup4",
        "yaml": "PyYAML",
        "google": "google-genai",
        "dotenv": "python-dotenv",
        "speech_recognition": "SpeechRecognition",
        "win32com": "pywin32",
        "pythoncom": "pywin32",
        "langchain_core": "langchain-core",
        "langchain_google_genai": "langchain-google-genai",
        "langchain_groq": "langchain-groq",
        "langchain_openai": "langchain-openai",
        "langgraph": "langgraph",
        "fastapi": "fastapi",
        "uvicorn": "uvicorn",
        "pandas": "pandas",
        "numpy": "numpy",
        "gradio": "gradio",
        "pydantic": "pydantic",
        "requests": "requests",
        "jinja2": "Jinja2"
    }

    local_modules = set()
    python_files = []

    # 1. Discover all python files in project path (excluding standard virtual environments)
    skip = {"venv", ".venv", ".git", "__pycache__", "node_modules", ".next", "dist", "build"}
    for root, dirs, fnames in os.walk(project_path):
        dirs[:] = [d for d in dirs if d not in skip]
        for fname in fnames:
            if fname.endswith(".py"):
                full_path = os.path.join(root, fname)
                python_files.append(full_path)
                # Base module name is the filename without .py
                local_modules.add(os.path.splitext(fname)[0])

    third_party_imports = set()

    # 2. Parse import syntax using Python AST
    for fp in python_files:
        try:
            with open(fp, "r", encoding="utf-8", errors="replace") as f:
                tree = ast.parse(f.read(), filename=fp)
                for node in ast.walk(tree):
                    if isinstance(node, ast.Import):
                        for name in node.names:
                            base = name.name.split(".")[0]
                            if base and base not in STD_LIBS and base not in local_modules:
                                third_party_imports.add(base)
                    elif isinstance(node, ast.ImportFrom):
                        if node.module:
                            base = node.module.split(".")[0]
                            if base and base not in STD_LIBS and base not in local_modules:
                                third_party_imports.add(base)
        except Exception:
            pass

    # 3. Map imports to pip packages
    mapped_packages = set()
    for imp in third_party_imports:
        pkg = IMPORT_TO_PKG.get(imp, imp)
        mapped_packages.add(pkg)

    if not mapped_packages:
        return

    # 4. Save to requirements.txt inside the project folder
    req_file_path = os.path.join(project_path, "requirements.txt")
    try:
        with open(req_file_path, "w", encoding="utf-8") as f:
            for pkg in sorted(mapped_packages):
                f.write(f"{pkg}\n")
        print(f"[Dependency Scanner] Successfully synced requirements.txt with: {sorted(mapped_packages)}")
    except Exception as e:
        print(f"[Dependency Scanner] Error writing requirements.txt: {e}")


def extract_traceback_files_context(project_path: str, test_report: str) -> str:
    """Parses a test report traceback to find failing files, and returns their content as context."""
    if not test_report:
        return ""
    import re
    injected_files_context = ""
    try:
        found_files = set(re.findall(r'\b([\w\-]+\.(?:py|js|ts|go))\b', test_report))
        if project_path and os.path.isdir(project_path):
            for root, _, filenames in os.walk(project_path):
                # Skip version control, cache and env directories
                if any(ignore in root for ignore in [".git", ".deep_agents", ".pytest_cache", "__pycache__", "venv", ".venv"]):
                    continue
                for f in filenames:
                    is_test_file = f.startswith("test_") or f.endswith("_test.py") or f.endswith(".test.js") or f.endswith(".spec.js") or f.endswith(".test.ts") or f.endswith(".spec.ts") or f.endswith("_test.go")
                    if f in found_files or is_test_file:
                        fpath = os.path.join(root, f)
                        if os.path.isfile(fpath) and os.path.getsize(fpath) < 30000:
                            rel_path = os.path.relpath(fpath, project_path)
                            with open(fpath, "r", encoding="utf-8", errors="ignore") as file_obj:
                                content = file_obj.read()
                            ext = os.path.splitext(f)[1].lower().replace(".", "")
                            lang = "python" if ext == "py" else ("javascript" if ext in ["js", "ts"] else ("go" if ext == "go" else ""))
                            injected_files_context += f"\n### File: {rel_path}\n```{lang}\n{content}\n```\n"
    except Exception as e:
        print(f"[DEVELOPER] Error parsing traceback files: {e}")
    return injected_files_context


def _parse_tool_call(text: str) -> dict | None:
    """Extract a tool call JSON block from the LLM response."""
    if not text:
        return None

    def repair_json_newlines(s: str) -> str:
        in_string = False
        escape = False
        chars = []
        for c in s:
            if c == '"' and not escape:
                in_string = not in_string
            if in_string:
                if c == '\n':
                    chars.append('\\n')
                elif c == '\r':
                    chars.append('\\r')
                elif c == '\t':
                    chars.append('\\t')
                else:
                    chars.append(c)
            else:
                chars.append(c)
            if c == '\\' and in_string:
                escape = not escape
            else:
                escape = False
        return "".join(chars)

    def try_parse(raw: str) -> dict | None:
        try:
            return json.loads(raw)
        except json.JSONDecodeError:
            pass
        try:
            return json.loads(repair_json_newlines(raw))
        except json.JSONDecodeError:
            pass
        return None

    def robust_json_parse(raw: str) -> dict | None:
        import re
        # Repair single backslashes in Windows paths (e.g. \M -> \\M)
        pattern = re.compile(r'\\(?:[\\"/bfnrt]|u[0-9a-fA-F]{4})|\\')
        raw_clean = pattern.sub(lambda m: '\\\\' if m.group(0) == '\\' else m.group(0), raw)

        res = try_parse(raw_clean)
        if res is not None:
            return res

        # Repair common array-quote bracket hallucination: replace `" "]"` or `" ]"` with `"]"`
        raw_rep = re.sub(r'"\s*"\]"', '"]', raw_clean)
        res = try_parse(raw_rep)
        if res is not None:
            return res

        # Fallback to regex-based extraction if json.loads failed
        try:
            tool_match = re.search(r'"tool"\s*:\s*"([^"]+)"', raw)
            if tool_match:
                tool_name = tool_match.group(1)
                args_dict = {}
                args_match = re.search(r'"args"\s*:\s*\{(.*)\}', raw, re.DOTALL)
                if args_match:
                    args_content = args_match.group(1).strip()
                    keys = ["file_path", "goal", "analysis", "proposed_changes", "steps", "pattern", "path", "recursive", "command", "background", "offset", "limit", "content", "name", "task", "old_string", "new_string", "diff", "url", "max_chars", "query", "top_k", "glob", "timeout", "selectors", "selector", "what", "wait_ms", "description", "file_refs", "key", "value", "title", "text"]
                    for key in keys:
                        # Match string values
                        string_pat = rf'"{key}"\s*:\s*"(.*?)"\s*(?:,|\n\s*"|\}}|$)'
                        m = re.search(string_pat, args_content, re.DOTALL)
                        if m:
                            val = m.group(1)
                            val = val.replace('\\n', '\n').replace('\\t', '\t').replace('\\"', '"').replace('\\\\', '\\')
                            args_dict[key] = val
                            continue
                        # Match list values
                        list_pat = rf'"{key}"\s*:\s*\[(.*?)\]'
                        m = re.search(list_pat, args_content, re.DOTALL)
                        if m:
                            list_content = m.group(1).strip()
                            items = []
                            for item in re.findall(r'"(.*?)"', list_content, re.DOTALL):
                                items.append(item.replace('\\"', '"').replace('\\\\', '\\'))
                            args_dict[key] = items
                            continue
                        # Match boolean values
                        bool_pat = rf'"{key}"\s*:\s*(true|false)'
                        m = re.search(bool_pat, args_content, re.IGNORECASE)
                        if m:
                            args_dict[key] = m.group(1).lower() == "true"
                            continue
                        # Match integer values
                        int_pat = rf'"{key}"\s*:\s*(\d+)'
                        m = re.search(int_pat, args_content)
                        if m:
                            args_dict[key] = int(m.group(1))
                            continue
                return {"tool": tool_name, "args": args_dict}
        except Exception:
            pass

        return None

    import re
    # Match ```tool ... ``` blocks
    pattern = r'```tool\s*\n(.*?)\n```'
    match = re.search(pattern, text, re.DOTALL)
    if match:
        raw_json = match.group(1).strip()
        parsed = robust_json_parse(raw_json)
        if parsed is not None:
            return parsed

    return None


def _parse_xml_tool_call(text: str) -> list[dict]:
    """
    Parse tool calls from XML-style formats that flash models emit.
    Handles:
      A) <function><tool_name>X</tool_name><args><param>k</param><value>v</value></args></function>
      B) <read_file file_path="..." offset="N" />  (self-closing, tool=tag name, attrs=args)
      C) <read_file><file_path>/path</file_path></read_file> (wrapping, child elements = args)
    Returns list of {tool, args} dicts.
    """
    import re
    results = []

    # â”€â”€ Format A: <function> wrapper â”€â”€
    # Matches <function>...<tool_name>NAME</tool_name>...<args>...<param>K</param><value>V</value>...</args>...</function>
    func_pattern = r'<function>\s*<tool_name>(.*?)</tool_name>\s*<args>(.*?)</args>\s*</function>'
    for m in re.finditer(func_pattern, text, re.DOTALL | re.IGNORECASE):
        tool_name = m.group(1).strip()
        args_block = m.group(2)
        args = {}
        # Parse <param>key</param><value>val</value> pairs
        pairs = re.findall(r'<param>(.*?)</param>\s*<value>(.*?)</value>', args_block, re.DOTALL | re.IGNORECASE)
        for k, v in pairs:
            k = k.strip()
            v = v.strip()
            # Try to preserve type: int, bool, or string
            if v.lower() == 'true':
                args[k] = True
            elif v.lower() == 'false':
                args[k] = False
            elif v.isdigit():
                args[k] = int(v)
            else:
                args[k] = v
        if tool_name:
            results.append({"tool": tool_name, "args": args})

    # â”€â”€ Format B: Self-closing XML tags (common tool names) â”€â”€
    known_tools = [
        "read_file", "write_file", "edit_file", "run_command",
        "search_code", "list_files", "view_signatures",
        "query_history", "compact_conversation",
        "task", "start_async_task", "check_async_task", "list_async_tasks",
    ]
    for tool in known_tools:
        # Self-closing: <read_file file_path="x" offset="5" />
        sc_pattern = rf'<{tool}\s+(.*?)\s*/?\s*>'
        for m in re.finditer(sc_pattern, text, re.DOTALL | re.IGNORECASE):
            attrs_str = m.group(1)
            args = {}
            # Parse key="value" attributes
            attr_pairs = re.findall(r'(\w+)\s*=\s*"([^"]*)"', attrs_str)
            for k, v in attr_pairs:
                k = k.strip()
                v = v.strip()
                if v.lower() == 'true':
                    args[k] = True
                elif v.lower() == 'false':
                    args[k] = False
                elif v.isdigit():
                    args[k] = int(v)
                else:
                    args[k] = v
            if args:
                results.append({"tool": tool, "args": args})

        # Wrapping format: <read_file><file_path>x</file_path><offset>5</offset></read_file>
        wrap_pattern = rf'<{tool}>(.*?)</{tool}>'
        for m in re.finditer(wrap_pattern, text, re.DOTALL | re.IGNORECASE):
            inner = m.group(1)
            args = {}
            # Parse <key>value</key> children
            child_pairs = re.findall(r'<(\w+)>(.*?)</\1>', inner, re.DOTALL | re.IGNORECASE)
            for k, v in child_pairs:
                k = k.strip()
                v = v.strip()
                if v.lower() == 'true':
                    args[k] = True
                elif v.lower() == 'false':
                    args[k] = False
                elif v.isdigit():
                    args[k] = int(v)
                else:
                    args[k] = v
            if args:
                results.append({"tool": tool, "args": args})

    # â”€â”€ Format C: DSML (DeepSeek native tool calling) â”€â”€
    # ||DSML||tool_calls>
    # ||DSML||invoke name="list_files">
    # ||DSML||parameter name="path" string="true">D:\path</||DSML||parameter>
    # </||DSML||invoke>
    # </||DSML||tool_calls>
    dsml_invoke = re.findall(
        r'\|\|DSML\|\|invoke\s+name="(\w+)"\s*>(.*?)</\|\|DSML\|\|invoke\s*>',
        text, re.DOTALL
    )
    for tool_name, params_block in dsml_invoke:
        args = {}
        param_matches = re.findall(
            r'\|\|DSML\|\|parameter\s+name="(\w+)"[^>]*>(.*?)</\|\|DSML\|\|parameter\s*>',
            params_block, re.DOTALL
        )
        for k, v in param_matches:
            k = k.strip()
            v = v.strip()
            if v.lower() == 'true':
                args[k] = True
            elif v.lower() == 'false':
                args[k] = False
            elif v.isdigit():
                args[k] = int(v)
            else:
                args[k] = v
        if tool_name:
            results.append({"tool": tool_name, "args": args})

    return results


def parse_all_tool_calls(text: str) -> list[dict]:
    """Extract all tool call blocks from the LLM response.

    Supports three format families:
      1. ```tool JSON blocks (our canonical format)
      2. XML-style (<function>, <tool_name/>, self-closing tags)
      3. DSML (DeepSeek native ||DSML||invoke)
    """
    if not text:
        return []
    import re
    results = []

    # â”€â”€ Format 1: ```tool JSON blocks (canonical) â”€â”€
    pattern = r'```tool\s*\n(.*?)\n```'
    matches = re.findall(pattern, text, re.DOTALL)
    for raw_json in matches:
        raw_json = raw_json.strip()
        parsed = _parse_tool_call(f"```tool\n{raw_json}\n```")
        if parsed:
            results.append(parsed)

    # â”€â”€ Format 1b: JSON array of tools â”€â”€
    if not results:
        try:
            import json
            array_pattern = r'(\[\s*\{\s*"tool"\s*:.*\}\s*\])'
            array_match = re.search(array_pattern, text, re.DOTALL)
            if array_match:
                parsed_list = json.loads(array_match.group(1))
                if isinstance(parsed_list, list):
                    for item in parsed_list:
                        if isinstance(item, dict) and "tool" in item:
                            results.append(item)
        except Exception:
            pass

    # â”€â”€ Format 2+3: XML and DSML (flash model native output) â”€â”€
    xml_results = _parse_xml_tool_call(text)
    # â”€â”€ Format 4: <function_calls><invoke> (DeepSeek alternate native format) â”€â”€
    # <function_calls>
    # <invoke name="read_file">
    # <parameter name="file_path">index.html</parameter>
    # </invoke>
    # </function_calls>
    fc_pattern = r'<function_calls>(.*?)</function_calls>'
    for fc_m in re.finditer(fc_pattern, text, re.DOTALL | re.IGNORECASE):
        fc_block = fc_m.group(1)
        for inv_m in re.finditer(r'<invoke\s+name="(\w+)"\s*>(.*?)</invoke>', fc_block, re.DOTALL | re.IGNORECASE):
            tool_name = inv_m.group(1)
            inv_body = inv_m.group(2)
            args = {}
            for pm in re.finditer(r'<parameter\s+name="(\w+)"[^>]*>(.*?)</parameter>', inv_body, re.DOTALL):
                k = pm.group(1).strip()
                v = pm.group(2).strip()
                # Handle string="true" annotated values
                string_match = re.match(r'^string="true">(.*)', pm.group(0), re.DOTALL)
                if string_match:
                    v = string_match.group(1).strip()
                if v.lower() == 'true':
                    args[k] = True
                elif v.lower() == 'false':
                    args[k] = False
                elif v.isdigit() and len(v) < 10:
                    args[k] = int(v)
                else:
                    args[k] = v
            if tool_name:
                results.append({"tool": tool_name, "args": args})

    # â”€â”€ Format 5: <tool_call> (DeepSeek native function calling format) â”€â”€
    # <tool_call name="list_files">
    # {"path": "D:/test", "recursive": true}
    # </tool_call>
    tc_pattern = r'<tool_call\s+name="(\w+)"\s*>(.*?)</tool_call>'
    for m in re.finditer(tc_pattern, text, re.DOTALL | re.IGNORECASE):
        tool_name = m.group(1)
        args_block = m.group(2).strip()
        args = {}
        # Try JSON args first
        try:
            args = json.loads(args_block)
        except Exception:
            # Fallback: try to repair and extract JSON-like key:value pairs
            # LLM often outputs unescaped HTML in content fields breaking json.loads
            try:
                # Find file_path
                fp_match = re.search(r'"file_path"\s*:\s*"((?:[^"\\]|\\.)*)"', args_block)
                if fp_match:
                    args["file_path"] = fp_match.group(1).replace('\\\\', '\\')
                # Find content (greedy, up to last " before closing or end)
                content_match = re.search(r'"content"\s*:\s*"(.*)"(?:\s*\})?\s*$', args_block, re.DOTALL)
                if content_match:
                    args["content"] = content_match.group(1)
                # Find pattern
                pat_match = re.search(r'"pattern"\s*:\s*"((?:[^"\\]|\\.)*)"', args_block)
                if pat_match:
                    args["pattern"] = pat_match.group(1).replace('\\\\', '\\')
                # Find command
                cmd_match = re.search(r'"command"\s*:\s*"((?:[^"\\]|\\.)*)"', args_block)
                if cmd_match:
                    args["command"] = cmd_match.group(1).replace('\\\\', '\\')
                # Find other simple string/int args
                for pm in re.finditer(r'"(\w+)"\s*:\s*(true|false|\d+|"[^"]*")', args_block):
                    k = pm.group(1)
                    if k in ("file_path", "content", "pattern", "command"):
                        continue  # Already handled
                    v = pm.group(2)
                    if v == 'true':
                        args[k] = True
                    elif v == 'false':
                        args[k] = False
                    elif v.isdigit():
                        args[k] = int(v)
                    else:
                        args[k] = v.strip('"')
            except Exception:
                pass
            # If still no args, try XML param/value fallback
            if not args:
                for pm in re.finditer(r'<param\s+name="(\w+)"[^>]*>(.*?)</param>', args_block, re.DOTALL):
                    k = pm.group(1).strip()
                    v = pm.group(2).strip()
                    if v.lower() == 'true':
                        args[k] = True
                    elif v.lower() == 'false':
                        args[k] = False
                    elif v.isdigit():
                        args[k] = int(v)
                    else:
                        args[k] = v
        if tool_name:
            results.append({"tool": tool_name, "args": args})

    # â”€â”€ Format 6: Anthropic-style tool_use â”€â”€
    # <Tool-Name id="toolu_...">write_file</Tool-Name>
    # <Parameter name="file_path">/path/to/file</Parameter>
    # <Parameter name="content">file content here</Parameter>
    anthro_tools = {}
    for m in re.finditer(r'<Tool-Name[^>]*>(\w+)</Tool-Name>', text, re.DOTALL | re.IGNORECASE):
        tool_name = m.group(1)
        # Find all <Parameter> tags after this Tool-Name until next Tool-Name or end
        search_start = m.end()
        next_tool = re.search(r'<Tool-Name[^>]*>', text[search_start:], re.IGNORECASE)
        param_block = text[search_start:search_start + next_tool.start()] if next_tool else text[search_start:]
        args = {}
        for pm in re.finditer(r'<Parameter\s+name="(\w+)"\s*>(.*?)</Parameter>', param_block, re.DOTALL | re.IGNORECASE):
            k = pm.group(1).strip()
            v = pm.group(2).strip()
            if v.lower() == 'true':
                args[k] = True
            elif v.lower() == 'false':
                args[k] = False
            elif v.isdigit() and len(v) < 10:
                args[k] = int(v)
            else:
                args[k] = v
        if tool_name and tool_name not in anthro_tools:
            anthro_tools[tool_name] = args
    for tool_name, args in anthro_tools.items():
        results.append({"tool": tool_name, "args": args})

    # â”€â”€ Format 7: <tool> JSON wrapper â”€â”€
    # <tool>
    # {"tool": "list_files", "args": {"path": ".", "recursive": false}}
    # </tool>
    tool_xml_matches = re.finditer(r'<tool>\s*(.*?)\s*</tool>', text, re.DOTALL | re.IGNORECASE)
    for m in tool_xml_matches:
        inner_str = m.group(1).strip()
        # Find the outermost JSON object by counting braces
        if inner_str.startswith('{'):
            depth = 0
            json_end = 0
            for i, c in enumerate(inner_str):
                if c == '{':
                    depth += 1
                elif c == '}':
                    depth -= 1
                    if depth == 0:
                        json_end = i + 1
                        break
            if json_end > 0:
                inner_str = inner_str[:json_end]
        try:
            inner = json.loads(inner_str)
            if isinstance(inner, dict) and "tool" in inner:
                results.append({"tool": inner["tool"], "args": inner.get("args", {})})
        except Exception:
            pass

    # â”€â”€ Normalize and Clean Up Formats â”€â”€
    for r in results:
        if not isinstance(r, dict):
            continue
        if "tool" not in r:
            r["tool"] = ""
        if "args" not in r or not isinstance(r["args"], dict):
            r["args"] = {k: v for k, v in r.items() if k not in ("tool", "args")}
        
        # Keep only "tool" and "args" keys in the final dict
        for k in list(r.keys()):
            if k not in ("tool", "args"):
                r.pop(k, None)

    # â”€â”€ Deduplicate across all formats â”€â”€
    def _make_hashable(obj):
        """Recursively convert dict values to hashable for dedup."""
        if isinstance(obj, dict):
            return tuple(sorted((k, _make_hashable(v)) for k, v in obj.items()))
        elif isinstance(obj, list):
            return tuple(_make_hashable(i) for i in obj)
        return obj

    seen_keys = set()
    deduped = []
    for r in results:
        key = (r["tool"], _make_hashable(r["args"]))
        if key not in seen_keys:
            seen_keys.add(key)
            deduped.append(r)
    return deduped
