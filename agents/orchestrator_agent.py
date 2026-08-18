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
from sync.state_sync import shared_state, safe_append_live_log
from agents.it_department_nodes_base import ITState
from tools import execute_tool, TOOL_DEFINITIONS
from llm import invoke_messages_with_fallback, invoke_messages_with_fallback_stream
from agents.developer_agent import _parse_tool_call, parse_all_tool_calls
from memory.context_budget import estimate_tokens
from memory.context_compaction import (
    tier1_compact,
    tier2_compact,
    checkpoint_compact,
    prune_stale_tool_outputs
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
2. **Check memory & context** — what do I already know from PROJECT MEMORY and chat history?
3. **Complexity pre-check** — is this ONE cohesive work item? Almost always YES. Spawning more than one subagent is ONLY for genuinely large, multi-domain tasks with clearly separable work items (e.g. "build the full store: backend + frontend + security hardening").
4. **Pick ONE subagent** from the routing table — the single best fit. Delegate it. Then wait.
5. **When it returns**: TRUST its outcome, run `update_feature()` / `update_project_meta()` to record progress, and give the user a concise report.

**YOU NEVER RE-DELEGATE AND YOU NEVER DOUBLE-DELEGATE.**
- The harness runs its OWN independent verification after the subagent completes; gate failures route back to Dev as repair turns (with the gate report + task journal). You do NOT re-delegate on failures and you do NOT spawn a second agent to "double-check" Dev's work.
- A returned subagent is FINAL. If it reports failure, include that honestly in your report — the next step belongs to the harness gate or the user.

That's it. Your only job: understand → classify → delegate ONE subagent → report.

---
## OODA LOOP (one pass — Observe → Orient → Decide → Act → Observe → report)
- **Observe**: read the request and PROJECT MEMORY / chat history.
- **Orient**: classify the domain; brief thinking here is enough.
- **Decide**: pick the ONE subagent from the routing table.
- **Act**: delegate via `task(...)` with a complete task description.
- **Observe the result** (the handoff), then Act once more: update memory and
  give the user the concise report. That closes the loop.

---
## WORK IN PHASES (one pass, three beats)
- **Phase A — CLASSIFY**: read the request, check memory, pick the ONE subagent
  from the routing table. One brief decision.
- **Phase B — DELEGATE**: call `task(...)` with a complete, clear task
  description. Then wait for the result.
- **Phase C — REPORT**: update memory (`update_feature` / `update_project_meta`)
  and give the user the concise report. Do NOT re-analyze, do NOT re-delegate,
  do NOT add ceremony turns — finish in the fewest turns possible.

---
## ROUTING TABLE (EXCLUSIVE — pick exactly ONE)

| Request domain | Subagent | What It Does |
|----------|-------|--------------|
| **Bug-fix / error / wrong behavior / "tests failing" / verify / reproduce** | **Dev** | Dev OWNS the full loop: diagnoses the failure (local tests or the CI gate report via gh api through run_command), finds the root cause, FIXES the code, writes/updates tests, and re-runs verification until it passes. Reports DONE only when verified. Does NOT audit security or performance — the CI security gate owns those. |
| **Coding / implementation / feature / new code** | **Dev** | Writes clean code WITH tests (features touching API/db/validation MUST ship tests), runs a syntax self-check, then STOPS — the CI gate verifies automatically and failures route back to Dev (repair turn) until green. Never delegates to other agents. Does NOT design architecture, review security, or optimize performance. |
| **General chat / research / quick question / vision** | **FreeAgent** | General-purpose free model agent (OpenRouter, Groq, Google AI Studio, Mistral, NVIDIA NIM, Cloudflare, Z.ai, OpenCode Zen, Kilo & Cline) — text chat, image analysis, quick research, lightweight tasks. Zero cost. |
| **New project creation** ("make a new project called X…", "set up a project") | **You** — call `create_project(name, concept)` | Creates the folder under D:\\MyProject, registers the workspace + a chat, and assigns a durable Dev agent that starts on the concept immediately. Do NOT delegate to a subagent; do NOT call this for work on an existing project. |
| **Security / infrastructure / performance** — ONLY when the request EXPLICITLY asks | **Dev** | Security & performance are enforced by the CI gate (Bandit, pip-audit, gitleaks, Ruff, perf-smoke, CodeQL) on every push with branch protection blocking merges until green. For explicit security/infra requests: Dev implements, pushes, reads the gate report via gh api through run_command, and fixes until the gate passes. |

*(future domains plug in as new registry entries + one row here.)*

## DELEGATION RULES

- **ONE request → ONE subagent.** Pick the single best fit and delegate ONLY it. Every coding/bug/verify request goes to Dev — there is no separate fixer agent. Do NOT spawn Dev AND anything else for the same work item — that is duplication and wastes the team's budget.
- **Multi-spawn is reserved for genuinely large, multi-domain requests** where the work items are clearly separable (backend + frontend + infrastructure, or a feature + an explicit security review). Each distinct work item gets exactly one subagent, each delegated ONCE.
- **NEVER re-delegate a subtask that already returned.** No retries, no "try again" handoffs. Failures flow to the harness verification gate, and gate failures route back to Dev as repair turns (with the gate report + task journal as context).
- For HIGH-RISK change (data schema/migrations, auth/security boundaries, payment/finance logic, large multi-file blast radius), instruct Dev to verify more aggressively (run the tests, check the gate).
- Dev's handoff is the standard AGENT HANDOFF REPORT — expect a compact report, and tell Dev to write its task journal (.agent-memory/task-journal-<taskid>.md) so repair turns start with full intent.

## PROJECT-AWARE ROUTING (P9)

Projects registered in this harness each have a DURABLE Dev agent (a continuable child). Follow these rules exactly:

- **Existing project → continue its Dev agent.** Coding, bug-fix, or follow-up work for a project that exists: call `find_project_dev(name)` → then `send_message(child_id, message)`. The project's Dev resumes its OWN durable session — full context, zero re-spawning. NEVER use `task("Dev")` for a project that already has a Dev agent.
- **New project → create it with `create_project(name, concept)`.** The tool makes the folder, workspace, chat, and a fresh durable Dev agent in one call. Only call it when the user genuinely asks for a NEW project.
- **No Dev agent yet for an existing folder?** `find_project_dev` returns NOT FOUND → then create the project with `create_project(name, concept)` to assign its Dev agent.
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
- Parallel async tasks (`start_async_task`) reduce wall-clock time but not token cost. Use when SPEED matters more than cost.

## WEB RESEARCH

Research goes through your subagents: FreeAgent has run_command (curl) + browser tools (zero cost); Dev has browser tools for JS-heavy pages. Delegate research to the right one — you do not fetch directly.

## SUBAGENT DEPTH

The system enforces a maximum nesting depth of 3 (orchestrator → subagent → sub-subagent). Subagents are terminal nodes — they do NOT spawn subagents. Only you delegate. You do not need to manage sub-subagents.

## CONTINUABLE SUBAGENTS (P6)

For long-running or iterative work, you can start a continuable subagent and keep talking to it across turns:

- `start_async_task` with a general-purpose agent creates a **continuable child** when the delegation returns a child id; otherwise it is one-shot.
- `list_agents()` — see your continuable children and their status (running / idle).
- `send_message(child_id, message)` — deliver a follow-up turn; an idle child resumes from its durable session, a running child queues the message.
- `interrupt_agent(child_id)` — ask a running child to stop at its next step.
- Children can `report()` back to you (wakes you via a follow-up) — treat reports as progress updates and respond or continue as needed.

Use continuable children when the work spans several user requests or when you need to steer an ongoing effort. Prefer one-shot `task()` for single delegations.

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


def _private_requery(messages: list, failed_content: str, malformed: list) -> tuple:
    """
    A2: private re-query for malformed tool calls (SWE-agent requery pattern).

    Re-queries the LLM with the error template WITHOUT appending the failed
    turn to history — mistakes live in the log/trajectory only, never in the
    conversation. Returns (content, tool_calls) on recovery, None if still bad.
    """
    error_template = (
        f"Your tool call `{malformed[0].get('tool')}` had NO arguments, so it could not "
        "be executed. Re-issue it with all required arguments in this exact format:\n"
        '```tool\n{"tool": "tool_name", "args": {"param": "value"}}\n```\n'
        "For `task`, you must include `name` and `task`. Fix the call and try again."
    )
    try:
        from llm import _extract_text
        resp = invoke_messages_with_fallback(
            role="Orchestrator",
            messages=list(messages) + [
                AIMessage(content=failed_content),
                HumanMessage(content=error_template),
            ],
            temp=0.2,
            reasoning_effort="high",
        )
        content = _extract_text(resp.content) if hasattr(resp, "content") else str(resp)
        calls = parse_all_tool_calls(content)
        if not calls:
            _log_requery_classification("format: no tool calls after private re-query")
            return None
        if any(tc.get("tool") and not tc.get("args", {}) for tc in calls):
            _log_requery_classification("format: recovered call still missing args")
            return None
        reasoning = getattr(resp, "additional_kwargs", {}).get("reasoning_content")
        return content, calls, reasoning
    except Exception:
        _log_requery_classification("exception during private re-query")
        return None


def _log_requery_classification(reason: str) -> None:
    """B8: log the error-taxonomy classification + handler policy for requery failures."""
    try:
        from agents.agent_errors import classify_tool_error, handler_policy
        err_cls = classify_tool_error(reason)
        _log(f"[ORCHESTRATOR] Requery failed ({reason}) — taxonomy: {err_cls.__name__}, policy: {handler_policy(err_cls)}")
    except Exception:
        pass


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
    project_path = s.get("project_path", "") or os.environ.get("HARNESS_ROOT") or os.path.dirname(os.path.abspath(__file__))
    chat_id = s.get("chat_id", "")

    shared_state["thoughts"]["orchestrator"] = "Analyzing request..."

    # ── Tools the orchestrator is allowed to use ──
    valid_tools = [
        "task", "start_async_task", "check_async_task", "list_async_tasks",
        "update_feature", "update_project_meta",
        "ask_user",
        "send_message", "list_agents", "interrupt_agent",
        "create_project", "find_project_dev",
    ]

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
                from memory.memory_manager import ensure_memory
                memory_content = ensure_memory(project_path)
                if memory_content:
                    task_message += f"## PROJECT MEMORY\n{memory_content}\n\n"
        except Exception as e:
            _log(f"[ORCHESTRATOR] Memory load error (non-fatal): {e}")

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
    orch_malformed_nudges = 0
    orch_think = False  # adaptive ladder: set by trouble signals, consumed next turn
    escalation_counts: dict[str, int] = {}  # delegations per task key
    escalated_keys: set[str] = set()
    _hard_stop_key: str | None = None  # set when a task key is re-delegated / ceiling hit
    _session_delegations = 0  # total delegations this session (rail: ORCH_MAX_DELEGATIONS)

    _log(f"\n{'='*60}\n[ORCHESTRATOR] Starting delegation loop\n{'='*60}")

    while True:
        iteration += 1
        _log(f"\n[ORCH Iteration {iteration}]")

        # ── Inbox (P2): a user abort takes effect at the next step boundary ──
        if consume_abort(chat_id):
            safe_append_live_log("\n[CANCELLED] Run cancelled by user request.")
            _log("[ORCHESTRATOR] Aborted by user request.")
            break

        # ── Safety ceiling (should never be reached — LLM decides when done) ──
        if iteration > MAX_ITERATIONS:
            _log(f"[ORCHESTRATOR] Safety ceiling ({MAX_ITERATIONS}) reached — forcing structured exit.")
            # A4: salvage partial work — capture workspace delta into the exit prompt
            salvage = ""
            try:
                from tools import capture_workspace_delta
                salvage = capture_workspace_delta(max_chars=4000)
            except Exception:
                pass
            salvage_block = f"\n\n{salvage}\n" if salvage else "\n"
            messages.append(HumanMessage(
                content="[HARD STOP] You have exceeded the safety iteration ceiling. "
                        "Output your structured exit summary NOW (## Summary, ## Files, ## Verification). "
                        "No tools — just the summary."
                        + salvage_block,
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
                # B5/B6: token-budgeted tail + incremental tier-0 prune (COMPACT_V2)
                compact_v2 = os.environ.get("COMPACT_V2", "1") == "1"
                tail_budget = int(os.environ.get("TAIL_BUDGET_TOKENS", "100000")) if compact_v2 else None

                # Tier-0: cheap incremental prune before any full-tier rewrite
                if compact_v2 and total_tokens > 600_000:
                    messages, pruned_chars = prune_stale_tool_outputs(messages)
                    if pruned_chars:
                        _log(f"[ORCHESTRATOR] Tier-0 prune freed {pruned_chars:,} chars of old tool outputs")
                        total_text = ""
                        for m in messages:
                            if hasattr(m, "content"):
                                total_text += str(m.content)
                            elif isinstance(m, dict):
                                total_text += str(m.get("content", ""))
                        total_tokens = estimate_tokens(total_text)

                if total_tokens > 800_000:
                    _log(f"[ORCHESTRATOR] Compaction Tier 3 (tokens={total_tokens})")
                    if len(messages) > keep_n + 2:
                        messages = checkpoint_compact(
                            messages, tool_call_log,
                            created=[], modified=[],
                            keep_last_n=keep_n,
                            tail_budget_tokens=tail_budget
                        )
                        _log(f"[ORCHESTRATOR] Compacted -> {len(messages)} messages remain")
                        orch_think = True  # rebuilt context → one careful re-orientation turn
                elif total_tokens > 600_000:
                    _log(f"[ORCHESTRATOR] Compaction Tier 2 (tokens={total_tokens})")
                    if len(messages) > keep_n + 2:
                        messages = tier2_compact(messages, keep_last_n=keep_n, tail_budget_tokens=tail_budget)
                        _log(f"[ORCHESTRATOR] Compacted -> {len(messages)} messages remain")
                        orch_think = True
                    # P3: Pi-style session-log checkpoint (summary entry +
                    # retainedTail), best-effort and zero impact on the loop
                    if orch_sess is not None:
                        try:
                            from memory.context_compaction import compact_session_log
                            _log_summary = compact_session_log(orch_sess)
                            if _log_summary:
                                _log(f"[ORCHESTRATOR] Session log checkpointed ({len(_log_summary)} chars summary)")
                        except Exception as e:
                            _log(f"[ORCHESTRATOR] Session log compaction error (non-fatal): {e}")
                elif total_tokens > 400_000:
                    _log(f"[ORCHESTRATOR] Compaction Tier 1 (tokens={total_tokens})")
                    if len(messages) > keep_n + 2:
                        messages = tier1_compact(messages, keep_last_n=keep_n, tail_budget_tokens=tail_budget)
                        _log(f"[ORCHESTRATOR] Compacted -> {len(messages)} messages remain")
                        orch_think = True
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
            # subagent report, malformed tool call, compaction, no-progress
            # audit) flips THIS turn back to high for diagnosis; the flag is
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
                reasoning_effort=orch_effort,
                where=f"Orchestrator iter {iteration}",
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

            # ── Inbox (P2): follow-up work queued while running → keep going ──
            follow_msgs = drain_follow_up(chat_id)
            if follow_msgs and iteration < MAX_ITERATIONS:
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
        messages.append(AIMessage(
            content=content,
            id=ai_msg_id,
            additional_kwargs={"reasoning_content": reasoning_content} if reasoning_content else {}
        ))

        # ── Guard: malformed tool calls (no usable args) — re-query instead of executing doomed calls ──
        # The flash model occasionally emits a bare `task({})`; executing it errors and the
        # orchestrator gives up without delegating. Detect empty-args calls.
        malformed = [tc for tc in tool_calls if tc.get("tool") and not tc.get("args", {})]
        if malformed and orch_malformed_nudges < 3 and iteration < MAX_ITERATIONS:
            orch_malformed_nudges += 1
            orch_think = True  # model misbehaved — next turn thinks before acting
            _log(f"[ORCHESTRATOR] ⚠️ Tool call(s) missing arguments: {[tc.get('tool') for tc in malformed]} — private re-query ({orch_malformed_nudges}/3).")
            # A2: private re-query keeps the failed turn OUT of history
            recovered = _private_requery(messages, content, malformed)
            if recovered is not None:
                content, tool_calls, recovered_reasoning = recovered
                if recovered_reasoning:
                    reasoning_content = recovered_reasoning
                _log("[ORCHESTRATOR] Recovered via private re-query — proceeding with fixed call.")
            else:
                messages.append(HumanMessage(
                    content=(
                        f"Your tool call `{malformed[0].get('tool')}` had NO arguments, so it could not "
                        "be executed. Re-issue it with all required arguments in this exact format:\n"
                        '```tool\n{"tool": "tool_name", "args": {"param": "value"}}\n```\n'
                        "For `task`, you must include `name` and `task`. Fix the call and try again."
                    ),
                    id=f"orch-malformed-{iteration}-{uuid.uuid4()}"
                ))
                continue

        # ── Session log (P1): record the step's assistant message ──
        # Placed after the requery guard so a failed turn (A2) is never logged.
        if orch_sess is not None:
            try:
                blocks = [{"type": "text", "text": content}]
                if reasoning_content:
                    blocks.insert(0, {"type": "thinking", "thinking": reasoning_content})
                orch_sess.append_assistant(blocks, stop_reason="toolUse" if tool_calls else "stop")
            except Exception:
                pass

        # ── Merged final turn (F2) ──
        # The model may emit its final report AND memory tools (update_feature /
        # update_project_meta) in ONE response. Execute those tools, then use the
        # response text as the final report — no extra summary-only turn (~1 saved
        # turn per completed task; the summary turn was pure ceremony).
        final_report = _extract_text_response(content)
        memory_tools = [tc for tc in tool_calls
                        if tc.get("tool") in ("update_feature", "update_project_meta")]
        has_non_memory = any(tc.get("tool") not in ("update_feature", "update_project_meta")
                             for tc in tool_calls)
        if (memory_tools and not has_non_memory and len(final_report.strip()) >= 40
                and ("## Summary" in final_report or "SUMMARY:" in final_report.upper())):
            _log(f"[ORCHESTRATOR] Merged final turn: {len(memory_tools)} memory tool(s) + summary in one response")
            if orch_sess is not None:
                try:
                    orch_sess.append_assistant(content, stop_reason="toolUse")
                except Exception:
                    pass
            merged_results = []
            for tc in memory_tools:
                try:
                    mres = execute_tool(tc["tool"], tc.get("args", {}))
                    merged_results.append(f"[{tc['tool']}] {str(mres)[:300]}")
                    tool_call_log.append({"tool": tc["tool"], "args": tc.get("args", {}), "iteration": iteration})
                    if orch_sess is not None:
                        try:
                            orch_sess.append_tool_result(
                                str(mres), tool_call_id=f"orch-mem-{iteration}-{tc['tool']}",
                                tool_name=tc["tool"],
                            )
                        except Exception:
                            pass
                except Exception as e:
                    merged_results.append(f"[{tc['tool']}] ERROR: {str(e)[:200]}")
                    if orch_sess is not None:
                        try:
                            orch_sess.append_tool_result(
                                str(e), tool_call_id=f"orch-mem-{iteration}-{tc['tool']}",
                                tool_name=tc["tool"], is_error=True,
                            )
                        except Exception:
                            pass
            messages.append(AIMessage(content=content))
            messages.append(HumanMessage(content="Tool results:\n" + "\n".join(merged_results)))
            clean_response = final_report
            _log(f"\n[ORCHESTRATOR] Agent finished after {iteration} iterations")
            _log(f"[ASSISTANT] {clean_response}")
            break

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
                # Truncate long results (A3: teaching note so the model stops
                # producing giant outputs it will only get clipped)
                if len(result_str) > 4000:
                    orig_len = len(result_str)
                    result_str = result_str[:2000] + "\n... (truncated middle) ...\n" + result_str[-2000:]
                    try:
                        from memory.context_compaction import truncation_note
                        result_str += truncation_note(orig_len, 4000)
                    except Exception:
                        pass

                # Session log (P1): record the tool result, best-effort
                if orch_sess is not None:
                    try:
                        orch_sess.append_tool_result(
                            result_str, tool_call_id=f"orch-{iteration}-{tool_name}",
                            tool_name=tool_name,
                        )
                    except Exception:
                        pass

                # Adaptive-ladder trigger: a subagent reported FAILURE (handoff
                # status, not stray words) → the NEXT orchestrator turn diagnoses
                # with high effort.
                if tool_name in ("task", "start_async_task"):
                    # Anti-retry + anti-runaway (harness-enforced): never re-delegate
                    # the same task key; a generous session ceiling (default 4)
                    # stops pathological loops without limiting legitimate
                    # multi-subagent decomposition.
                    _task_key = f"{args.get('name', '?')}:{str(args.get('task', ''))[:80]}"
                    escalation_counts[_task_key] = escalation_counts.get(_task_key, 0) + 1
                    _session_delegations += 1
                    if escalation_counts[_task_key] >= 2 and _task_key not in escalated_keys:
                        escalated_keys.add(_task_key)
                        _hard_stop_key = _task_key
                        _log(f"[ORCHESTRATOR] HARD STOP: re-delegation blocked for {_task_key[:80]}")
                    if _session_delegations > int(os.environ.get("ORCH_MAX_DELEGATIONS", "4")):
                        _hard_stop_key = _hard_stop_key or "session ceiling"
                        _log(f"[ORCHESTRATOR] HARD STOP: delegation ceiling ({_session_delegations}) reached")
                    low = result_str.lower()
                    is_failure = any(m in low for m in ("handoff report [failed]", "handoff report [blocked]",
                                                        "handoff report [stuck]", "[failed]", "[blocked]",
                                                        "timed out"))
                    if is_failure:
                        orch_think = True

                # Log tool output for frontend trace tracking
                _log(f"[TOOL OUTPUT] {tool_name}: {result_str}")
                tool_results.append(f"[{tool_name}] {result_str}")
                tool_call_log.append({"tool": tool_name, "args": args, "iteration": iteration})
            except Exception as e:
                tool_results.append(f"[{tool_name}] ERROR: {str(e)[:300]}")
                if orch_sess is not None:
                    try:
                        orch_sess.append_tool_result(
                            str(e)[:300], tool_call_id=f"orch-{iteration}-{tool_name}",
                            tool_name=tool_name, is_error=True,
                        )
                    except Exception:
                        pass
                orch_think = True  # tool blew up — next turn must diagnose

        # ── Feed results back ──
        feedback = "\n\n".join(tool_results) if tool_results else "No tool results."
        human_msg_id = f"orch-human-{iteration}-{uuid.uuid4()}"
        messages.append(HumanMessage(content=f"Tool results:\n{feedback}", id=human_msg_id))

        # ── Zero re-delegation: enforced HARD STOP ──
        if _hard_stop_key:
            messages.append(HumanMessage(
                content=(
                    "[HARD STOP] You violated the one-delegation rule by delegating the same "
                    "request twice. Re-delegation is FORBIDDEN — the subagent persists until "
                    "the request is fulfilled; failures are handled by the harness verification "
                    "gate, never by re-delegation. Do NOT call any delegation tool. Produce "
                    "your final report NOW with the honest outcome of the delegation you "
                    "already made."
                ),
                id=f"orch-hardstop-{iteration}-{uuid.uuid4()}",
            ))
            _log(f"[ORCHESTRATOR] Enforcing HARD STOP after re-delegation of {_hard_stop_key[:80]}")
            break

        # ── Progress audit ──
        if iteration > 4 and iteration % 4 == 1:
            no_delegation = all(
                t.get("tool") not in ("task", "start_async_task")
                for t in tool_call_log[-8:]
            )
            if no_delegation:
                _log("[ORCHESTRATOR] ⚠️ No delegation detected for several turns.")
                orch_think = True  # drifting — next turn thinks before delegating
                messages.append(HumanMessage(
                    content="[AUDITOR] You haven't called any subagent recently. "
                            "Delegate the work now: task(), start_async_task(), or wrap up if done.",
                    id=f"orch-audit-{iteration}-{uuid.uuid4()}"
                ))

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
    from sync.state_sync import safe_get_state
    new_msgs = [m for m in messages if getattr(m, "id", None) not in existing_ids]
    final_report = _extract_text_response(content) if content else ""
    final_error = ""
    if not final_report:
        final_error = "The orchestrator completed its delegation loop but returned no final response."
        _log(f"[ORCHESTRATOR ERROR] {final_error}")

    # ── Deterministic memory persistence (harness-owned, zero LLM cost) ──
    # The harness writes the session outcome to .agent-memory/features.md
    # itself instead of relying on the model voluntarily emitting
    # update_feature — this is what makes project memory actually accumulate.
    try:
        if final_report and project_path and os.path.isdir(project_path):
            from memory.memory_manager import update_feature
            from datetime import datetime
            _stamp = datetime.now().strftime("%Y-%m-%d %H:%M")
            update_feature(project_path, f"Session {_stamp}", final_report[:400])
    except Exception:
        pass

    # ── Lessons journal: auto-record subagent BLOCKED/FAILED handoffs ──
    # The Dev handoff reports classify their own failures
    # ("### AGENT HANDOFF REPORT [BLOCKED]" + Primary Exception + Error Trail).
    # Harvest those into .agent-memory/lessons.md (deduped) so future tasks
    # start knowing the repo's known blockers instead of re-discovering them.
    try:
        if project_path and os.path.isdir(project_path):
            from memory.memory_manager import append_lesson
            import re as _re
            for m in reversed(messages):
                content = str(getattr(m, "content", ""))
                if "AGENT HANDOFF REPORT" not in content:
                    continue
                status_m = _re.search(r"\[(BLOCKED|FAILED|DONE)\]", content)
                if not status_m or status_m.group(1) == "DONE":
                    continue
                status = status_m.group(1)
                exc_m = _re.search(r"\*\*Primary Exception:\*\*\s*`([^`]+)`", content)
                loc_m = _re.search(r"\*\*Failure Locus & Context\*\*", content)
                title = exc_m.group(1) if exc_m else "unclassified failure"
                body = content[content.index("#### 1. ERROR CLASSIFICATION"):][:900] if "#### 1. ERROR CLASSIFICATION" in content else content[:400]
                append_lesson(project_path, status, title, body)
                break  # one lesson per session — the LAST handoff
    except Exception:
        pass

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
