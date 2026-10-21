# 06 — Orchestration

Routing, the durable orchestrator graph, delegation, specialists, and plan
mode. This is the part that makes Stark a multi-agent system rather than a
chat loop.

---

## 1. Deterministic front-door router

Port `routing/router.py` exactly. It is pure, has no LLM, and has no side
effects.

```rust
pub enum RouteKind { ProjectDev, House, General, Dfd, Design, Orchestrator }

pub struct RouteDecision {
    pub kind: RouteKind,
    pub project: Option<ProjectRef>,
    pub reason: String,
}

pub fn decide_route(
    prompt: &str,
    workspace_path: &str,
    chat_id: &str,
    workspace_kind: &str,
) -> RouteDecision;
```

Order:
1. Explicit `workspace_kind` hints:
   - `house` / `house_workspace` → House
   - `personal` / `personal_workspace` / `general_workspace` → General
   - `dfd` / `dfd_workspace` → DFD
   - `design` / `design_workspace` → Design
2. Creation phrasing (`create project`, `new project`, `project named`) →
   Orchestrator.
3. Existing project hit (path or name mention) → direct Developer.
4. Coding workspace with no direct hit → Orchestrator.
5. Empty workspace path → Orchestrator.

On router error: fall back to Orchestrator, never drop the request.

Keep `RouteDecision.reason` for tracing/tests.

---

## 2. Orchestrator graph

The Python implementation is a typed, resumable `pydantic-graph`:

```
analyze → call_llm → decide → park → verify → end
```

Semantics to preserve:
- Boss never codes.
- State is saved after every step; crash = resume.
- `park` sleeps on an inbox queue while workers run; it does not hold an LLM
  context.
- `verify` checks the worker's report against the ask.
- `end` produces a short summary and closes the stream.

### 2.1 Rust model

Hand-roll the step machine (no workflow framework):

```rust
pub enum Step {
    Analyze,
    CallLlm,
    Decide,
    Park,
    Verify,
    End,
}

pub struct OrchestratorState {
    pub chat_id: ChatId,
    pub workspace: PathBuf,
    pub request: String,
    pub plan: Vec<PlanItem>,
    pub pending_jobs: Vec<JobId>,
    pub completed_jobs: Vec<JobResult>,
    pub outputs: Outputs,
    pub step: Step,
    pub version: u64,
}

#[async_trait]
pub trait StepRunner {
    async fn run(&self, state: &mut OrchestratorState, ctx: &RunCtx) -> Result<Next>;
}
```

Persistence:
- `state.json` or a `runs` row per chat (checkpoint after each step).
- Checkpoint includes `step`, `version`, plan, pending job IDs, last event seq.
- On boot, load and resume from the checkpoint; if the step was mid-tool,
  re-run it idempotently where possible.

### 2.2 Park/resume

- `park` registers the run as waiting and returns control to the event loop.
- Delegation results are enqueued into the orchestrator inbox.
- A wake re-enters the graph at `verify`.
- A timeout/stall produces a user-visible question, not a silent hang.

Do not implement `park` as a thread sleep holding a model connection. The
point is to release the LLM context.

---

## 3. Delegation

Source: `agents/delegation.py`, `tools/AddOnTool/delegationTools.py`.

### 3.1 Job types

| Type | Behavior |
|---|---|
| **Fresh** | New child chat + new worker run; result returns to the boss |
| **Resume** | Continue an existing durable Dev child chat |
| **Subagent** | Ephemeral specialist; no cross-turn memory by default |

### 3.2 Fan-out

- `delegate` accepts a list of jobs (agent, task, workspace, context).
- Each job gets its own child chat ID and its own Tokio task.
- Concurrency is bounded (slot semaphore; existing config).
- Results are reduced into one `[DELEGATION RESULT]` block.
- A failure is reported per job, not as a global run failure.
- Child runs inherit the workspace, envelope, and tool scopes; they do not
  inherit the parent conversation.

### 3.3 Status, steer, cancel

- `delegation_status` lists queued/running/done/failed/cancelled + chat IDs.
- `job_message` steers a running job (fresh: mirror to job chat + steering
  channel; resume: queue on the durable child).
- `job_cancel` cooperatively aborts; settle as `CANCELLED`.
- Child runs have their own cancellation tokens; aborting the parent cancels
  children.

### 3.4 Rust types

```rust
pub struct Job {
    pub id: JobId,
    pub kind: JobKind,
    pub agent: AgentKey,
    pub task: String,
    pub workspace: PathBuf,
    pub chat_id: ChatId,
    pub status: JobStatus,
    pub cancel: CancellationToken,
    pub steer: mpsc::Sender<SteerMessage>,
}

pub struct JobResult {
    pub job_id: JobId,
    pub status: JobStatus,
    pub report: String,
    pub usage: Usage,
    pub error: Option<String>,
}
```

Store jobs in `stark-store` (reuse `jobs.db` schema) so restart can report
what was running.

---

## 4. Specialists and subagents

- Specialist profiles: `agents/specialists/*.json` (UI, DevOps, Explore,
  Compactor). Load into `AgentSpec`; no consumer edits when adding one.
- `call_subagent` actions: `run`, `spawn`, `status`.
- Subagents see only the job description + selected tools.
- Their report is a tool result; the spawner decides what to do next.

Keep the registry as the single source of truth for:
- tool scopes
- MCP scopes
- model pin/thinking
- compaction overrides
- per-tool caps
- aliases/delegatability
- history namespace

---

## 5. Plan mode

Source: `tools/plan_mode_guard.py`, `MainTool/planTools.py`.

- Plan mode hides write/execute tools for a step.
- `update_plan` replaces the plan dock; at most one `in_progress`.
- `propose_plan` creates a plan-review question (`ask_user` intent).
- The plan block is appended to the orchestrator prompt when a plan exists
  and is not completed.

Rust:
```rust
pub struct PlanItem { pub step: String, pub status: PlanStatus }
pub struct Plan { pub scope: String, pub items: Vec<PlanItem> }
```
Persist per chat/scope; include in `AppState.plan`.

---

## 6. Schedules, wakes, canvas

Source: `server/store/agent_wakes.py`, `schedules.py`,
`server/services/agent_wake.py`, `canvas_watch.py`, `schedule_scheduler.py`.

- `schedule`: one-off/recurring timed wake for the orchestrator chat.
- `give_note`: sticky notes for a chat/project.
- `canvas_mode`: watch selected agent chats; on completion/failure enqueue a
  wake for the orchestrator (bounded actions, until timestamp).
- Background tasks poll due rows every second, enqueue a synthetic user
  message, and start a normal run.

Rust: one `Scheduler` task + `WakeDispatcher` task, DB-backed; no per-schedule
threads. Use `next_run_at` indexes.

---

## 7. Module layout

```
stark-agents/
  src/
    registry.rs        # AgentSpec, aliases, scopes
    routing.rs         # decide_route
    graph/
      mod.rs           # Step, OrchestratorState, runner
      checkpoint.rs    # persist/resume
    delegate.rs        # jobs, fan-out, reduction
    subagent.rs        # run/spawn/status
    plan.rs            # plan mode
    prompts/           # role prompts as files
  tests/
    routing.rs
    graph_resume.rs
    delegation.rs
```

---

## 8. Acceptance

- Router decisions match Python for a fixture corpus (including hints,
  creation phrasing, existing projects, errors).
- Orchestrator resumes after a simulated crash at every step.
- A two-job delegation runs in parallel, returns one reduced result, and can
  be steered/cancelled.
- A specialist profile added as JSON appears in the registry and tool scopes
  without code changes.
- Plan mode hides write tools and the plan survives restart.
- Schedules/wakes fire once, persist, and do not duplicate after restart.
