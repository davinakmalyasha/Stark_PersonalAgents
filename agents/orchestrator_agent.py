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
from langchain_core.messages import SystemMessage, HumanMessage, AIMessage, ToolMessage
from sync.state_sync import shared_state, safe_append_live_log
from agents.it_department_nodes_base import ITState
from tools import (
    ToolExecutionResult,
    execute_tool_detailed,
    get_native_tools,
    get_native_tool_names,
)
from llm import invoke_messages_with_fallback_stream
from llm.loop_detector import LoopGuard
from memory.context_compaction import (
    compact_history,
    get_compaction_policy,
)
from inbox import drain_steering, drain_follow_up, consume_abort, render_delivery


def _orch_session_log(chat_id: str, project_path: str):
    """Get-or-create the Pi-style session JSONL for this chat (best-effort)."""
    try:
        from session_log import get_session
        ws = project_path or os.environ.get("HARNESS_ROOT") or os.path.dirname(os.path.abspath(__file__))
        return get_session(chat_id or "default", ws)
    except Exception:
        return None


# ═══════════════════════════════════════════════════════════════════════════════
# Zone 1: Frozen system prompt --- byte-identical across all calls
# ═══════════════════════════════════════════════════════════════════════════════

_ORCHESTRATOR_SYSTEM_TEMPLATE = """\
You are an Orchestrator — the central coordination agent. You classify ONE request, delegate it to the SINGLE most relevant subagent, and report the outcome to the user. You do NOT write code, edit files, or run arbitrary commands.

---
## THINKING GUIDE

Before your first action each turn:

1. **What's the request?** — classify the DOMAIN: bug-fix / coding-implementation / general / security-infra / spec-analysis / other
2. **Check the request context** — what is the user asking for, and which domain owns it?
3. **Complexity pre-check** — is this ONE cohesive work item? Almost always YES. Spawning more than one subagent is ONLY for genuinely large, multi-domain tasks with clearly separable work items (e.g. "build the full store: backend + frontend + security hardening").
4. **Pick ONE subagent** from the routing table — the single best fit. Delegate it. Then wait.
5. **When it returns**: TRUST its outcome and give the user a concise report.

**YOU NEVER RE-DELEGATE AND YOU NEVER DOUBLE-DELEGATE.**
- The harness runs its OWN independent verification after the subagent completes; gate failures route back to Dev as repair turns (with the gate report + task journal). You do NOT re-delegate on failures and you do NOT spawn a second agent to "double-check" Dev's work.
- A returned subagent is FINAL. If it reports failure, include that honestly in your report — the next step belongs to the harness gate or the user.

That's it. Your only job: understand → classify → delegate ONE subagent → report.

---
## OODA LOOP (one pass — Observe → Orient → Decide → Act → Observe → report)
- **Observe**: read the request and the supplied runtime context.
- **Orient**: classify the domain; brief thinking here is enough.
- **Decide**: pick the ONE subagent from the routing table.
- **Act**: delegate via `task(...)` with a complete task description.
- **Observe the result** (the handoff), then give the user the concise report.

---
## WORK IN PHASES (one pass, three beats)
- **Phase A — CLASSIFY**: read the request, pick the ONE subagent
  from the routing table. One brief decision.
- **Phase B — DELEGATE**: call `task(...)` with a complete, clear task
  description. Then wait for the result.
- **Phase C — REPORT**: give the user the concise report. Do NOT re-analyze, do NOT re-delegate,
  do NOT add ceremony turns — finish in the fewest turns possible.

---
## ROUTING TABLE (EXCLUSIVE — pick exactly ONE)

| Request domain | Subagent | What It Does |
|----------|-------|--------------|
| **Bug-fix / error / wrong behavior / "tests failing" / verify / reproduce** | **Dev** | Dev OWNS the full loop: diagnoses the failure (local tests or the CI gate report via gh api through run_command), finds the root cause, FIXES the code, writes/updates tests, and re-runs verification until it passes. Reports DONE only when verified. Does NOT audit security or performance — the CI security gate owns those. |
| **Coding / implementation / feature / new code** | **Dev** | Writes clean code WITH tests (features touching API/db/validation MUST ship tests), runs a syntax self-check, then STOPS — the CI gate verifies automatically and failures route back to Dev (repair turn) until green. Never delegates to other agents. Does NOT design architecture, review security, or optimize performance. |
| **General chat / research / quick question / vision** | **FreeAgent** | General-purpose free model agent (OpenRouter, Groq, Google AI Studio, Mistral, NVIDIA NIM, Cloudflare, Z.ai, OpenCode Go, Kilo & Cline) — text chat, image analysis, quick research, lightweight tasks. Zero cost. |
| **New project creation** ("make a new project called X…", "set up a project") | **You** — call `create_project(name, concept)` | Creates the folder under D:\\MyProject, registers the workspace + a chat, and assigns a durable Dev agent that starts on the concept immediately. Do NOT delegate to a subagent; do NOT call this for work on an existing project. |
| **Security / infrastructure / performance** — ONLY when the request EXPLICITLY asks | **Dev** | Security & performance are enforced by the CI gate (Bandit, pip-audit, gitleaks, Ruff, perf-smoke, CodeQL) on every push with branch protection blocking merges until green. For explicit security/infra requests: Dev implements, pushes, reads the gate report via gh api through run_command, and fixes until the gate passes. |

*(future domains plug in as new registry entries + one row here.)*

## DELEGATION RULES

- **ONE request → ONE subagent.** Pick the single best fit and delegate ONLY it. Every coding/bug/verify request goes to Dev — there is no separate fixer agent. Do NOT spawn Dev AND anything else for the same work item — that is duplication and wastes the team's budget.
- **Multi-spawn is reserved for genuinely large, multi-domain requests** where the work items are clearly separable (backend + frontend + infrastructure, or a feature + an explicit security review). Each distinct work item gets exactly one subagent, each delegated ONCE.
- **NEVER re-delegate a subtask that already returned.** No retries, no "try again" handoffs. Failures flow to the harness verification gate, and gate failures route back to Dev as repair turns (with the gate report + task journal as context).
- For HIGH-RISK change (data schema/migrations, auth/security boundaries, payment/finance logic, large multi-file blast radius), instruct Dev to verify more aggressively (run the tests, check the gate).
- Dev's handoff is the standard AGENT HANDOFF REPORT — expect a compact report.

## PROJECT-AWARE ROUTING (P9)

Projects registered in this harness each have a DURABLE Dev agent (a continuable child). Follow these rules exactly:

- **Existing project → continue its Dev agent.** Coding, bug-fix, or follow-up work for a project that exists: call `resume_project(name, message)` — it resolves the project and delivers the message to its durable Dev in one call. The project's Dev resumes its OWN durable session — full context, zero re-spawning. NEVER use `task("Dev")` for a project that already has a Dev agent.
- **New project → create it with `create_project(name, concept)`.** The tool makes the folder, workspace, chat, and a fresh durable Dev agent in one call. Only call it when the user genuinely asks for a NEW project.
- **No Dev agent yet for an existing folder?** `resume_project` returns [NOT FOUND] → then create the project with `create_project(name, concept)` to assign its Dev agent.
- **Multiple new projects** ("two projects: A and B") → call `create_project` once per project (they can run in the same turn).
- The harness pre-router already sends most in-project and name-mention messages straight to the project's Dev agent before you ever see them; when you DO see project work, it was ambiguous enough to need your call — make it with the tools above.

## REPORTING (concise, Claude-Code style)

Your final answer to the user is a TIGHT summary — never a wall of text:

✓ Done — <one line: the outcome>
- What was done: <2-5 short bullets, mention files touched>
- Verification: <what the subagent ran / harness gate status>
- Next: <only if something genuinely remains>

Rules: ≤15 lines total. No raw dumps, no repeated context, no "Here is the summary of..." preamble. If the outcome was a failure or partial, say so plainly in the first line (⚠ Partial / ✗ Failed + one-line why).

## COST AWARENESS

- **Dev** is the workhorse (cheap flash): implementation, bug-fix, verify, and gate-repair all route to it; its adaptive reasoning ladder (iter-1 high, routine disabled, trouble → high) handles deep diagnosis. **FreeAgent** is $0 for general work. The CI gate (github-ci-security) verifies every push and routes failures back to Dev as repair turns.
- One well-written delegation prompt is cheaper than two vague ones — invest clarity upfront.
- Parallel async tasks (`async_task(action="start")`) reduce wall-clock time but not token cost. Use when SPEED matters more than cost. Check with `async_task(action="status")` and list with `async_task(action="list")`.

## WEB RESEARCH

Research goes through your subagents: FreeAgent has run_command (curl) + browser tools (zero cost); Dev has browser tools for JS-heavy pages. Delegate research to the right one — you do not fetch directly.

## SUBAGENT DEPTH

The system enforces a maximum nesting depth of 3 (orchestrator → subagent → sub-subagent). Subagents are terminal nodes — they do NOT spawn subagents. Only you delegate. You do not need to manage sub-subagents.

## CONTINUABLE SUBAGENTS (P6)

For long-running or iterative work, use the unified async tool:

- `async_task(action="start", name, task)` — fire-and-forget; returns task_id and, when applicable, a continuable child id.
- `async_task(action="status", task_id, wait_seconds?)` — poll one task; wait optionally.
- `async_task(action="list")` — list all async tasks and their statuses.
- For existing projects, prefer `resume_project(name, message)` — it resumes the durable Dev in one call without needing a child_id.

Use continuable children when the work spans several user requests or when you need to steer an ongoing effort. Prefer one-shot `task()` for single delegations.

"""


# ═══════════════════════════════════════════════════════════════════════════════
# Orchestrator Agent Node
# ═══════════════════════════════════════════════════════════════════════════════

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
      2. Spawns subagents via task() / async_task(action="start") / resume_project()
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
    project_path = s.get("project_path", "") or os.environ.get("HARNESS_ROOT") or os.path.dirname(os.path.abspath(__file__))
    chat_id = s.get("chat_id", "")

    shared_state["thoughts"]["orchestrator"] = "Analyzing request..."

    # ── Tools the orchestrator is allowed to use ──
    valid_tools = list(get_native_tool_names("orchestrator"))
    native_orchestrator_tools = get_native_tools("orchestrator")

    # ── P4: registry scope — one visibility for the orchestrator ──
    _scope_token = None
    try:
        from tools import active_tool_scope, get_registry
        get_registry().restrict_scope("Orchestrator", allow=valid_tools)
        _scope_token = active_tool_scope.set("Orchestrator")
    except Exception:
        pass

    # ── Zone 1: Frozen system prompt (never changes → cached) ──
    static_system = _build_orchestrator_prompt()

    # ── Zone 2: ALL dynamic context (cache break point) ──
    task_message = ""

    # Workspace info (changes per project)
    if project_path and os.path.isdir(project_path):
        task_message += f"## WORKSPACE\n{project_path}\n\n"
    # Chat history for multi-turn context
    chat_history_str = ""
    if project_path and chat_id:
        try:
            from memory.chat_context import build_chat_context
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

    # Vision: inject user-attached images (from the API stash) before the first
    # LLM call so they are visible from turn one.
    try:
        from tools.vision_channel import drain_attachment_images
        if drain_attachment_images(messages):
            _log("[VISION] Orchestrator: user attachment image(s) injected")
    except Exception:
        pass

    # ── Session log (P1): record this run's user turn, best-effort ──
    orch_sess = _orch_session_log(chat_id, project_path)
    if orch_sess is not None:
        try:
            orch_sess.append_message("user", client_req, source="user")
        except Exception:
            pass

    content = ""  # Initialize --- set during loop
    existing_ids = {m.id for m in messages if getattr(m, "id", None)}
    iteration = 0
    tool_call_log: list[dict] = []
    orch_think = False  # adaptive ladder: set by trouble signals, consumed next turn
    compaction_policy = get_compaction_policy("orchestrator")

    _log(f"\n{'='*60}\n[ORCHESTRATOR] Starting delegation loop\n{'='*60}")

    while True:
        iteration += 1
        _log(f"\n[ORCH Iteration {iteration}]")

        # ── Inbox (P2): a user abort takes effect at the next step boundary ──
        if consume_abort(chat_id):
            safe_append_live_log("\n[CANCELLED] Run cancelled by user request.")
            _log("[ORCHESTRATOR] Aborted by user request.")
            break

        # Append below the shared threshold; at the threshold the same single
        # checkpoint operation used by Dev compacts the replaceable transcript.
        if iteration > 1:
            try:
                messages, compaction_info = compact_history(
                    messages,
                    tool_call_log=tool_call_log,
                    policy=compaction_policy,
                    chat_id=chat_id,
                )
                if compaction_info.get("checkpoint"):
                    _log(
                        f"[ORCHESTRATOR] Single {compaction_info.get('trigger')} checkpoint "
                        f"({compaction_info['before']:,} -> {compaction_info['after']:,} est. tokens; "
                        f"tail={compaction_policy.tail_budget_tokens:,})"
                    )
                    orch_think = True  # rebuilt context -> one careful re-orientation turn
            except Exception as e:
                _log(f"[ORCHESTRATOR] Compaction error (non-fatal): {e}")

        # ── Call LLM ──
        try:
            # ── Adaptive reasoning budget ladder ──
            # Iteration 1 is a mechanical route-and-delegate decision
            # (thinking disabled — the routing table is in the prompt).
            # Routine follow-up turns run with thinking DISABLED too — delegation
            # handoffs and verification are mechanical. Any trouble signal from
            # the previous turn (delegation/tool error, failure markers in a
            # subagent report, or compaction) flips THIS turn back to high; the flag is
            # consumed (one high turn, then back to routine).
            # Knobs: REASONING_ITER1 (first-call depth, default "disabled"),
            #        REASONING_ORCH_ROUTINE (routine depth, default "disabled").
            if orch_think:
                orch_effort = "high"
                orch_think = False
            elif iteration <= 1:
                orch_effort = os.environ.get("ORCH_REASONING_ITER1",
                                             os.environ.get("REASONING_ITER1", "disabled"))
            else:
                orch_effort = os.environ.get("REASONING_ORCH_ROUTINE", "disabled")
            response = invoke_messages_with_fallback_stream(
                role="Orchestrator",
                messages=list(messages),
                temp=0.2,
                tools=native_orchestrator_tools,
                reasoning_effort=orch_effort,
                where=f"Orchestrator iter {iteration}",
            )
        except Exception as e:
            import traceback
            tb = traceback.format_exc()
            _log(f"[ORCHESTRATOR] LLM call failed: {e}\n{tb}")
            break

        # Extract content and native API tool calls. Text/XML parsing is
        # intentionally absent; the coordinator uses the same structured tool
        # protocol as the Developer.
        content = ""
        native_tool_calls = []
        reasoning_content = None
        if isinstance(response, tuple):
            if len(response) >= 3:
                content, native_tool_calls, reasoning_content = response[:3]
            elif len(response) == 2:
                content, native_tool_calls = response
            else:
                content = response[0] if response else ""
        elif hasattr(response, "content"):
            from llm import _extract_text
            content = _extract_text(response.content)
        else:
            content = str(response)

        if hasattr(response, "additional_kwargs"):
            reasoning_content = response.additional_kwargs.get("reasoning_content")

        if reasoning_content:
            _log(f"[THINKING] Supervisor: {reasoning_content.strip()}")

        tool_calls = []
        for call_index, tc in enumerate(native_tool_calls or []):
            tname = tc.get("tool") or tc.get("name") if isinstance(tc, dict) else ""
            targs = tc.get("args") if isinstance(tc, dict) else {}
            if isinstance(targs, str):
                try:
                    targs = json.loads(targs)
                except Exception:
                    targs = {}
            if tname:
                tool_calls.append({
                    "tool": tname,
                    "args": targs if isinstance(targs, dict) else {},
                    "id": (tc.get("id") if isinstance(tc, dict) else None)
                    or f"orch-call-{iteration}-{call_index}-{uuid.uuid4().hex[:8]}",
                })

        if not tool_calls:
            # Agent is done --- no more tools to call
            # No tool call this turn -> reasoning_content is ignored by the API
            # (DeepSeek docs) and does not need to participate in concatenation,
            # so do NOT carry it forward as new cache-miss input.
            messages.append(AIMessage(content=content))
            if orch_sess is not None:
                try:
                    orch_sess.append_assistant(content, stop_reason="stop")
                except Exception:
                    pass
            clean_response = _extract_text_response(content)

            # ── Validate exit ──
            # Let the model finish naturally. Only nudge if response is empty or bare minimum.
            is_too_short = len(clean_response.strip()) < 20
            delegated_work = any(
                tc.get("tool") in ("task", "async_task", "resume_project")
                for tc in tool_call_log
            )

            if is_too_short and delegated_work:
                _log(f"[ORCHESTRATOR] Exit too short — nudging for proper response.")
                messages.append(HumanMessage(
                    content="[NUDGE] That response was too brief. Summarize what was accomplished and offer a next step the user might want.",
                    id=f"orch-exit-nudge-{iteration}-{uuid.uuid4()}"
                ))
                continue  # Back to LLM loop for corrected summary

            # ── Inbox (P2): follow-up work queued while running → keep going ──
            follow_msgs = drain_follow_up(chat_id)
            if follow_msgs:
                _log(f"[ORCHESTRATOR] Follow-up queued ({len(follow_msgs)}) — continuing")
                messages.append(HumanMessage(
                    content=render_delivery("FOLLOW-UP", follow_msgs),
                    id=f"orch-followup-{iteration}-{uuid.uuid4()}",
                ))
                continue

            _log(f"\n[ORCHESTRATOR] Agent finished after {iteration} iterations")
            _log(f"[ASSISTANT] {clean_response}")
            break

        # ── Record AI response ──
        ai_msg_id = f"orch-ai-{iteration}-{uuid.uuid4()}"
        native_history_calls = [
            {
                "name": tc.get("tool", ""),
                "args": tc.get("args") or {},
                "id": tc.get("id") or f"orch-call-{iteration}-{index}",
                "type": "tool_call",
            }
            for index, tc in enumerate(tool_calls)
            if tc.get("tool")
        ]
        messages.append(AIMessage(
            content=content,
            id=ai_msg_id,
            tool_calls=native_history_calls,
            additional_kwargs={"reasoning_content": reasoning_content} if reasoning_content else {}
        ))

        # ── Session log (P1): record the step's assistant message ──
        if orch_sess is not None:
            try:
                blocks = [{"type": "text", "text": content}]
                if reasoning_content:
                    blocks.insert(0, {"type": "thinking", "thinking": reasoning_content})
                orch_sess.append_assistant(blocks, stop_reason="toolUse" if tool_calls else "stop")
            except Exception:
                pass

        # ── Execute tools ──
        # Keep the provider's native assistant -> tool-result protocol intact.
        # A synthetic HumanMessage containing "Tool results:" breaks call-id
        # correlation and makes the next model turn reinterpret observations
        # as user instructions.
        tool_results = []
        native_results = []
        for tc in tool_calls:
            tool_name = tc.get("tool", "")
            args = tc.get("args", {})
            if not args or not isinstance(args, dict):
                args = {k: v for k, v in tc.items() if k not in ("tool", "args")}
            call_id = tc.get("id") or f"orch-call-{iteration}-{len(native_results)}"
            execution = None

            if tool_name not in valid_tools:
                result_str = (
                    f"[BLOCKED] Tool '{tool_name}' is not available to the Orchestrator. "
                    f"Available: {', '.join(valid_tools)}"
                )
                execution = ToolExecutionResult.from_content(tool_name, args, result_str)
            else:
                try:
                    import json
                    _log(f"[TOOL_CALL] {tool_name}({json.dumps(args)})")
                    execution = execute_tool_detailed(tool_name, args, scope="Orchestrator")
                    result_str = str(execution.content)
                except Exception as e:
                    result_str = f"ERROR: {str(e)[:300]}"
                    orch_think = True  # tool blew up — next turn must diagnose

            loop_warning = LoopGuard.check_pre_execute(tool_call_log, tool_name, args)
            # Truncate long results (teaching note so the model stops producing
            # giant outputs it will only get clipped).
            if len(result_str) > 4000:
                orig_len = len(result_str)
                result_str = result_str[:2000] + "\n... (truncated middle) ...\n" + result_str[-2000:]
                try:
                    from memory.context_compaction import truncation_note
                    result_str += truncation_note(orig_len, 4000)
                except Exception:
                    pass
            if loop_warning and loop_warning[0] == "WARNING":
                result_str += "\n" + loop_warning[1]

            native_results.append((call_id, tool_name, result_str))
            tool_results.append(f"[{tool_name}] {result_str}")

            # Session log (P1): record the same call id used by the native turn.
            if orch_sess is not None:
                try:
                    orch_sess.append_tool_result(
                        result_str,
                        tool_call_id=call_id,
                        tool_name=tool_name,
                        is_error=bool(execution and execution.is_error),
                    )
                except Exception:
                    pass

            # Adaptive-ladder trigger: a subagent reported FAILURE (handoff
            # status, not stray words) → the NEXT orchestrator turn diagnoses
            # with high effort.
            low = result_str.lower()
            if tool_name in ("task", "async_task", "resume_project"):
                is_failure = bool(execution and execution.is_error) or any(
                    marker in low
                    for marker in (
                        "handoff report [failed]", "handoff report [blocked]",
                        "handoff report [stuck]", "[failed]", "[blocked]",
                        "timed out",
                    )
                )
                if is_failure:
                    orch_think = True

            # Log tool output for frontend trace tracking and append the compact
            # execution record used by the loop guard/checkpoint.
            _log(f"[TOOL OUTPUT] {tool_name}: {result_str}")
            tool_call_log.append({
                "tool": tool_name,
                "args": args,
                "iteration": iteration,
                "status": "error" if (execution and execution.is_error) else "ok",
                "exit_code": execution.exit_code if execution else None,
                "timed_out": execution.timed_out if execution else False,
                "offloaded": execution.offloaded if execution else False,
            })

        # One native ToolMessage per native call. Never merge observations into
        # a synthetic user turn; the provider needs the call ids to associate
        # each result with the assistant tool call.
        for call_id, tool_name, result_str in native_results:
            messages.append(ToolMessage(
                content=result_str,
                tool_call_id=call_id,
                name=tool_name or None,
            ))

        # Vision: drain any inline images queued by tools into ONE user message.
        try:
            from tools.vision_channel import drain_into_messages
            if drain_into_messages(messages, header="[Inline image(s) from tool result]"):
                _log(f"[VISION] Orchestrator iter {iteration}: inline image(s) injected")
        except Exception:
            pass

        # ── Inbox (P2): steering lands at the next step boundary (after this
        # step's tool calls) — the model sees it on the next iteration ──
        steer_msgs = drain_steering(chat_id)
        if steer_msgs:
            _log(f"[ORCHESTRATOR] Steering queued ({len(steer_msgs)}) — injecting")
            messages.append(HumanMessage(
                content=render_delivery("STEERING", steer_msgs),
                id=f"orch-steer-{iteration}-{uuid.uuid4()}",
            ))

    # ── Return ──
    if _scope_token is not None:
        try:
            from tools import active_tool_scope
            active_tool_scope.reset(_scope_token)
        except Exception:
            pass
    new_msgs = [m for m in messages if getattr(m, "id", None) not in existing_ids]
    final_report = _extract_text_response(content) if content else ""
    final_error = ""
    if not final_report:
        final_error = "The orchestrator completed its delegation loop but returned no final response."
        _log(f"[ORCHESTRATOR ERROR] {final_error}")

    # Session transcripts remain in the session store only. No project feature,
    # lesson, greeting, or "last session" files are generated here.

    return {
        "messages": new_msgs,
        "agent_report": final_report,
        "code": final_report,
        "test_report": "",
        "project_path": project_path,
        "code_updated": any(tc.get("tool") in ("task", "async_task", "resume_project") for tc in tool_call_log),
        "tech_spec_updated": False,
        "next_agent": "finish",
        "error": final_error,
    }
