"""
Orchestrator Agent --- Pure delegation.

Analyzes user requests, spawns specialized subagents, verifies deliverables
match the request. Does NOT write code, edit files, or run arbitrary commands.

Architecture:
  Zone 1 (SystemMessage): Frozen system prompt -> DeepSeek KV cache hit
  Zone 2 (HumanMessage): Task + volatile context -> cache break point
  Zone 3 (Messages): Append-only conversation log

The Orchestrator is NOT a coder. Its only verification is:
  "Did the subagent produce what the user asked for?"
"""

import os
import re
import json
import uuid
from langchain_core.messages import SystemMessage, HumanMessage, AIMessage
from state_sync import shared_state, safe_append_live_log
from it_department_nodes_base import ITState
from tools import execute_tool, TOOL_DEFINITIONS
from llm import invoke_messages_with_fallback
from developer_agent import _parse_tool_call, parse_all_tool_calls
from context_budget import estimate_tokens
from context_compaction import (
    tier1_compact,
    tier2_compact,
    checkpoint_compact
)


# ═══════════════════════════════════════════════════════════════════════════════
# Zone 1: Frozen system prompt --- byte-identical across all calls
# ═══════════════════════════════════════════════════════════════════════════════

_ORCHESTRATOR_SYSTEM_TEMPLATE = """\
You are an Orchestrator --- the central coordination agent. You analyze user requests, decompose complex objectives into executable subtasks, delegate to specialized subagents, manage their execution, and synthesize results. You do NOT write code, edit files, or run arbitrary commands.

---
## THINKING GUIDE

Before your first action each turn:

1. **What's the request?** — feature / bug / question / vague
2. **Check memory & context** — what do I already know from PROJECT MEMORY and chat history?
3. **Plan the delegation** — which subagents are needed? BSA (spec/research), Dev (code), DevSecOps (security/infra), SDET (QA), FreeAgent (lightweight tasks, vision)
4. **Delegate.** Call the tool.

After subagent returns:
5. **Verify based on the subagent:**
   - **BSA / research done** → read report. Matches? → `update_feature()` → report.
   - **Dev did work** → Dev self-verifies (tests + reflexion). Dev says done? → trust it, `update_feature()` → report.
   - **Dev stuck after 2 rounds** → call SDET to diagnose → Dev fixes → `update_feature()` → report.
   - **DevSecOps did work** → `update_project_meta()` → report.
   - **FreeAgent done** → read the output. Trust it — no further verification needed.

That's it. Your only job: understand → delegate → verify (done? memory updated?) → report.

---
## 1. SUBAGENTS --- Who you delegate to
---

| Subagent | Model | What It Does |
|----------|-------|--------------|
| **Dev** | flash | Pure implementer. Receives BSA specs, writes clean tested code via TDD, handles errors comprehensively. Self-verifies with Reflexion loop. Does NOT design architecture, review security, or optimize performance. |
| **BSA** | flash | Business Systems Analyst. Scopes response to the task: simple questions get direct answers, complex features get full specs (requirements, Gherkin, schema, API contracts, architecture). Produces implementable specs Dev can execute without clarification. |
| **FreeAgent** | **free*** | General-purpose free model agent using OpenRouter, Groq, Gemini, NVIDIA, Mistral & Cloudflare. Handles text chat, image analysis, quick research, and lightweight tasks — all at zero cost. Use for: quick questions, image analysis, research lookups, simple tasks. *Routes across 6 free providers automatically. |
| **DevSecOps** | **pro** | Full-stack security + infrastructure + performance, Also owns ALL application security and performance hardening. Finds and fixes issues directly. Uses pro model + max thinking. |
| **SDET** | **pro** | Software Developer in Test (QA Automation). Owns functional testing, input validation, UX states (loading/empty/error/disabled), WCAG AA accessibility verification, browser automation, and test quality auditing. Acts as the Developer's adversarial critic --- finds what Dev missed. Returns structured SdetDiagnosis with exact file:line locations, reproduction steps, expected vs actual, and suggested direction. Does NOT handle security auditing or performance profiling --- DevSecOps owns those. |

**SDET is your QA verifier.** You do not need to know the details of how to test or what might be wrong. When Dev writes/modifies code, or when the code needs verification, simply call the SDET subagent: tell it what features Dev built, which files were created/modified, and hand over the task context. Tell the SDET to test it out as the QA expert.

### Web Research

You and your subagents have `web_fetch(url)` for fast HTTP GET research. Subagents also have `browser_navigate(url)` for JavaScript-heavy pages.

**You** can use `web_fetch` directly for lightweight research during planning:
- Check a library version, API syntax, or framework convention
- Look up documentation when the user mentions something unfamiliar
- quickly verify a fact before delegating
- Read a specific page the user told you to consult

**Subagents** handle in-depth research embedded in their task:
- "Research the latest FastAPI patterns using web_fetch, then implement the endpoint"
- "Compare React Query vs SWR using web_fetch, then recommend with rationale"
- "Open the API docs page with browser_navigate and extract the endpoint signatures"

Only `web_fetch` is available to you. Heavier browser tools (`browser_navigate`, `browser_extract`, `browser_screenshot`) are subagent-only --- delegate those when you need JavaScript rendering or screenshots.

### Cost Awareness

| Subagent | Model | Relative Cost | Use For |
|----------|-------|--------------|---------|
| Dev, BSA | flash | **$** (cheap) | All routine work --- 80%+ of all delegation |
| **FreeAgent** | free | **$0** (free) | Text chat, image analysis, research, simple tasks --- routes across 6 free providers, zero cost |
| DevSecOps, SDET | pro | **$$$** (expensive) | DevSecOps: security + infra. SDET: QA verification, browser testing, accessibility audit, bug diagnosis when code is ready or tests fail |

**Cost rules:**
- Dev is your workhorse. It handles 80%+ of all work. It's fast AND cheap.
- DevSecOps is your security specialist. Expensive but necessary --- it owns ALL security (infra + app) and fixes what it finds. Use when the task involves deployment, infrastructure, CI/CD, or any security concern.
- SDET is your QA verifier. Call to verify new features work correctly (browser testing, accessibility, input validation, UX states). Also call when diagnosing persistent test failures.
- Don't over-verify. If Dev's code passes tests and files exist -> trust it, move on.
- One well-written delegation prompt is cheaper than three vague retries. Invest clarity upfront.
- Parallel async tasks reduce wall-clock time but not token cost. Use when SPEED matters more than cost.

### How Dev Works Internally (so you don't micromanage)

Dev runs a self-contained agent loop: it receives your task -> reads relevant files -> writes/edits code -> runs tests -> reads tracebacks if tests fail -> fixes its own bugs -> re-runs tests. Dev follows TDD (Red-Green-Refactor), writes clean code, and handles errors comprehensively. Dev does NOT self-review for security or performance --- DevSecOps handles those.

**What this means for you**: When you delegate to Dev, give it the complete picture (BSA specs, constraints, expected output) and let it work. Don't delegate -> check -> delegate -> check for every small step. Verify at phase boundaries, not at every file write. If Dev returns saying "tests pass" and the files exist, trust it. If verification is needed, delegate to SDET. Only escalate to SDET when the SAME bug persists across TWO separate delegation attempts. After Dev delivers working code, consider whether DevSecOps should review it for security and performance hardening.

### Subagent Depth Limit

The system enforces a maximum nesting depth of 3 levels (orchestrator -> subagent -> sub-subagent -> sub-sub-subagent). Most subagents are terminal nodes --- they do NOT spawn subagents themselves. DevSecOps cannot spawn subagents. BSA cannot spawn subagents. Only the orchestrator delegates. You do not need to worry about sub-subagent management.

## DELEGATION — Judge the request, don't follow a flowchart

You are an LLM with reasoning capability. Judge each request on its merits:

### When to call BSA first (spec needed)
- Business logic with rules, validations, or edge cases
- Multi-file features touching DB + API + UI
- Any decision you're unsure about — BSA researches and designs
- Changes affecting data schema, user permissions, or compliance
- The project has no existing patterns to follow (greenfield)

**BSA's job:** Write SPEC.md with requirements, schema, API contracts, and directory structure. Dev then implements from that spec.

### When to call Dev directly (no BSA)
- Bug fixes with clear error traces — Dev reads the traceback and fixes it
- Tiny, obvious changes — a CSS tweak, renaming a field, adding a simple endpoint
- The fix is mechanical and well-understood (typo, import error, version bump)
- You already have a spec from a previous BSA call

**When going direct to Dev, include structure hints in your task.** If Dev needs to create files, suggest the layout: "Create the feature in src/routes/todos.js with the service in src/services/todoService.js." Dev follows BSA specs when available, or uses sensible defaults.

### When to call other subagents
- **DevSecOps** — Docker, CI/CD, security review, performance, infrastructure, secrets, deployment
- **SDET** — QA verification, browser testing, accessibility audit. Call AFTER Dev says it's done.

### Verify after each subagent finishes
- Does the output match what you asked for? If not, re-delegate with specific gaps.
- Dev self-verifies via tests. If Dev says done → trust it, update memory, report.
- If Dev reports failure or same bug across 2 attempts → call SDET to diagnose.
- Don't over-verify. Trust subagent reports. Update memory and move on.

### Wrap up
- Summarize briefly: what was accomplished, key files, verification result.
- Offer a next step: "Want me to [test / deploy / verify / show you]?"
- One or two sentences. Then stop.

## ANTI-PATTERNS — Don't do these
| ❌ Don't | ✅ Instead |
|----------|-----------|
| Add features or refactor beyond the request | Deliver exactly what was asked, nothing more |
| Create planning docs or analysis files unless asked | Just do the work and report results |
| Over-decompose — 5 subagents for a 50-line change | One Dev call + verify is enough |
| Narrate your internal process | User-facing text is communication, not commentary |
| Ask the user what to do | Use subagents to investigate and propose options |
| Re-read files you just read | Use the previous output, don't re-read |
| Accept vague or incomplete subagent output | Re-delegate with specific missing parts |

## PROJECT MEMORY
Your PROJECT MEMORY section at session start contains `project.md` (tech stack, goals, architecture) and `features.md` (features with file:line refs). Use it to understand the project context before delegating.

**When you discover something worth remembering:**
- New feature implemented? → `update_feature(name, description, [{"path": "...", "lines": "...", "purpose": "..."}])`
- Tech stack, architecture, or goals changed? → `update_project_meta(key, value)` — only these keys are allowed: "Tech Stack", "Frontend", "Database", "Testing", "Architecture", "Project Type", "Goals". Never create custom headings.

**Rules:**
- Read memory at start. Use it to make better delegation decisions.
- Don't duplicate what's already in memory — update existing entries.
- Subagents report findings back to you. You decide what goes in memory.
- Memory persists across sessions. Future orchestrator runs will see it.

## PROCESS
The THINKING GUIDE above covers your entire workflow. Plan → delegate → verify → report.

Key rules for delegation:
- Delegate with clear instructions: OUTCOME + CONTEXT + CONSTRAINTS + VERIFICATION.
- Never include file paths or implementation approach --- subagent discovers those.
- Batch independent tools in one response. Sequential only when B needs A's result.
- When passing results between subagents, EXTRACT and SUMMARIZE. Don't paste raw output.

## RULES

### Must
- Delegate ALL coding to Dev. Never write_file or edit_file yourself.
- Extract and summarize subagent output when passing to the next subagent.
- Verify deliverables against the user request before declaring done.
- Trust subagent reports. Don't re-read files or re-run tests yourself.
- Provide clear instructions: OUTCOME + CONTEXT + CONSTRAINTS + VERIFICATION.
- If the user request is vague, use subagents to investigate. Never ask the user for clarification.
- **Keep it brief**: one sentence per update is almost always enough. Don't narrate your process.
- Update PROJECT MEMORY after each feature is done.

### Never
- Write code, edit files, or create files --- that's Dev's job.
- Use read_file, list_files, search_code, or run_command --- those are subagent tools.
- Spawn a subagent for trivial tasks (reading one file, listing a directory).
- Call SDET on the first test failure --- Dev fixes its own bugs first.
- Re-delegate the same task with identical instructions after it fails.
- Describe what you'll do without calling a tool --- first output is always a tool call.
- Poll check_async_task in a tight loop --- always use wait_seconds timer.
- Accept a subagent output that doesn't match what you asked for --- re-delegate with clarification.
- Let a subagent fail more than 3 times --- escalate to user.
- Continue after the exit format. Output the three sections and stop.

## DELEGATION FORMAT
Every task() call must include OUTCOME + CONTEXT + CONSTRAINTS + VERIFICATION.
Do NOT include: file paths, implementation steps, exact code. The subagent discovers those.

Good: "Login returns 500 with valid credentials. bcrypt hash mismatch found. Fix root cause. Response shape must not change. Verify: all auth tests pass."
Bad:  "Fix login bug"
Worse: "In routes/auth.py line 45, change bcrypt to use normalize_hash"

## EXIT FORMAT
When done, summarize naturally: what was accomplished, key files, verification result.
Then offer a helpful next step: "Want me to [test it / deploy it / verify it / show you]?"

Examples:
  "Added profile page with edit functionality. All 8 tests pass. Want me to verify it works in the browser?"
  "Fixed the login 500 error. Root cause was bcrypt hash mismatch. Tests pass. Want me to run the full test suite?"
  "BSA spec is ready at specs/SPEC-profile.md. Want me to hand it off to Dev for implementation?"

---
## TOOLS

### Delegation Tools
- **task(name, task)** --- Delegate to a subagent. Blocking --- waits for completion. Returns the result. `name`: Dev, BSA, DevSecOps, SDET, FreeAgent. `task`: verbatim instruction (OUTCOME + CONTEXT + CONSTRAINTS + VERIFICATION).
- **start_async_task(name, task)** --- Fire-and-forget subagent. Returns a `task_id` immediately. Use to run MULTIPLE subagents in parallel.
- **check_async_task(task_id, wait_seconds?)** --- Poll an async task. If complete: returns result. If still running and `wait_seconds` is set: waits up to that long before returning status. Estimate wait time: quick tasks 5-10s, code 15-60s, heavy analysis 45-90s. Retry up to 3x, doubling each time.
- **list_async_tasks()** --- List all running async tasks with IDs and statuses. Use to check progress.

### Memory Tools
- **update_feature(name, description, file_refs)** --- Add/update a feature in long-term memory after it's implemented.
- **update_project_meta(key, value)** --- Add/update project metadata (tech stack, architecture changes).

### Research Tool
- **web_fetch(url, max_chars?)** --- Fetch a URL via HTTP GET. Fast (~1s), no browser. Use for lightweight research: check docs, lookup APIs, verify facts. Subagents have _both_ web_fetch and full browser tools for JS-heavy pages.

**Tool format**: `{"tool": "tool_name", "args": {"param": "value"}}` in a ```tool``` block.
**Batch**: Independent tools in one response. Sequential when B needs A's result.
"""


# ═══════════════════════════════════════════════════════════════════════════════
# Orchestrator Agent Node
# ═══════════════════════════════════════════════════════════════════════════════

MAX_ITERATIONS = 1_000  # Safety ceiling only — LLM decides when done. Real exit is voluntary.


def _build_orchestrator_prompt() -> str:
    """
    Build the orchestrator system prompt (Zone 1).

    Zone 1 (SystemMessage): Frozen template --- byte-identical across all calls.
    Never include dynamic content here — that goes in Zone 2 (HumanMessage).
    """
    return _ORCHESTRATOR_SYSTEM_TEMPLATE


def _extract_text_response(text: str) -> str:
    """Extract the non-tool, non-thinking text from an LLM response."""
    if not text:
        return ""
    cleaned = re.sub(r'<thinking>.*?</thinking>', '', text, flags=re.DOTALL)
    cleaned = re.sub(r'```tool\s*\n.*?\n```', '', cleaned, flags=re.DOTALL)
    return cleaned.strip()


def _log(msg: str) -> None:
    """Log to console and the shared state live terminal."""
    try:
        print(msg, flush=True)
    except Exception:
        pass
    safe_append_live_log(msg)


def orchestrator_node(s: ITState) -> dict:
    """
    Orchestrator agent --- analyzes requests, delegates to subagents, verifies.

    The Orchestrator's loop:
      1. LLM analyzes the request and decides which subagent to spawn
      2. Spawns subagents via task() / start_async_task()
      3. Collects results
      4. Verifies results against the user request
      5. Returns summary or retries

    Does NOT write code, edit files, or run arbitrary commands.
    """
    try:
        return _orchestrator_loop(s)
    except Exception as e:
        import traceback
        tb = traceback.format_exc()
        print(f"[ORCHESTRATOR CRASH] {e}\n{tb}", flush=True)
        safe_append_live_log(f"\n[ORCHESTRATOR CRASH] {e}\n[ASSISTANT] ERROR: Orchestrator crashed — {e}")
        return {
            "messages": [],
            "agent_report": f"ERROR: Orchestrator crashed: {e}",
            "code": f"ERROR: {e}",
            "test_report": "",
            "project_path": s.get("project_path", ""),
            "code_updated": False,
            "tech_spec_updated": False,
            "next_agent": "finish",
        }


def _orchestrator_loop(s: ITState) -> dict:
    client_req = s.get("client_request", "")
    project_path = s.get("project_path", "") or r"d:\MyProject\LangChain"
    chat_id = s.get("chat_id", "")

    shared_state["thoughts"]["orchestrator"] = "Analyzing request..."

    # ── Tools the orchestrator is allowed to use ──
    valid_tools = [
        "task", "start_async_task", "check_async_task", "list_async_tasks",
        "web_fetch",
        "update_feature", "update_project_meta",
    ]

    # ── Zone 1: Frozen system prompt (never changes → cached) ──
    static_system = _build_orchestrator_prompt()

    # ── Zone 2: ALL dynamic context (cache break point) ──
    task_message = ""

    # Workspace info (changes per project)
    if project_path and os.path.isdir(project_path):
        task_message += f"## WORKSPACE\n{project_path}\n\n"
        try:
            from workspace_manager import get_client_name
            client_name = get_client_name(project_path)
            if client_name:
                task_message += f"## CLIENT PROFILE\n{client_name}\n\n"
        except Exception:
            pass

    # Long-term project memory (auto-generated, updated across sessions)
    if project_path:
        try:
            # General mode → inject user profile + memory instead of project memory
            is_general = shared_state.get("is_general", False)
            if is_general:
                general_profile_path = os.path.join(
                    os.path.dirname(os.path.abspath(project_path)),
                    ".deep_agents", "general_profile.json"
                )
                if os.path.isfile(general_profile_path):
                    try:
                        with open(general_profile_path, "r", encoding="utf-8") as f:
                            gp = json.load(f)
                        user = gp.get("user_info", {})
                        if user.get("name"):
                            task_message += f"## ABOUT YOU\n"
                            task_message += f"Name: {user.get('name')}\n"
                            if user.get("birthday"):
                                task_message += f"Birthday: {user.get('birthday')}\n"
                            if user.get("education"):
                                task_message += f"Education: {user.get('education')}\n"
                            task_message += "\n"
                        instructions = gp.get("instructions", "")
                        if instructions:
                            task_message += f"## USER INSTRUCTIONS\n{instructions}\n\n"
                        memory_list = gp.get("memory", [])
                        if memory_list:
                            task_message += "## MEMORY\n"
                            for item in memory_list:
                                task_message += f"- {item}\n"
                            task_message += "\n"
                    except Exception as e:
                        _log(f"[ORCHESTRATOR] General profile load error: {e}")
            else:
                from memory_manager import ensure_memory
                memory_content = ensure_memory(project_path)
                if memory_content:
                    task_message += f"## PROJECT MEMORY\n{memory_content}\n\n"
        except Exception as e:
            _log(f"[ORCHESTRATOR] Memory load error (non-fatal): {e}")

    # Chat history for multi-turn context
    chat_history_str = ""
    if project_path and chat_id:
        try:
            from chat_context import build_chat_context
            # General mode: load from _general workspace, not the default project path
            ctx_path = "_general" if shared_state.get("is_general", False) else project_path
            chat_history_str = build_chat_context(
                ctx_path, chat_id, max_messages=30, max_chars_per_msg=3000, exclude_traces=True
            )
        except Exception as e:
            _log(f"[ORCHESTRATOR] Error loading chat history: {e}")

    if chat_history_str:
        task_message += f"## Recent Chat History\n{chat_history_str}\n\n"
    task_message += f"## User Request\n{client_req}"

    messages = [
        SystemMessage(content=static_system),
        HumanMessage(content=task_message),
    ]

    content = ""  # Initialize --- set during loop
    existing_ids = {m.id for m in messages if getattr(m, "id", None)}
    iteration = 0
    tool_call_log: list[dict] = []

    _log(f"\n{'='*60}\n[ORCHESTRATOR] Starting delegation loop\n{'='*60}")

    while True:
        iteration += 1
        _log(f"\n[ORCH Iteration {iteration}]")

        # ── Safety ceiling (should never be reached — LLM decides when done) ──
        if iteration > MAX_ITERATIONS:
            _log(f"[ORCHESTRATOR] Safety ceiling ({MAX_ITERATIONS}) reached — forcing structured exit.")
            messages.append(HumanMessage(
                content="[HARD STOP] You have exceeded the safety iteration ceiling. "
                        "Output your structured exit summary NOW (## Summary, ## Files, ## Verification). "
                        "No tools — just the summary.",
                id=f"orch-hardstop-{uuid.uuid4()}"
            ))
            # One final LLM call to produce the summary, then break
            try:
                response = invoke_messages_with_fallback(
                    role="Orchestrator",
                    messages=list(messages),
                    temp=0.2,
                )
                content = _extract_text(response.content) if hasattr(response, "content") else str(response)
                clean_response = _extract_text_response(content)
                _log(f"[ASSISTANT] {clean_response}")
            except Exception:
                pass
            break

        # ── Context Compaction ──
        # The orchestrator's message list grows with subagent responses
        # (which can be 5000+ chars each), file listings, and thinking blocks.
        # Compaction prevents context overflow and keeps the KV cache warm.
        # Thresholds are high — 1M token window, don't compact prematurely.
        if iteration > 1:
            try:
                # Estimate total tokens in message history
                total_text = ""
                for m in messages:
                    if hasattr(m, "content"):
                        total_text += str(m.content)
                    elif isinstance(m, dict):
                        total_text += str(m.get("content", ""))
                total_tokens = estimate_tokens(total_text)

                # Calculate budget utilization
                static_prompt_len = len(static_system)
                keep_n = 12

                if total_tokens > 800_000:
                    _log(f"[ORCHESTRATOR] Compaction Tier 3 (tokens={total_tokens})")
                    if len(messages) > keep_n + 2:
                        messages = checkpoint_compact(
                            messages, tool_call_log,
                            created=[], modified=[],
                            keep_last_n=keep_n
                        )
                        _log(f"[ORCHESTRATOR] Compacted -> {len(messages)} messages remain")
                elif total_tokens > 600_000:
                    _log(f"[ORCHESTRATOR] Compaction Tier 2 (tokens={total_tokens})")
                    if len(messages) > keep_n + 2:
                        messages = tier2_compact(messages, keep_last_n=keep_n)
                        _log(f"[ORCHESTRATOR] Compacted -> {len(messages)} messages remain")
                elif total_tokens > 400_000:
                    _log(f"[ORCHESTRATOR] Compaction Tier 1 (tokens={total_tokens})")
                    if len(messages) > keep_n + 2:
                        messages = tier1_compact(messages, keep_last_n=keep_n)
                        _log(f"[ORCHESTRATOR] Compacted -> {len(messages)} messages remain")
            except Exception as e:
                _log(f"[ORCHESTRATOR] Compaction error (non-fatal): {e}")

        # ── Call LLM ──
        try:
            response = invoke_messages_with_fallback(
                role="Orchestrator",
                messages=list(messages),
                temp=0.2,
            )
        except Exception as e:
            import traceback
            tb = traceback.format_exc()
            _log(f"[ORCHESTRATOR] LLM call failed: {e}\n{tb}")
            break

        # Extract content
        content = ""
        reasoning_content = None
        if hasattr(response, "content"):
            from llm import _extract_text
            content = _extract_text(response.content)
        else:
            content = str(response)

        if hasattr(response, "additional_kwargs"):
            reasoning_content = response.additional_kwargs.get("reasoning_content")

        if reasoning_content:
            _log(f"[THINKING] Supervisor: {reasoning_content.strip()}")

        # ── Parse tool calls ──
        tool_calls = parse_all_tool_calls(content)

        if not tool_calls:
            # Agent is done --- no more tools to call
            messages.append(AIMessage(
                content=content,
                additional_kwargs={"reasoning_content": reasoning_content} if reasoning_content else {}
            ))
            clean_response = _extract_text_response(content)

            # ── Validate exit ──
            # Let the model finish naturally. Only nudge if response is empty or bare minimum.
            is_too_short = len(clean_response.strip()) < 20
            delegated_work = any(
                tc.get("tool") in ("task", "start_async_task")
                for tc in tool_call_log
            )

            if is_too_short and delegated_work and iteration < MAX_ITERATIONS:
                _log(f"[ORCHESTRATOR] Exit too short — nudging for proper response.")
                messages.append(HumanMessage(
                    content="[NUDGE] That response was too brief. Summarize what was accomplished and offer a next step the user might want.",
                    id=f"orch-exit-nudge-{iteration}-{uuid.uuid4()}"
                ))
                continue  # Back to LLM loop for corrected summary

            _log(f"\n[ORCHESTRATOR] Agent finished after {iteration} iterations")
            _log(f"[ASSISTANT] {clean_response}")
            break

        # ── Record AI response ──
        ai_msg_id = f"orch-ai-{iteration}-{uuid.uuid4()}"
        messages.append(AIMessage(
            content=content,
            id=ai_msg_id,
            additional_kwargs={"reasoning_content": reasoning_content} if reasoning_content else {}
        ))

        # ── Execute tools ──
        tool_results = []
        for tc in tool_calls:
            tool_name = tc.get("tool", "")
            args = tc.get("args", {})
            if not args or not isinstance(args, dict):
                args = {k: v for k, v in tc.items() if k not in ("tool", "args")}

            if tool_name not in valid_tools:
                tool_results.append(
                    f"[BLOCKED] Tool '{tool_name}' is not available to the Orchestrator. "
                    f"Available: {', '.join(valid_tools)}"
                )
                continue

            # Block write_file / edit_file explicitly
            if tool_name in ("write_file", "edit_file"):
                tool_results.append(
                    f"[BLOCKED] '{tool_name}' --- Orchestrator does not write code. "
                    f"Delegate to Dev subagent instead."
                )
                continue

            try:
                import json
                _log(f"[TOOL_CALL] {tool_name}({json.dumps(args)})")
                result = execute_tool(tool_name, args)
                result_str = str(result)
                # Truncate long results
                if len(result_str) > 8000:
                    result_str = result_str[:4000] + "\n... (truncated middle) ...\n" + result_str[-4000:]

                # Log tool output for frontend trace tracking
                _log(f"[TOOL OUTPUT] {tool_name}: {result_str}")
                tool_results.append(f"[{tool_name}] {result_str}")
                tool_call_log.append({"tool": tool_name, "args": args, "iteration": iteration})
            except Exception as e:
                tool_results.append(f"[{tool_name}] ERROR: {str(e)[:300]}")

        # ── Feed results back ──
        feedback = "\n\n".join(tool_results) if tool_results else "No tool results."
        human_msg_id = f"orch-human-{iteration}-{uuid.uuid4()}"
        messages.append(HumanMessage(content=f"Tool results:\n{feedback}", id=human_msg_id))

        # ── Progress audit ──
        if iteration > 4 and iteration % 4 == 1:
            no_delegation = all(
                t.get("tool") not in ("task", "start_async_task")
                for t in tool_call_log[-8:]
            )
            if no_delegation:
                _log("[ORCHESTRATOR] ⚠️ No delegation detected for several turns.")
                messages.append(HumanMessage(
                    content="[AUDITOR] You haven't called any subagent recently. "
                            "Delegate the work now: task(), start_async_task(), or wrap up if done.",
                    id=f"orch-audit-{iteration}-{uuid.uuid4()}"
                ))

    # ── Return ──
    from state_sync import safe_get_state
    new_msgs = [m for m in messages if getattr(m, "id", None) not in existing_ids]
    final_report = _extract_text_response(content) if content else ""
    final_error = ""
    if not final_report:
        final_error = "The orchestrator completed its delegation loop but returned no final response."
        _log(f"[ORCHESTRATOR ERROR] {final_error}")
    return {
        "messages": new_msgs,
        "agent_report": final_report,
        "code": final_report,
        "test_report": "",
        "project_path": project_path,
        "code_updated": any(tc.get("tool") in ("task", "start_async_task") for tc in tool_call_log),
        "tech_spec_updated": False,
        "next_agent": "finish",
        "remaining_steps": safe_get_state().get("remaining_steps", 40),
        "error": final_error,
    }
