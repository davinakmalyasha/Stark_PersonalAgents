"""agents.graph — pydantic-graph orchestration for the orchestrator agent.

Replaces the hand-rolled ``while True`` loop in ``orchestrator_agent`` with a
typed, resumable, step-based graph (``GraphBuilder``):

  analyze → call_llm → decide → (park | nudge | verify → end)

- ``OrchestratorState``: the single mutable state object (history, iteration,
  tool_call_log, delegation bookkeeping, outputs).
- ``GraphDeps``: per-run dependencies (WorkspaceDeps, abort handling, inbox
  helpers, cancellation).
- Steps are ``@g.step`` functions; ``decide`` is a decision node; the driver
  uses ``graph.iter()`` + ``GraphRun.override_next`` so the event-driven
  park/wait pattern is preserved (the graph yields, we wait on the inbox, then
  inject the next task back in).
- ``span=`` is passed to ``graph.iter`` for OTel/Logfire tracing.

The legacy ``_orchestrator_loop`` was deleted — this graph driver is the ONLY
orchestrator path. ``ORCH_GRAPH=0`` is retained purely as a documented gate
(no fallback code).
"""
from __future__ import annotations

import json
import os
from dataclasses import dataclass, field
from typing import Any

from pydantic_ai.messages import ModelMessage, ModelRequest, ModelResponse, UserPromptPart

from pydantic_graph import GraphBuilder, GraphRunContext, StepContext

from llm.deps import WorkspaceDeps

# ── State ────────────────────────────────────────────────────────────────────


@dataclass
class OrchestratorState:
    """Mutable state shared across graph steps (typed replacement for dict-spread)."""

    client_request: str = ""
    project_path: str = ""
    chat_id: str = ""
    history: list[ModelMessage] = field(default_factory=list)
    iteration: int = 0
    tool_call_log: list[dict] = field(default_factory=list)
    park_timeouts: int = 0
    nudge_count: int = 0
    delegated_work: bool = False
    reasoning_content: str | None = None
    content: str = ""
    response_text: str = ""
    tool_calls: list[dict] = field(default_factory=list)
    native_msgs: list[ModelMessage] = field(default_factory=list)
    outputs: dict = field(default_factory=dict)
    code_updated: bool = False
    tech_spec_updated: bool = False
    final_report: str = ""
    error: str = ""

    # Resume support: the pending GraphTaskRequest to re-inject after a crash.
    pending_task_request: list[dict] | None = None

    def to_json(self) -> dict:
        from pydantic_ai import ModelMessagesTypeAdapter

        return {
            "client_request": self.client_request,
            "project_path": self.project_path,
            "chat_id": self.chat_id,
            "history": ModelMessagesTypeAdapter.dump_python(self.history, mode="json"),
            "iteration": self.iteration,
            "tool_call_log": self.tool_call_log,
            "park_timeouts": self.park_timeouts,
            "nudge_count": self.nudge_count,
            "delegated_work": self.delegated_work,
            "reasoning_content": self.reasoning_content,
            "content": self.content,
            "response_text": self.response_text,
            "tool_calls": self.tool_calls,
            "native_msgs": ModelMessagesTypeAdapter.dump_python(self.native_msgs, mode="json"),
            "outputs": self.outputs,
            "code_updated": self.code_updated,
            "tech_spec_updated": self.tech_spec_updated,
            "final_report": self.final_report,
            "error": self.error,
            "pending_task_request": self.pending_task_request,
        }

    @classmethod
    def from_json(cls, data: dict) -> "OrchestratorState":
        from pydantic_ai import ModelMessagesTypeAdapter

        s = cls(
            client_request=str(data.get("client_request") or ""),
            project_path=str(data.get("project_path") or ""),
            chat_id=str(data.get("chat_id") or ""),
            iteration=int(data.get("iteration") or 0),
            tool_call_log=list(data.get("tool_call_log") or []),
            park_timeouts=int(data.get("park_timeouts") or 0),
            nudge_count=int(data.get("nudge_count") or 0),
            delegated_work=bool(data.get("delegated_work")),
            reasoning_content=data.get("reasoning_content"),
            content=str(data.get("content") or ""),
            response_text=str(data.get("response_text") or ""),
            tool_calls=list(data.get("tool_calls") or []),
            outputs=data.get("outputs") or {},
            code_updated=bool(data.get("code_updated")),
            tech_spec_updated=bool(data.get("tech_spec_updated")),
            final_report=str(data.get("final_report") or ""),
            error=str(data.get("error") or ""),
            pending_task_request=data.get("pending_task_request"),
        )
        try:
            s.history = ModelMessagesTypeAdapter.validate_python(data.get("history") or [])
        except Exception:
            s.history = []
        try:
            s.native_msgs = ModelMessagesTypeAdapter.validate_python(data.get("native_msgs") or [])
        except Exception:
            s.native_msgs = []
        return s


# ── Deps ─────────────────────────────────────────────────────────────────────


@dataclass
class GraphDeps:
    """Per-run dependencies injected into the graph."""

    deps: WorkspaceDeps
    where: str = ""
    compaction_policy: Any = None
    valid_tools: list = field(default_factory=list)
    native_orchestrator_tools: list = field(default_factory=list)
    orch_sess: Any = None
    # injected callables (defaults wired in the driver; overridable for tests)
    build_prompt: Any = None
    run_llm_turn: Any = None
    drain_follow_up: Any = None
    consume_abort: Any = None
    park_wait: Any = None
    compact_history: Any = None
    estimate_tokens: Any = None
    append_log: Any = None


def _default_append_log(msg: str) -> None:
    try:
        print(msg, flush=True)
    except Exception:
        pass
    try:
        from sync.state_sync import safe_append_live_log
        safe_append_live_log(msg)
    except Exception:
        pass


# ── Project-path inference (deterministic create-before-delegate) ────────────

#: User requests almost always reference projects under D:\MyProject. We detect
#: an explicit ``D:\MyProject\...`` path in the request: if the folder exists we
#: pin ``state.project_path`` to it (delegate uses that workspace); if it's
#: missing we pin the target AND inject a compact directive telling the model to
#: call ``create_project`` FIRST, then delegate. Prompt reinforcement (compact
#: rule) + this deterministic safety net = belt and suspenders.
_PROJECT_ROOT_MARKER = "D:\\MyProject"


def _infer_project_path(client_request: str, fallback: str) -> tuple[str, str]:
    """Return ``(project_path, directive)``.

    ``directive`` is a short instruction appended to the task message when the
    referenced folder does not exist yet ('' when nothing to do).
    """
    import re as _re

    req = client_request or ""
    # Find a D:\MyProject\... path (forward or back slashes).
    m = _re.search(r"(?:D:|d:)[\\/]+MyProject[\\/]+[^\s,;.!?\"']+", req)
    if not m:
        return fallback, ""
    raw = m.group(0).replace("/", "\\")
    # Normalize trailing separators / whitespace
    target = os.path.normpath(raw.strip().rstrip("\\/"))
    if not target.lower().startswith(os.path.normpath(_PROJECT_ROOT_MARKER).lower()):
        return fallback, ""
    if os.path.isdir(target):
        return target, ""
    # Folder missing → pin target + directive to create first.
    name = os.path.basename(target) or "project"
    directive = (
        f"[PROJECT CHECK] The folder '{target}' does not exist yet. "
        f"Call create_project(name='{name}', concept=<one-line goal>) FIRST to "
        f"create it, THEN delegate the actual task to Dev in that new workspace."
    )
    return target, directive


# ── Steps ────────────────────────────────────────────────────────────────────

# Graph object is built lazily (the builder + steps need no side effects at
# import time; steps are registered as plain functions on the builder).


def _build_graph() -> GraphBuilder:
    g = GraphBuilder(state_type=OrchestratorState, deps_type=GraphDeps, input_type=str, output_type=dict)

    @g.step
    async def analyze(ctx: StepContext[OrchestratorState, GraphDeps, str]) -> Any:
        """Build the initial context: system prompt, task message, vision, session log."""
        state = ctx.state
        deps = ctx.deps
        client_req = ctx.inputs or state.client_request or ""
        state.client_request = client_req
        project_path = state.project_path or deps.deps.project_path or os.environ.get("HARNESS_ROOT", "")
        state.project_path = project_path
        chat_id = state.chat_id or deps.deps.chat_id or ""
        state.chat_id = chat_id

        # Deterministic project-path inference: pin the workspace to a referenced
        # D:\MyProject\... path, and if it's missing tell the model to create it first.
        inferred_path, directive = _infer_project_path(client_req, project_path)
        if inferred_path:
            state.project_path = inferred_path

        try:
            deps.append_log(f"[ORCHESTRATOR] Analyzing request...")
            if deps.build_prompt is not None:
                static_system = deps.build_prompt()
            else:
                from agents.orchestrator_agent import _build_orchestrator_prompt
                static_system = _build_orchestrator_prompt()
            task_message = f"## User Request\n{client_req}"
            if directive:
                task_message += f"\n\n{directive}"
            if state.project_path and os.path.isdir(state.project_path):
                task_message = f"## WORKSPACE\n{state.project_path}\n\n" + task_message
            try:
                from server.context.chat_history import build_chat_context
                from sync.state_sync import shared_state
                ctx_path = "_general" if shared_state.get("is_general", False) else project_path
                chat_history_str = build_chat_context(
                    ctx_path, chat_id, max_messages=30, max_chars_per_msg=3000, exclude_traces=True
                )
                if chat_history_str:
                    task_message += f"\n\n## Recent Chat History\n{chat_history_str}"
            except Exception as e:
                deps.append_log(f"[ORCHESTRATOR] Error loading chat history: {e}")
            try:
                from agents.delegation import recent_jobs_block
                jb = recent_jobs_block(10)
                if jb:
                    task_message += f"\n\n{jb}"
            except Exception:
                pass

            from llm.model_history import build_initial_history
            state.history = build_initial_history(static_system, task_message)

            try:
                from tools.vision_channel import drain_attachment_images_native
                if drain_attachment_images_native(state.history, ctx=deps.deps):
                    deps.append_log("[VISION] Orchestrator: user attachment image(s) injected")
            except Exception:
                pass
        except Exception as e:
            deps.append_log(f"[ORCHESTRATOR] analyze error (non-fatal): {e}")

        if deps.orch_sess is not None:
            try:
                deps.orch_sess.append_message("user", client_req, source="user")
            except Exception:
                pass
        return "call_llm"

    @g.step
    async def call_llm(ctx: StepContext[OrchestratorState, GraphDeps, str]) -> Any:
        """Run one native pydantic-ai turn (tools executed internally)."""
        state = ctx.state
        deps = ctx.deps
        state.iteration += 1
        deps.append_log(f"\n[ORCH Iteration {state.iteration}]")

        # Compaction at iteration boundaries (native ModelMessage history).
        # Runs whenever the policy trigger is hit, including the first turn.
        if deps.compact_history is not None:
            try:
                est = deps.estimate_tokens(state.history) if deps.estimate_tokens is not None else 0
                if est >= (deps.compaction_policy.checkpoint_trigger if deps.compaction_policy else 1 << 30):
                    before = est
                    state.history = deps.compact_history(
                        state.history,
                        policy=deps.compaction_policy,
                        summary_text="[Native compaction checkpoint — earlier turns summarized]",
                    )
                    after = deps.estimate_tokens(state.history) if deps.estimate_tokens is not None else 0
                    deps.append_log(
                        f"[ORCHESTRATOR] Native checkpoint ({before:,} -> {after:,} est. tokens)"
                    )
            except Exception as e:
                deps.append_log(f"[ORCHESTRATOR] Native compaction error (non-fatal): {e}")

        if deps.run_llm_turn is not None:
            text, native_msgs, reasoning = deps.run_llm_turn(state, deps)
        else:
            from agents.orchestrator_agent import _run_native_orchestrator_turn
            text, native_msgs, reasoning = _run_native_orchestrator_turn(state, deps)

        state.content = text or ""
        state.response_text = text or ""
        state.reasoning_content = reasoning
        state.native_msgs = native_msgs or []
        if state.native_msgs:
            # Merge the executed turn into history so the next iteration sees
            # its own assistant/tool turns (parity with the workspace agents).
            state.history.extend(state.native_msgs)

        # Extract tool calls from native transcript slice
        calls: list[dict] = []
        for m in (native_msgs or []):
            if isinstance(m, ModelResponse):
                for p in getattr(m, "parts", []) or []:
                    if getattr(p, "part_kind", "") == "tool-call":
                        args = p.args if isinstance(p.args, dict) else {}
                        if isinstance(args, str):
                            try:
                                import json as _json
                                args = _json.loads(args)
                            except Exception:
                                args = {}
                        calls.append({
                            "tool": p.tool_name,
                            "args": args if isinstance(args, dict) else {},
                            "id": p.tool_call_id or "",
                        })
        state.tool_calls = calls
        for c in calls:
            state.tool_call_log.append({"tool": c.get("tool"), "args": c.get("args"), "id": c.get("id")})
        state.delegated_work = any(
            c.get("tool") in ("delegate", "task", "resume_project", "create_project")
            for c in calls
        )
        return "decide"

    @g.step
    async def decide(ctx: StepContext[OrchestratorState, GraphDeps, str]) -> Any:
        """Route to park/nudge/end based on inbox + tool state (decision node)."""
        state = ctx.state
        deps = ctx.deps

        # Abort first
        if deps.consume_abort is not None:
            try:
                if deps.consume_abort(state.chat_id):
                    try:
                        from agents import delegation as _dlg
                        cancelled = _dlg.cancel_all()
                        deps.append_log(f"\n[CANCELLED] Run cancelled by user request ({cancelled} delegation(s) stopped).")
                    except Exception:
                        deps.append_log("\n[CANCELLED] Run cancelled by user request.")
                    state.final_report = "[CANCELLED] Run cancelled by user request."
                    return "end"
            except Exception:
                pass

        # Park while children run
        if state.delegated_work or (state.tool_calls and not state.native_msgs):
            try:
                from agents import delegation as _dlg
                pending = _dlg.running_count(state.chat_id) if state.chat_id else 0
            except Exception:
                pending = 0
            if pending > 0:
                deps.append_log(f"[ORCHESTRATOR] {pending} delegation(s) running — parking for events")
                return "park"
        return "verify"

    @g.step
    async def park(ctx: StepContext[OrchestratorState, GraphDeps, str]) -> Any:
        """Wait on the inbox; the driver injects the next task via override_next."""
        state = ctx.state
        deps = ctx.deps
        wait = {"events": [], "steering": [], "aborted": False, "timed_out": True}
        if deps.park_wait is not None:
            wait = deps.park_wait(state.chat_id)
        if wait.get("aborted"):
            try:
                from agents import delegation as _dlg
                cancelled = _dlg.cancel_all()
                deps.append_log(f"\n[CANCELLED] Run cancelled by user request ({cancelled} delegation(s) stopped).")
            except Exception:
                deps.append_log("\n[CANCELLED] Run cancelled by user request.")
            state.final_report = "[CANCELLED] Run cancelled by user request."
            return "end"

        payload_parts = []
        if wait.get("events"):
            payload_parts.append(_render_delivery("DELEGATION RESULT", wait["events"]))
        if wait.get("steering"):
            payload_parts.append(_render_delivery("STEERING", wait["steering"]))
        if payload_parts:
            deps.append_log(
                f"[ORCHESTRATOR] Woke with {len(wait.get('events', []))} result(s), "
                f"{len(wait.get('steering', []))} steering — new coordination turn"
            )
            state.history.append(ModelRequest(parts=[UserPromptPart(content="\n\n".join(payload_parts))]))
            return "call_llm"
        if wait.get("timed_out"):
            state.park_timeouts += 1
            if state.park_timeouts >= 3:
                deps.append_log("[ORCHESTRATOR] Wait timeout x3 with children alive — reporting partial.")
                state.history.append(ModelRequest(parts=[UserPromptPart(content=_render_delivery("SYSTEM", [
                    "Delegations exceeded the maximum wait window and are STILL RUNNING. "
                    "Give the user a partial report now: what finished, what is outstanding, "
                    "and which chats to check. Do NOT spawn duplicates.",
                ]))]))
                state.park_timeouts = 0
                return "call_llm"
            deps.append_log(f"[ORCHESTRATOR] Wait timeout #{state.park_timeouts} — re-parking (children alive)")
            return "park"
        return "call_llm"

    @g.step
    async def verify(ctx: StepContext[OrchestratorState, GraphDeps, str]) -> Any:
        """Validate exit: nudge too-short replies, drain follow-ups, else finish."""
        state = ctx.state
        deps = ctx.deps
        clean = state.response_text.strip()
        is_too_short = len(clean) < 20
        if is_too_short and state.delegated_work and state.nudge_count < 2:
            state.nudge_count += 1
            deps.append_log(f"[ORCHESTRATOR] Exit too short — nudging ({state.nudge_count}/2).")
            state.history.append(ModelRequest(parts=[UserPromptPart(content="[NUDGE] That response was too brief. Summarize what was accomplished and offer a next step the user might want.")]))
            return "call_llm"

        follow_msgs = []
        if deps.drain_follow_up is not None:
            try:
                follow_msgs = deps.drain_follow_up(state.chat_id) or []
            except Exception:
                follow_msgs = []
        if follow_msgs:
            deps.append_log(f"[ORCHESTRATOR] Follow-up queued ({len(follow_msgs)}) — continuing")
            state.history.append(ModelRequest(parts=[UserPromptPart(content=_render_delivery("FOLLOW-UP", follow_msgs))]))
            return "call_llm"

        state.final_report = clean
        deps.append_log(f"\n[ORCHESTRATOR] Agent finished after {state.iteration} iterations")
        deps.append_log(f"[ASSISTANT] {clean}")
        return "end"

    @g.step
    async def end(ctx: StepContext[OrchestratorState, GraphDeps, str]) -> dict:
        """Terminal step: produce the final result dict."""
        state = ctx.state
        return {
            "messages": [],
            "agent_report": state.final_report or state.content or "",
            "code": "",
            "test_report": "",
            "project_path": state.project_path,
            "chat_id": state.chat_id,
            "code_updated": state.code_updated,
            "tech_spec_updated": state.tech_spec_updated,
            "next_agent": "finish",
        }

    # ── Wire edges ──
    g.add(
        g.edge_from(g.start_node).to(analyze),
        g.edge_from(analyze).to(call_llm),
        g.edge_from(call_llm).to(decide),
        g.edge_from(decide).to(
            g.decision()
            .branch(g.match(str, matches=lambda s: s == "park").to(park))
            .branch(g.match(str, matches=lambda s: s == "verify").to(verify))
            .branch(g.match(str, matches=lambda s: s == "end").to(end))
            .branch(g.match(str, matches=lambda s: s == "call_llm").to(call_llm))
            .branch(g.match(str).to(end))
        ),
        g.edge_from(park).to(
            g.decision()
            .branch(g.match(str, matches=lambda s: s == "call_llm").to(call_llm))
            .branch(g.match(str, matches=lambda s: s == "park").to(park))
            .branch(g.match(str, matches=lambda s: s == "end").to(end))
            .branch(g.match(str).to(end))
        ),
        g.edge_from(verify).to(
            g.decision()
            .branch(g.match(str, matches=lambda s: s == "call_llm").to(call_llm))
            .branch(g.match(str).to(end))
        ),
        g.edge_from(end).to(g.end_node),
    )
    return g


def _render_delivery(kind: str, parts) -> str:
    """Render inbox events into a user-prompt block (mirrors orchestrator_agent.render_delivery)."""
    try:
        from inbox import render_delivery as _rd
        return _rd(kind, parts)
    except Exception:
        return f"[{kind}]\n" + "\n\n".join(str(p) for p in (parts or []))


_GRAPH: Any = None
_GRAPH_LOCK = object()


def get_graph():
    """Lazily build + cache the graph (import-safe)."""
    global _GRAPH
    if _GRAPH is None:
        _GRAPH = _build_graph().build()
    return _GRAPH


# ── Resume helpers ───────────────────────────────────────────────────────────


def state_file_path(chat_id: str, project_path: str) -> str:
    d = os.path.join(project_path or "", ".deep_agents")
    try:
        os.makedirs(d, exist_ok=True)
    except Exception:
        pass
    return os.path.join(d, f"orch_state_{chat_id}.json")


def save_state(state: OrchestratorState, project_path: str = "") -> None:
    try:
        path = state_file_path(state.chat_id, state.project_path or project_path)
        with open(path, "w", encoding="utf-8") as f:
            json.dump(state.to_json(), f, ensure_ascii=False, indent=2)
    except Exception:
        pass


def load_state(chat_id: str, project_path: str) -> OrchestratorState | None:
    try:
        path = state_file_path(chat_id, project_path)
        if not os.path.isfile(path):
            return None
        with open(path, "r", encoding="utf-8") as f:
            data = json.load(f)
        return OrchestratorState.from_json(data)
    except Exception:
        return None


def clear_state(chat_id: str, project_path: str = "") -> None:
    try:
        path = state_file_path(chat_id, project_path)
        if os.path.isfile(path):
            os.remove(path)
    except Exception:
        pass
