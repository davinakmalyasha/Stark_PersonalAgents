# 05 — Agent Runtime

The agent runtime is the hand-rolled replacement for pydantic-ai's `Agent`
loop. It uses the same low-level approach as Codex: `reqwest` + SSE, typed
events, explicit retries, explicit cancellation.

---

## 1. Provider layer (`stark-providers`)

### 1.1 No SDK

Do not use `async-openai`, `rig`, or an official OpenAI SDK. Build the wire
client directly:

```rust
pub trait Provider: Send + Sync {
    fn stream_chat(
        &self,
        model: &ModelId,
        messages: &[Message],
        tools: &[ToolSpec],
        settings: &ModelSettings,
        cancel: &CancellationToken,
    ) -> ProviderStream;
}
```

`ProviderStream = Pin<Box<dyn Stream<Item = Result<ProviderEvent, ProviderError>> + Send>>`.

```rust
pub enum ProviderEvent {
    TextDelta(String),
    ReasoningDelta(String),
    ToolCallDelta { index: usize, id: Option<String>, name: Option<String>, args: String },
    Usage(Usage),
    Done { stop_reason: StopReason },
    Error(ProviderError),
}
```

### 1.2 HTTP/SSE

- `reqwest::Client` with `rustls`, connection pooling, timeouts.
- `POST {base_url}/chat/completions` with `stream: true`.
- Parse bytes with `eventsource-stream`.
- Accumulate tool-call deltas by index; JSON-parse at the end.
- Handle `[DONE]`, empty keepalives, and provider-specific fields:
  `reasoning_content`, `reasoning`, `usage`, `cache_hit_tokens`,
  `cache_miss_tokens`.
- Map errors:
  - 401/403 → `Auth`
  - 429 → `RateLimited { retry_after }`
  - 5xx → `Upstream`
  - timeout → `Timeout`
  - malformed stream → `Protocol`
- Retries: exponential backoff with jitter; retry only idempotent calls;
  do not retry canceled runs.
- Log one `tracing` span per request with model, latency, tokens, finish reason.

### 1.3 Model registry

Port `llm/model_history.py` + `llm/llm.py` knowledge into a static registry:

```rust
pub struct ModelProfile {
    pub id: String,
    pub context_window: usize,
    pub max_output: usize,
    pub supports_vision: bool,
    pub supports_reasoning: bool,
    pub tokenizer: TokenizerHint,
    pub cost_in: f64,
    pub cost_out: f64,
}
```

- Unknown models get conservative defaults; never assume 128k.
- Context window drives compaction thresholds.
- Token counting: use provider `usage` when present; otherwise a conservative
  `tiktoken-rs` estimate. Never undercount tool schemas.

### 1.4 Provider modes

Port `llm/provider_modes.py` exactly:

| Mode | Orchestrator | Workspace | Specialist |
|---|---|---|---|
| `free` | free pool, no reasoning bias | free pool, reasoning bias | free pool |
| `deepseek` | `deepseek-v4.1-flash`, thinking off | `deepseek-v4.1-flash`, thinking high on iter-1/trouble | free pool |
| `opencode` | same matrix, OpenCode gateway | same | free pool |
| `command` | `deepseek/deepseek-v4.1-flash`, thinking high | `deepseek/deepseek-v4.1-flash` | `poolside/laguna-s-2.1-free` |

Resolution precedence: env override > per-agent pin (`AGENT_MODELS`) > tier
default > registry spec. Keep this in one `provider_modes.rs` table.

### 1.5 Fallback pool

Port the behavior, not the 70k-line provider list:

- A `FallbackProvider` wraps an ordered list of `(provider, model)`.
- On `RateLimited`/`Auth`/`Upstream`, advance to the next candidate.
- Track per-provider health and rate-limit windows in memory.
- Never fall back after a tool call has begun; fall back only before the turn.
- Preserve the final error if all candidates fail.

---

## 2. Agent loop (`stark-core`)

### 2.1 Loop

```rust
pub async fn run(&self, ctx: RunCtx, mut history: Vec<Message>) -> Result<RunOutcome> {
    let mut step = 0;
    loop {
        step += 1;
        if ctx.cancel.is_cancelled() { return Ok(RunOutcome::Cancelled); }
        self.drain_steering(&ctx, &mut history);

        let tools = self.registry.for_run(&ctx);
        let specs = tools.iter().map(|t| t.spec().clone()).collect::<Vec<_>>();
        let mut stream = self.provider.stream_chat(
            &ctx.model, &history, &specs, &ctx.settings, &ctx.cancel,
        ).await?;

        let mut assistant = AssistantTurn::default();
        while let Some(event) = stream.next().await {
            match event? {
                ProviderEvent::TextDelta(t) => {
                    assistant.text.push_str(&t);
                    ctx.emit(Delta::text(t));
                }
                ProviderEvent::ReasoningDelta(t) => {
                    assistant.reasoning.push_str(&t);
                    ctx.emit(Delta::reasoning(t));
                }
                ProviderEvent::ToolCallDelta { .. } => assistant.push_tool_delta(..),
                ProviderEvent::Usage(u) => ctx.account(u),
                ProviderEvent::Done { stop_reason } => assistant.stop = stop_reason,
                ProviderEvent::Error(e) => return Err(e.into()),
            }
        }

        history.push(assistant.to_message());

        if assistant.tool_calls.is_empty() {
            return Ok(RunOutcome::Done(assistant));
        }

        let results = self.dispatch_tools(&ctx, &assistant.tool_calls).await;
        for (call, result) in assistant.tool_calls.iter().zip(results) {
            history.push(Message::tool_result(call.id.clone(), result));
        }

        if self.should_compact(&history, &ctx) {
            history = self.compact(&ctx, history).await?;
        }

        if step >= ctx.max_steps {
            return Ok(RunOutcome::StepLimit);
        }
    }
}
```

### 2.2 Tool dispatch

- Tool calls in one turn may run in parallel **only** if every call's spec is
  `parallel_safe`; otherwise execute in order.
- Each call goes through the dispatcher (policy → approval → execute).
- A denied/deferred call becomes a tool result (`[BLOCKED] ...`), not a loop
  error.
- Tool retries: invalid args can be retried by the model; a tool failure is
  returned as `[ERR]` content.
- Per-tool timeout from the spec/env; cancellation cancels the tool future.

### 2.3 Streaming

Emit `Delta` as Codex streams:
- `text` — assistant text
- `reasoning` — thinking/reasoning text (provider-specific)
- `usage` — token/cost update
- `tool_started` — name + JSON args
- `tool_finished` — ok + duration
- `end` — turn finished

The UI already parses these; do not add/rename kinds without a frontend change.

### 2.4 Cancellation and steering

- `CancellationToken` is checked before each provider call, between stream
  chunks, and before each tool call.
- `POST /api/abort` cancels the token; the final state is still persisted.
- Steering messages are drained between turns (and optionally injected as a
  user message). Do not interrupt an in-flight provider request.
- Follow-ups queue the next turn on the same chat/run.

### 2.5 Retries and limits

Port env knobs:
- `PAI_TOOL_RETRIES` (default 3)
- `PAI_OUTPUT_RETRIES` (default 1)
- `PAI_USE_CONCURRENCY` / max concurrency
- Per-run max steps (registry/spec)

Retries apply to invalid tool args and structured-output violations, not to
policy denials or cancellations.

---

## 3. Prompts and history

### 3.1 System prompt

Assemble in a fixed order:
1. Role prompt module (`agents/*_prompt.py` content, ported to `stark-agents`).
2. Workspace rules/profile (`server/routers/workspaces.py`).
3. Plan block (if a plan exists and is not completed).
4. Specialist/subagent instructions.
5. Tool-usage guidance (compact docs from `_schemas.py`).

Prompts are text assets; keep them in `stark-agents/src/prompts/*.md` or
`include_str!` files so they can be reviewed like code.

### 3.2 Message model

Mirror the JSONL/session model:

```rust
pub enum ContentBlock {
    Text { text: String },
    Thinking { thinking: String },
    ToolCall { id: String, name: String, arguments: serde_json::Value },
    Image { data: String, mime_type: String },
}

pub struct Message {
    pub role: Role, // user | assistant | toolResult | custom
    pub content: Vec<ContentBlock>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub usage: Option<Usage>,
    pub stop_reason: Option<StopReason>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub is_error: bool,
}
```

Serialization must match `server/session/log.py` entries (camelCase where the
JSONL uses it). A Rust-written session must be readable by Python and vice
versa.

### 3.3 Compaction

Port `memory/compaction_engine.py` + `llm/compaction_native.py`:

- Trigger at a percentage of the model window.
- Keep the system prompt and the most recent complete turns.
- Never split a tool call from its result.
- Summarize with a dedicated agent call; persist a `compaction` entry with
  `retainedTail` + `first_kept_entry_id`.
- Rebuild context by starting at the compaction checkpoint.
- Head/tail token budgets from the registry spec (`compaction` overrides).

### 3.4 Sessions

- One JSONL per chat under `.deep_agents/sessions/{kind}/{ws}/{chat}.jsonl`.
- Header line `session`; entries with `id` + `parentId`.
- LLM history is **derived**, never stored separately.
- Appends are atomic (open-append-write-flush) under a per-chat lock.
- Child chats get their own file with `parent_session` and delegation depth.

---

## 4. Agent registry (`stark-agents`)

Port `agents/workspace_registry.py` + `agents/specialists/*.json`.

```rust
pub struct AgentSpec {
    pub key: String,
    pub display: String,
    pub runtime: Runtime, // Direct, Graph, Subagent
    pub tier: Tier,       // Orchestrator, Workspace, Specialist, Subagent
    pub provider_tier: ProviderTier,
    pub tools: Vec<String>,
    pub mcp_scopes: Vec<String>,
    pub model: Option<String>,
    pub thinking: ThinkingProfile,
    pub compaction: CompactionOverrides,
    pub caps: HashMap<String, usize>,
    pub prompt: String,
    pub aliases: Vec<String>,
    pub delegatable: bool,
    pub history_ns: String,
}
```

Workspace agents are code entries; specialists are JSON files. Consumers
(tool schemas, delegation, provider modes, plans) read the registry; no
hardcoded name lists.

### 4.1 Subagents

- Self-contained job description: goal, constraints, paths, verification.
- Small prompt, chosen tool subset, no parent history.
- `run` blocks; `spawn` returns an ID; `status` reports.
- Result is a tool result to the spawner.

---

## 5. Runtime wiring

```rust
pub struct AgentRuntime {
    pub providers: Arc<ProviderRegistry>,
    pub tools: Arc<ToolRegistry>,
    pub policy: Arc<PolicyEngine>,
    pub store: Arc<Store>,
    pub events: Arc<EventBus>,
    pub models: Arc<ModelRegistry>,
}
```

`RunCtx` is built once per run from the chat/workspace/registry and passed
immutably. This replaces `WorkspaceDeps` + `shared_state` reads.

---

## 6. Parity acceptance

- A recorded Python run replays through Rust with the same delta sequence
  (text may differ only by provider nondeterminism; use a deterministic
  mock provider for the test).
- History round-trips Python ↔ Rust (load a Python JSONL in Rust, append,
  reload in Python).
- Compaction keeps every tool call paired and stays under the window.
- Abort stops within one stream chunk; final state is persisted.
- Steering is applied on the next turn, not mid-request.
- Token/cost totals match provider usage within a documented tolerance.
