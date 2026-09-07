# Fixed Concept — LangChain Workspace Agents

Status: **FIXED CONCEPT — the migration contract.** Everything below is the
spec that the current refactor must converge on. When in doubt about an
architectural decision, this document wins.

Last updated: 2026-08-31 (pre-Phase A of the full-native migration)

---

## 1. Purpose & Non-Goals

This repo is a **multi-agent orchestration system**. The boss (user) hands a
task to an **Orchestrator agent**; the Orchestrator splits it into jobs and
routes each to the right **Workspace agent**; workspace agents work inside
their own scoped workspace with their own warehouse-selected tools; while they
work, **lightweight subagents** (task-specific, ephemeral) can be spawned to
do focused sub-work.

Non-goals:
- Not a general-purpose "one agent does everything" app.
- Not a framework to be re-sold; it is the user's main personal agent system.
- Not a home for vendored LLM frameworks — we build ON pydantic-ai, we do not
  reimplement what it gives us (loops, tool execution, retries, history,
  cancellation, model fallback).

The whole codebase must be **maintainable and scalable** because this is the
primary agent system and will connect to many types of workspace, app, and
MCP server over time. "Easy to add a new agent" and "easy to wire a new tool /
MCP / workspace" are first-class requirements.

---

## 2. Architecture (the "fixed concept")

```
                        ┌─────────────────────────────┐
                        │         ORCHESTRATOR        │
                        │  routes, delegates, tracks  │
                        └───────┬───────────┬─────────┘
                     delegate   │           │
              ┌─────────────────┴──┐   ┌────┴─────────────────┐
              ▼                    ▼   ▼                     ▼
   ┌─────────────────┐  ┌──────────────────┐   ┌──────────────────────┐
   │ WORKSPACE AGENT │  │ WORKSPACE AGENT  │   │ WORKSPACE AGENT      │
   │ Developer       │  │ General / House  │   │ DFD / (future: ...)  │
   │ coding, SE      │  │ docs, smart-home │   │ agency business      │
   └───────┬─────────┘  └────────┬─────────┘   └──────────┬───────────┘
           │                    │                         │
           │   spawns           │                         │
           ▼                    ▼                         ▼
   ┌───────────────────────────────────────────────────────────────┐
   │                        SUBAGENTS (lightweight)                │
   │   task-specific, small system prompt, chosen tools,          │
   │   one focused job, result returned to the spawner            │
   └───────────────────────────────────────────────────────────────┘
           │                    │                         │
           └────────────────────┼─────────────────────────┘
                                ▼
   ┌───────────────────────────────────────────────────────────────┐
   │                        TOOL WAREHOUSE                        │
   │   every tool registered ONCE (name/schema/impl); agents pick  │
   │   by scope allowlist from a registry — never hand-wired       │
   │   + MCP servers (scoped) + Workspace bindings (project_path)  │
   └───────────────────────────────────────────────────────────────┘
```

### The three tiers

| Tier | Role | Behavior |
|---|---|---|
| **Orchestrator** | Single coordinator | Receives the user task, splits into independent jobs, delegates to workspace agents (optionally in parallel), parks/waits on their results (inbox), relays, nudges, aborts. Does NOT do hands-on work (no edit/run). |
| **Workspace agent** | Scoped single-agent loop | One agent per workspace/domain, working inside its own `project_path`/scope. E.g. **Developer** = coding/software-engineering tasks only (read/edit/run/search/test); **General** = docs, browsing, personal; **House** = smart home devices/scenes; **DFD** = agency business (clients/projects, no money tools). |
| **Subagent** | Lightweight, task-specific | Like classic harness subagents: a small purpose-built system prompt + a chosen tool subset, spawned by a workspace agent (or orchestrator) to do one focused job and report back. Cheap, ephemeral, no cross-turn memory by default. |

### Roles & boundaries

- **Orchestrator** is NOT a coder. Its only "verification" is: did the
  workspace agent produce what the user asked? It tracks jobs via a plan
  tracker and job ledger; it can call `delegate`/`delegation_status`,
  `create_project`/`resume_project`, `ask_user`, plan tools.
- **Workspace agents** are single-agent loops: they call tools, get results,
  iterate until the task is done or they hit a limit. They own their
  conversation history, compaction, and session persistence.
- **Subagents** are spawned with a complete self-contained job description
  (goal, constraints, paths, how to verify). They see nothing else from the
  parent conversation. Their result is returned as a tool result to the
  spawner, which decides what to do with it.

---

## 3. The Tool Warehouse (the scalable model)

Tools are stored "in a warehouse" instead of being hand-wired per agent.

### Rule: a tool is defined ONCE, then *assigned* to scopes.

1. **Implementation**: a plain Python function (the `execute` body) lives in
   `tools/` (or `tools/AddOnTool/`), with its docstring/params as the schema.
2. **Registry**: `agents/workspace_registry.py` is the **single source of
   truth** mapping agent key → allowed tool names, MCP scopes, tier, history
   namespace. Adding a new agent = adding one `WorkspaceSpec`.
3. **Selection at build time**: an agent gets its tools by looking up the
   registry (scope ACL), never by hand-listing tools in the agent file.
4. **Per-run scoping**: `WorkspaceDeps` carries the concrete allowlist for a
   run; pydantic-ai `FilteredToolset` enforces it at tool-build time.
5. **MCP servers**: scoped per agent via `mcp_scopes` in the spec; MCP tools
   are first-class tools in the same warehouse model.

### What "scale" means here

- New workspace/app = new `WorkspaceSpec` (maybe a new `WorkspaceDeps`
  factory for its workspace bindings) + system prompt. No touching the
  orchestrator or other agents.
- New tool = new function + register it; adding it to an agent = one tuple
  entry in that agent's spec.
- New MCP server = register it; exposing it to an agent = add its scope to
  the spec.

---

## 4. Agent/LLM primitives — all pydantic-ai

Everything in this section is **the native pydantic-ai way**. The migration
target is: no hand-rolled agent loops, no hand-rolled LLM plumbing, no custom
message-schema bridging.

| Concern | Native mechanism (pydantic-ai 2.36) |
|---|---|
| Agent construction | `Agent(model, toolsets=[...], capabilities=[...], deps_type=WorkspaceDeps, system_prompt=...)` via one `build_agent(role, ...)` |
| Tool execution + retries | `Tool` / `@agent.tool`, parallel by default, `ModelRetry`, `ToolFailed`, per-tool `retries` |
| Tool schema | Derived from signature + docstring (griffe); `Tool.from_schema` ONLY as a legacy bridge, to be removed |
| Tool scoping | `FilteredToolset(filter_func=...)` |
| Plan-mode veto | `Tool.prepare` (return `None` to hide a tool for a step) |
| ask_user / approval | `requires_approval=True` / `ApprovalRequired` / `CallDeferred` + `HandleDeferredToolCalls` |
| Tool cache hit | hook `SkipToolExecution(result)` |
| Secret redact / offload | `Hooks` (before/after/wrap_tool_execute) |
| Chat history & persistence | `message_history=` + `ModelMessagesTypeAdapter` (JSON round-trip); `conversation_id`/`run_id`; `ReinjectSystemPrompt` |
| Compaction | `ProcessHistory` capability (truncate + summarizer agent) |
| Cancellation / stop | `CancellationToken`; `RunCancelled.all_messages()` to resume |
| Multi-agent delegation | A parent tool that `await sub_agent.run(prompt, deps=ctx.deps, usage=ctx.usage)` and returns the output; OR `pydantic-graph` (`GraphBuilder`/`BaseNode`) for explicit control flow |
| Model fallback / free pool | `FallbackModel(...)` / custom `Provider` subclass |
| Streaming to UI | `run_stream` / `run_stream_events` + `event_stream_handler` |
| Steering mid-run | `RunContext.enqueue(...)` |

### Explicitly BANNED (must be deleted in the migration)

- Hand-rolled `while True` agent loops that call the LLM, unpack tool calls,
  execute tools, and re-append messages (orchestrator, developer, general,
  house, dfd, subagent manager, subagentTools) — replaced by native runs.
- LangChain messages as the storage/history format (`SystemMessage`,
  `HumanMessage`, `AIMessage`, `ToolMessage` lists) and the
  `_lc_messages_to_pai`/`_pai_to_lc` converters — replaced by native
  `ModelMessage` history.
- `StrWithMetadata`, regex JSON parsing of LLM output, manual retry/backoff
  ladders, `<think>` tag extraction — replaced by native message parts
  (`ThinkingPart`) and native retry/validation.
- `asyncio.run()` inside call paths, `time.sleep` retry loops — replaced by
  async calls driven from the server's event loop.
- ContextVars as plumbing (`stream_chat_override`, `active_permissions`,
  `_TASK_DEPTH`, `pending_images`) — replaced by `RunContext.deps`.
- Hand-maintained duplicate tool metadata (`TOOL_DEFINITIONS` + `_TOOL_DOCS`
  + `_TOOL_REQUIRED_ARGS` + `_SCOPE_MAP` as separate copies) — replaced by
  the registry + signatures.
- `PAI_NATIVE_TOOLS=0` legacy single-turn path — removed once all loops are
  native.

---

## 5. Concurrency & isolation

- Workspace agents run **in parallel** (orchestrator fan-out) — no shared
  mutable state; everything per-run lives in `WorkspaceDeps`.
- `shared_state` remains only as a *cross-process coordination* surface
  (live log, session store), never as the source of truth for a run's
  context.
- Subagent spawns are bounded (a task-depth limit / usage limits); a runaway
  subagent must not kill the parent.

---

## 6. Verification / "done" contract for each migration step

A step is done when:
1. `pytest tests -q` passes (143+ tests, no regressions).
2. `scratch/compare_agents.py` 3-path PASSED.
3. `scratch/test_pai_transport_offline.py` + `scratch/test_pai_transport_wire.py`
   all gates PASSED.
4. Import sweep of every touched module OK.
5. No banned pattern remains in the touched files (see §4 banned list).
6. The "fixed concept" shape is preserved: Orchestrator → Workspace agents →
   subagents → warehouse, exactly as §2–§3.

---

## 7. Known "how it actually works" details to preserve

- **Provider/model resolution**: use the `command` provider for model tests
  (`https://api.commandcode.ai/provider/v1`, `flash: deepseek/deepseek-v4-flash`),
  resolved via `.deep_agents/llm_provider.json` + `_resolve_llm_provider()`.
- **Parallel tool calls**: pydantic-ai runs tools in parallel by default; the
  read/stuff class of tools (read_file, search_code, browser_act, ...) must
  stay parallel-safe. The `PARALLEL_SAFE` set is preserved.
- **MCP tools must be real tools on the native path** (currently skipped
  when no impl exists — this is a bug to fix, not a feature).
- **Vision**: user-attached images and inline images from tools flow via the
  vision channel; the native path must keep them visible to the model.
- **Session store**: the server-side session JSONL (`server/session/log.py`)
  is the durable transcript; agent loops append to it; it is NOT the model
  history source.
- **Free-pool providers** (`llm/free_providers.py`) are a separate,
  self-contained feature; if/when ported to a custom `Provider`, behavior
  (rate limits, retries, provider rotation) must be preserved.
