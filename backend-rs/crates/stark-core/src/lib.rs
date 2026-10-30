use std::collections::{BTreeMap, HashMap, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde_json::{Value, json};
use stark_contract::{AppState, Delta, ServerEvent};
use stark_providers::{
    ChatMessage, ImagePart, Provider, ProviderEvent, ProviderSession, ToolCall, pricing_for,
};
use stark_store::{AgentStore, ChatStore, StoreError};
use stark_tools::{Checkpoint, PlanStore, QuestionBroker, ToolContext, ToolOutcome};
use thiserror::Error;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

pub mod agents;
mod agents_tools;
pub mod canvas;
mod codex_agents;
mod compaction;
mod deepseek_prompt;
mod delegation;
mod groups;
mod instructions;
pub mod memory_agent;
mod notes;
mod projects;
mod schedules;
pub mod worktree;
pub use agents::{
    AgentSpec, FileAccess, default_access, for_scope as agent_for_scope, resolve_agent,
};

pub type JobDoneHook = Arc<dyn Fn(String) + Send + Sync>;

const HISTORY_LIMIT: usize = 40;
const DEFAULT_RUN_SECONDS: u64 = 3_600;
const DEFAULT_RUN_TOKENS: u64 = 4_000_000;
const DEFAULT_RUN_TOOL_CALLS: usize = 500;
const REPEAT_READ_LIMIT: usize = 4;
const REPEAT_WRITE_LIMIT: usize = 5;
const MAX_CONSECUTIVE_ERRORS: usize = 5;

static AGENT_CANCELS: OnceLock<Mutex<HashMap<String, CancellationToken>>> = OnceLock::new();

fn agent_cancels() -> &'static Mutex<HashMap<String, CancellationToken>> {
    AGENT_CANCELS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn register_agent_cancel(agent_id: &str, token: CancellationToken) {
    if let Ok(mut cancels) = agent_cancels().lock() {
        cancels.insert(agent_id.to_string(), token);
    }
}

pub fn unregister_agent_cancel(agent_id: &str) {
    if let Ok(mut cancels) = agent_cancels().lock() {
        cancels.remove(agent_id);
    }
}

pub fn cancel_agent_run(agent_id: &str) -> bool {
    agent_cancels()
        .lock()
        .ok()
        .and_then(|cancels| cancels.get(agent_id).cloned())
        .map(|token| {
            token.cancel();
            true
        })
        .unwrap_or(false)
}

#[derive(Debug, Error)]
pub enum RunError {
    #[error(transparent)]
    Provider(#[from] stark_providers::ProviderError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("{0}")]
    Message(String),
}

pub struct RunContext {
    pub chat_id: String,
    pub workspace_id: String,
    pub workspace: PathBuf,
    pub role: Option<String>,
    pub scope: String,
    pub depth: u32,
    pub file_access: FileAccess,
    pub model: String,
    pub temperature: f64,
    /// Images attached to this run (uploads, screen frames). Sent once with
    /// the opening user message; never persisted into chat history.
    pub images: Vec<ImagePart>,
    pub provider: Provider,
    pub store: ChatStore,
    pub questions: Arc<QuestionBroker>,
    pub tx: broadcast::Sender<ServerEvent>,
    pub latest: Arc<Mutex<Option<AppState>>>,
    pub steering: Arc<Mutex<VecDeque<String>>>,
    pub follow_up: Arc<Mutex<VecDeque<String>>>,
    pub checkpoint: Option<Arc<Checkpoint>>,
    pub cancel: CancellationToken,
    pub on_job_done: Option<JobDoneHook>,
}

impl RunContext {
    fn emit(&self, event: impl Into<String>, data: impl serde::Serialize) {
        let _ = self.tx.send(ServerEvent::new(event, data));
    }

    fn emit_delta(&self, delta: Delta) {
        self.emit("delta", delta);
    }

    fn emit_state(&self, state: &mut AppState) {
        state.state_version = state.state_version.saturating_add(1);
        if let Ok(mut latest) = self.latest.lock() {
            *latest = Some(state.clone());
        }
        self.emit("state", state.clone());
    }

    fn agent_label(&self) -> String {
        agent_for_scope(&self.scope)
            .map(|agent| agent.display.to_string())
            .or_else(|| self.role.clone())
            .unwrap_or_else(|| "Stark".to_string())
    }

    /// Provider cache/routing affinity: a child chat shares its parent's
    /// affinity, so opencode-go can reuse the parent's prompt cache.
    pub async fn affinity(&self) -> String {
        self.store
            .chat_affinity(&self.chat_id)
            .await
            .unwrap_or_else(|_| self.chat_id.clone())
    }
}

fn session_messages(entries: &[Value]) -> Vec<ChatMessage> {
    let start = entries.len().saturating_sub(80);
    let mut messages = Vec::new();
    if start > 0
        && let Some(first) = entries.first()
    {
        let role = first.get("role").and_then(Value::as_str).unwrap_or("");
        if role == "system" {
            let summary = first.get("content").map(content_text).unwrap_or_default();
            if !summary.is_empty() {
                messages.push(ChatMessage::system(summary));
            }
        } else if role == "user" {
            let checkpoint = first.get("content").map(content_text).unwrap_or_default();
            if checkpoint
                .trim_start()
                .starts_with("<conversation-checkpoint>")
            {
                messages.push(ChatMessage::user(checkpoint));
            }
        }
    }
    for entry in &entries[start..] {
        let role = entry.get("role").and_then(Value::as_str).unwrap_or("");
        if role == "system" {
            let content = entry.get("content").map(content_text).unwrap_or_default();
            if !content.is_empty() {
                messages.push(ChatMessage::system(content));
            }
            continue;
        }
        if role == "toolResult" {
            let call_id = entry
                .get("tool_call_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            let content = entry.get("content").map(content_text).unwrap_or_default();
            messages.push(ChatMessage::tool(call_id, content));
            continue;
        }
        if role != "user" && role != "assistant" {
            continue;
        }
        let content = entry.get("content").cloned().unwrap_or(Value::Null);
        if role == "user" {
            messages.push(ChatMessage::user(content_text(&content)));
            continue;
        }
        if let Some(blocks) = content.as_array() {
            let mut text = String::new();
            let mut calls = Vec::new();
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        text.push_str(block.get("text").and_then(Value::as_str).unwrap_or(""))
                    }
                    Some("toolCall") => {
                        let id = block.get("id").and_then(Value::as_str).unwrap_or("");
                        let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                        let arguments =
                            block.get("arguments").cloned().unwrap_or_else(|| json!({}));
                        calls.push(ToolCall::function(
                            id,
                            name,
                            serde_json::to_string(&arguments).unwrap_or_default(),
                        ));
                    }
                    _ => {}
                }
            }
            if calls.is_empty() {
                if !text.is_empty() {
                    messages.push(ChatMessage::assistant(text));
                }
            } else {
                messages.push(ChatMessage::assistant_tool_calls(
                    (!text.is_empty()).then_some(text),
                    calls,
                ));
            }
        } else {
            messages.push(ChatMessage::assistant(content_text(&content)));
        }
    }
    messages
}

fn content_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

fn is_internal_delegation_result(prompt: &str) -> bool {
    prompt.trim_start().starts_with("[DELEGATION RESULT]")
}

async fn run_agent_inner(ctx: RunContext, prompt: String) -> Result<(), RunError> {
    let history = ctx
        .store
        .history(&ctx.chat_id, HISTORY_LIMIT)
        .await
        .unwrap_or_default();
    let session_entries = ctx
        .store
        .read_session_entries(&ctx.workspace_id, &ctx.workspace, &ctx.chat_id)
        .await
        .unwrap_or_default();

    ctx.store
        .ensure_chat(
            &ctx.chat_id,
            &ctx.workspace_id,
            ctx.workspace.to_string_lossy().as_ref(),
            &ctx.model,
            &prompt,
        )
        .await?;
    let plan_store = PlanStore::new(ctx.store.harness_root().to_path_buf());
    let mode = ctx
        .store
        .chat_mode(&ctx.chat_id)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "build".to_string());
    let initial_plan = plan_store
        .plan_state(&ctx.chat_id)
        .await
        .unwrap_or_default();
    if !is_internal_delegation_result(&prompt) {
        let user_message_id = ctx
            .store
            .append_message(&ctx.chat_id, "user", &prompt, false)
            .await?;
        if let Some(checkpoint) = &ctx.checkpoint {
            checkpoint.set_user_message(&user_message_id);
        }
        ctx.store
            .append_session_entry_for(
                &ctx.workspace_id,
                &ctx.workspace,
                &ctx.chat_id,
                json!({"type": "message", "role": "user", "content": prompt.clone()}),
            )
            .await?;
    }

    let workspace_label = ctx.workspace.to_string_lossy().to_string();
    let role = ctx.role.as_deref().unwrap_or("a coding agent");
    let system_prompt = if ctx.scope == "developer" || ctx.scope == "coder" {
        // Developer: OpenCode-shaped prompt, stable content first and the
        // volatile runtime snapshot last so prefix-cache reuse survives plan
        // switches. Memory/worktree blocks are intentionally absent.
        // Coder shares the shape and adds the program-tool-calling section.
        deepseek_prompt::system_prompt(
            &ctx.workspace,
            ctx.store.harness_root(),
            &ctx.model,
            &mode,
            ctx.depth,
            ctx.scope == "coder",
        )
        .await
    } else {
        let preamble = match agent_for_scope(&ctx.scope) {
            Some(agent) => agent.instructions.to_string(),
            None => format!("You are Stark, {role}."),
        };
        let mut prompt = format!(
            "{preamble} Working in {workspace_label}. \
             Use the provided tools to inspect and change files. Read before editing. \
             Never invent tool results. Keep the final answer concise."
        );
        if ctx.file_access.is_read_only() {
            prompt.push_str(
                " You are READ-ONLY in this task. Do not edit files, run mutating commands, or call write tools. \
                 Inspect with read_file/search_code/memory and report findings or a proposed patch.",
            );
        }
        if let Some(instructions) = instructions::load(ctx.store.harness_root(), &ctx.workspace) {
            prompt.push_str("\n\n");
            prompt.push_str(&instructions);
        }
        if stark_store::memory_enabled()
            && let Ok(memory) = stark_store::MemoryStore::open(ctx.store.harness_root()).await
            && let Some(index) = memory
                .index(&ctx.workspace_id, stark_store::memory_max_chars())
                .await
        {
            prompt.push_str(
                "

[WORKSPACE MEMORY]
",
            );
            prompt.push_str(&index);
            prompt.push_str(
                "
Memory is a hint, not the source of truth. Verify current code.              Use the memory tool (action=\"read\") for topic details.",
            );
        }
        if mode == "plan" {
            prompt.push_str(
                " You are in PLAN mode: inspect and plan only. Use update_plan to maintain the plan. \
                 Do not edit files, spawn subagents, or run commands with side effects.",
            );
        }
        prompt
    };
    let mut messages = vec![ChatMessage::system(system_prompt)];
    if session_entries.is_empty() {
        for message in history {
            if message.role == "user" || message.role == "assistant" {
                messages.push(ChatMessage {
                    role: message.role,
                    content: Some(message.content),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                    images: Vec::new(),
                });
            }
        }
    } else {
        messages.extend(session_messages(&session_entries));
    }
    if ctx.scope != "developer"
        && ctx.scope != "coder"
        && let Ok(block) = plan_store.progress_block(&ctx.chat_id, &ctx.scope).await
        && !block.is_empty()
    {
        messages.push(ChatMessage::user(block));
    }
    if ctx.images.is_empty() {
        messages.push(ChatMessage::user(&prompt));
    } else {
        messages.push(ChatMessage::user_with_images(&prompt, ctx.images.clone()));
    }
    let affinity = ctx.affinity().await;
    maybe_compact(&ctx, &mut messages, &affinity).await;

    let mut state = AppState {
        active_node: "agent".to_string(),
        next_agent: String::new(),
        client_request: prompt.clone(),
        project_path: workspace_label.clone(),
        selected_model: ctx.model.clone(),
        selected_temp: ctx.temperature,
        outputs: json!({}),
        plan: (!initial_plan.is_empty()).then_some(initial_plan),
        ..AppState::default()
    };
    ctx.emit_state(&mut state);

    let tool_specs =
        stark_tools::tool_specs_for_scope_with_access(&mode, &ctx.scope, ctx.file_access.as_str());
    let agent_label = ctx.agent_label();
    let mut tools = ToolContext::new(
        ctx.workspace.clone(),
        ctx.chat_id.clone(),
        ctx.questions.clone(),
    )
    .with_plan_store(Arc::new(plan_store))
    .with_harness_root(ctx.store.harness_root().to_path_buf())
    .with_workspace_id(ctx.workspace_id.clone())
    .with_scope(ctx.scope.clone())
    .with_access(ctx.file_access.as_str())
    .with_mode(mode.clone())
    .with_depth(ctx.depth);
    if let Some(checkpoint) = &ctx.checkpoint {
        tools = tools.with_checkpoint(checkpoint.clone());
    }
    {
        let tx = ctx.tx.clone();
        let agent = agent_label.clone();
        tools = tools.with_code_delta(Arc::new(move |delta| {
            let _ = tx.send(ServerEvent::new("delta", delta.with_agent(agent.clone())));
        }));
    }
    let mut last_continuous_screen: Option<Instant> = None;
    let (max_wall, max_tokens, max_tool_calls) = run_limits();
    let run_started = Instant::now();
    let mut tool_calls_used = 0usize;
    let mut repeat_counts: HashMap<(String, String), usize> = HashMap::new();
    let mut consecutive_errors = 0usize;

    loop {
        if ctx.cancel.is_cancelled() {
            finish_aborted(&ctx, &mut state).await;
            return Ok(());
        }
        if run_started.elapsed() >= max_wall {
            let message = format!(
                "Run stopped: wall-clock budget of {} seconds exhausted.",
                max_wall.as_secs()
            );
            fail(&ctx, &mut state, message.clone()).await;
            return Err(RunError::Message(message));
        }
        let used_tokens = state
            .token_usage
            .total_input_tokens
            .saturating_add(state.token_usage.total_output_tokens);
        if used_tokens >= max_tokens {
            let message = format!("Run stopped: token budget of {max_tokens} exhausted.");
            fail(&ctx, &mut state, message.clone()).await;
            return Err(RunError::Message(message));
        }
        if tool_calls_used >= max_tool_calls {
            let message = format!("Run stopped: tool-call budget of {max_tool_calls} exhausted.");
            fail(&ctx, &mut state, message.clone()).await;
            return Err(RunError::Message(message));
        }

        if let Some(message) = ctx.steering.lock().unwrap().pop_front() {
            messages.push(ChatMessage::user(&message));
        }

        if last_continuous_screen.is_none_or(|last| last.elapsed() >= Duration::from_secs(15))
            && let Some(image) = stark_tools::latest_continuous_image()
        {
            messages.push(ChatMessage::user_with_images(
                "[Live screen (continuous share)]",
                vec![ImagePart {
                    url: image.url,
                    note: image.note,
                }],
            ));
            last_continuous_screen = Some(Instant::now());
        }

        let mut stream = match ctx
            .provider
            .stream_chat_for_session(
                &messages,
                &tool_specs,
                ctx.temperature,
                ProviderSession {
                    session_id: &ctx.chat_id,
                    affinity: &affinity,
                },
            )
            .await
        {
            Ok(stream) => stream,
            Err(error) => {
                fail(&ctx, &mut state, error.to_string()).await;
                return Err(RunError::Provider(error));
            }
        };

        let mut text = String::new();
        let mut reasoning = String::new();
        let mut calls: BTreeMap<usize, PartialCall> = BTreeMap::new();
        let mut finish_reason: Option<String> = None;

        loop {
            let event = tokio::select! {
                _ = ctx.cancel.cancelled() => {
                    finish_aborted(&ctx, &mut state).await;
                    return Ok(());
                }
                event = stream.next() => event,
            };
            let Some(event) = event else {
                break;
            };
            match event {
                Ok(ProviderEvent::Text(chunk)) => {
                    text.push_str(&chunk);
                    ctx.emit_delta(Delta::text(chunk).with_agent(agent_label.clone()));
                }
                Ok(ProviderEvent::Reasoning(chunk)) => {
                    reasoning.push_str(&chunk);
                    ctx.emit_delta(Delta::reasoning(chunk).with_agent(agent_label.clone()));
                }
                Ok(ProviderEvent::Usage(usage)) => {
                    let cached = usage.cached_tokens.min(usage.input_tokens);
                    let miss = usage.input_tokens.saturating_sub(cached);
                    let pricing = pricing_for(&ctx.model);
                    let cost = if usage.cost > 0.0 {
                        usage.cost
                    } else {
                        pricing.cost(&usage)
                    };
                    state.token_usage.total_input_tokens += usage.input_tokens;
                    state.token_usage.total_output_tokens += usage.output_tokens;
                    state.token_usage.total_cache_hit_tokens += cached;
                    state.token_usage.total_cache_miss_tokens += miss;
                    state.token_usage.total_cost += cost;
                    let call = json!({
                        "agent": "Supervisor",
                        "model": ctx.model,
                        "input": usage.input_tokens,
                        "output": usage.output_tokens,
                        "cache_hits": cached,
                        "cache_misses": miss,
                        "input_tokens": usage.input_tokens,
                        "cached_tokens": cached,
                        "cache_miss_tokens": miss,
                        "output_tokens": usage.output_tokens,
                        "reasoning_tokens": usage.reasoning_tokens,
                        "total_tokens": usage.total_tokens,
                        "cost": cost,
                        "timestamp": chrono::Utc::now().to_rfc3339(),
                        "where": agent_label,
                    });
                    state.token_usage.calls.push(call.clone());
                    ctx.emit_delta(Delta::usage(call.to_string()).with_agent(agent_label.clone()));
                }
                Ok(ProviderEvent::ToolCall {
                    index,
                    id,
                    name,
                    arguments,
                }) => {
                    let call = calls.entry(index).or_default();
                    if let Some(id) = id {
                        call.id = Some(id);
                    }
                    if let Some(name) = name {
                        call.name = Some(name);
                    }
                    if let Some(arguments) = arguments {
                        call.arguments.push_str(&arguments);
                    }
                }
                Ok(ProviderEvent::Done {
                    finish_reason: reason,
                }) => {
                    finish_reason = reason;
                    break;
                }
                Err(error) => {
                    fail(&ctx, &mut state, error.to_string()).await;
                    return Err(RunError::Provider(error));
                }
            }
        }

        if ctx.cancel.is_cancelled() {
            finish_aborted(&ctx, &mut state).await;
            return Ok(());
        }

        if calls.is_empty() {
            if !text.is_empty() {
                ctx.store
                    .append_message(&ctx.chat_id, "assistant", &text, false)
                    .await?;
                ctx.store
                    .append_session_entry_for(
                        &ctx.workspace_id,
                        &ctx.workspace,
                        &ctx.chat_id,
                        json!({
                            "type": "message",
                            "role": "assistant",
                            "content": text.clone(),
                            "provider": "stark-rs",
                            "model": ctx.model.clone(),
                            "stop_reason": finish_reason,
                        }),
                    )
                    .await?;
            }
            let next_follow_up = ctx.follow_up.lock().unwrap().pop_front();
            if let Some(follow_up) = next_follow_up {
                if !text.is_empty() {
                    messages.push(ChatMessage::assistant(&text));
                }
                if !is_internal_delegation_result(&follow_up) {
                    let user_id = ctx
                        .store
                        .append_message(&ctx.chat_id, "user", &follow_up, false)
                        .await?;
                    ctx.store
                        .append_session_entry_for(
                            &ctx.workspace_id,
                            &ctx.workspace,
                            &ctx.chat_id,
                            json!({"type": "message", "role": "user", "content": follow_up}),
                        )
                        .await?;
                    let _ = user_id;
                }
                messages.push(ChatMessage::user(&follow_up));
                state.active_node = "agent".to_string();
                state.outputs = json!({});
                state.state_version += 1;
                ctx.emit_state(&mut state);
                continue;
            }
            state.active_node = "done".to_string();
            state.outputs = json!({"agent_report": text});
            ctx.emit_state(&mut state);
            if ctx.depth <= 1
                && ctx.scope != "developer"
                && stark_store::memory_enabled()
                && let Ok(memory) = stark_store::MemoryStore::open(ctx.store.harness_root()).await
            {
                let _ = memory
                    .record(&ctx.workspace_id, &ctx.chat_id, None, &prompt, &text)
                    .await;
                // Distillation/consolidation is background work. The
                // deterministic stage-1 record above is the durable capture;
                // the model pass only decides what deserves long-term memory.
                if !cfg!(test) {
                    let provider = crate::memory_agent::provider_for_memory(&ctx.provider);
                    let cancel = ctx.cancel.clone();
                    let memory = memory.clone();
                    let workspace_id = ctx.workspace_id.clone();
                    tokio::spawn(async move {
                        if let Err(error) = crate::memory_agent::maybe_consolidate(
                            &provider,
                            &cancel,
                            &memory,
                            &workspace_id,
                        )
                        .await
                        {
                            tracing::warn!("background memory consolidation failed: {error}");
                        }
                    });
                }
            }
            return Ok(());
        }

        let tool_calls: Vec<ToolCall> = calls
            .into_iter()
            .map(|(index, call)| {
                ToolCall::function(
                    call.id.unwrap_or_else(|| format!("call_{index}")),
                    call.name.unwrap_or_default(),
                    call.arguments,
                )
            })
            .collect();

        messages.push(ChatMessage::assistant_tool_calls(
            if text.is_empty() {
                None
            } else {
                Some(text.clone())
            },
            tool_calls.clone(),
        ));

        let mut assistant_blocks = Vec::new();
        if !text.is_empty() {
            assistant_blocks.push(json!({"type": "text", "text": text}));
        }
        for call in &tool_calls {
            let arguments = serde_json::from_str::<Value>(&call.function.arguments)
                .unwrap_or_else(|_| json!({}));
            assistant_blocks.push(json!({
                "type": "toolCall",
                "id": call.id,
                "name": call.function.name,
                "arguments": arguments,
            }));
        }
        let _ = ctx
            .store
            .append_session_entry_for(
                &ctx.workspace_id,
                &ctx.workspace,
                &ctx.chat_id,
                json!({
                    "type": "message",
                    "role": "assistant",
                    "content": assistant_blocks,
                    "provider": "stark-rs",
                    "model": ctx.model.clone(),
                    "stop_reason": "toolUse",
                }),
            )
            .await;

        // A response cut by the output limit must never execute its tool
        // calls: the arguments are a partial payload (this silently truncated
        // a heredoc once). Answer every call id with a clear retry
        // instruction so history stays valid and the model splits the work.
        if !tool_calls.is_empty()
            && matches!(
                finish_reason.as_deref(),
                Some("length") | Some("max_tokens")
            )
        {
            let notice = "[ERROR] Your response hit the output token limit mid-tool-call, so nothing was executed. Retry with a smaller payload: split large files or heredocs into several write/edit/append calls.";
            for call in &tool_calls {
                messages.push(ChatMessage::tool(&call.id, notice));
                let _ = ctx
                    .store
                    .append_session_entry_for(
                        &ctx.workspace_id,
                        &ctx.workspace,
                        &ctx.chat_id,
                        json!({
                            "type": "message",
                            "role": "toolResult",
                            "tool_call_id": call.id,
                            "tool_name": call.function.name,
                            "is_error": true,
                            "content": notice,
                        }),
                    )
                    .await;
                ctx.emit_delta(
                    Delta::tool_finished(&call.function.name, "{}", false, 0, notice)
                        .with_agent(agent_label.clone()),
                );
            }
            continue;
        }

        for call in tool_calls {
            if ctx.cancel.is_cancelled() {
                finish_aborted(&ctx, &mut state).await;
                return Ok(());
            }

            let args: Value =
                serde_json::from_str(&call.function.arguments).unwrap_or_else(|_| json!({}));
            let args_text = serde_json::to_string(&args).unwrap_or_default();
            ctx.emit_delta(
                Delta::tool_started(&call.function.name, &args_text)
                    .with_agent(agent_label.clone()),
            );

            let started = Instant::now();
            let outcome = if call.function.name == "call_subagent" {
                if let Some(reason) =
                    stark_tools::plan_mode_denial(&mode, &call.function.name, &args)
                {
                    ToolOutcome::err(reason)
                } else {
                    run_subagent_tool(&ctx, &args).await
                }
            } else if matches!(
                call.function.name.as_str(),
                "spawn_agent"
                    | "message_agent"
                    | "wait_agent"
                    | "list_agents"
                    | "interrupt_agent"
                    | "close_agent"
            ) {
                if let Some(reason) =
                    stark_tools::plan_mode_denial(&mode, &call.function.name, &args)
                {
                    ToolOutcome::err(reason)
                } else {
                    codex_agents::handle(&ctx, &call.function.name, &args).await
                }
            } else if call.function.name == "create_coding_project" {
                if let Some(reason) =
                    stark_tools::plan_mode_denial(&mode, &call.function.name, &args)
                {
                    ToolOutcome::err(reason)
                } else {
                    projects::create_coding_project(&ctx, &args).await
                }
            } else if call.function.name == "delegate" {
                if let Some(reason) =
                    stark_tools::plan_mode_denial(&mode, &call.function.name, &args)
                {
                    ToolOutcome::err(reason)
                } else {
                    delegation::delegate(&ctx, &args).await
                }
            } else if call.function.name == "delegation_status" {
                delegation::delegation_status(&ctx, &args).await
            } else if call.function.name == "job_message" {
                delegation::job_message(&ctx, &args).await
            } else if call.function.name == "job_cancel" {
                delegation::job_cancel(&ctx, &args).await
            } else if call.function.name == "create_group" {
                if let Some(reason) =
                    stark_tools::plan_mode_denial(&mode, &call.function.name, &args)
                {
                    ToolOutcome::err(reason)
                } else {
                    groups::create_group(&ctx, &args).await
                }
            } else if call.function.name == "group_board" {
                groups::group_board(&ctx, &args).await
            } else if call.function.name == "group_item_detail" {
                groups::group_item_detail(&ctx, &args).await
            } else if call.function.name == "group_events" {
                groups::group_events(&ctx, &args).await
            } else if call.function.name == "group_update" {
                groups::group_update(&ctx, &args).await
            } else if call.function.name == "schedule" {
                schedules::schedule(&ctx, &args).await
            } else if call.function.name == "give_note" {
                notes::give_note(&ctx, &args).await
            } else if call.function.name == "canvas_mode" {
                canvas::handle(&ctx, &args).await
            } else if call.function.name == "agent" {
                agents_tools::handle(&ctx, &args).await
            } else if call.function.name == "ask_user_question" {
                // Plan-review approval (replaces exit_plan_mode): when the
                // tool result carries [PLAN APPROVED], flip plan -> build.
                let outcome = stark_tools::dispatch(&call.function.name, &args, &tools).await;
                maybe_flip_plan_mode(&ctx, &mode, &outcome).await;
                outcome
            } else {
                stark_tools::dispatch(&call.function.name, &args, &tools).await
            };
            let elapsed = started.elapsed().as_millis() as u64;
            let preview: String = outcome.content.chars().take(300).collect();
            ctx.emit_delta(
                Delta::tool_finished(
                    &call.function.name,
                    &args_text,
                    !outcome.is_error,
                    elapsed,
                    preview,
                )
                .with_agent(agent_label.clone())
                .with_children(outcome.children.clone()),
            );
            if let Some(plan) = &outcome.plan {
                state.plan = Some(plan.clone());
                ctx.emit_state(&mut state);
            }

            messages.push(ChatMessage::tool(&call.id, &outcome.content));
            let _ = ctx
                .store
                .append_session_entry_for(
                    &ctx.workspace_id,
                    &ctx.workspace,
                    &ctx.chat_id,
                    json!({
                        "type": "message",
                        "role": "toolResult",
                        "tool_call_id": call.id,
                        "tool_name": call.function.name,
                        "is_error": outcome.is_error,
                        "content": outcome.content,
                    }),
                )
                .await;

            tool_calls_used += 1;
            if outcome.is_error {
                consecutive_errors += 1;
            } else {
                consecutive_errors = 0;
            }
            let repeat_key = (call.function.name.clone(), args_text.clone());
            let repeat_count = repeat_counts.entry(repeat_key).or_insert(0);
            *repeat_count += 1;
            let repeat_limit = if is_read_only_tool(&call.function.name) {
                REPEAT_READ_LIMIT
            } else {
                REPEAT_WRITE_LIMIT
            };
            if *repeat_count >= repeat_limit {
                let message = format!(
                    "Run stopped: {} was called {} times with identical arguments. Use the previous result or change the approach.",
                    call.function.name, repeat_count
                );
                fail(&ctx, &mut state, message.clone()).await;
                return Err(RunError::Message(message));
            }
            if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                let message =
                    format!("Run stopped: {MAX_CONSECUTIVE_ERRORS} consecutive tool errors.");
                fail(&ctx, &mut state, message.clone()).await;
                return Err(RunError::Message(message));
            }
        }

        let images = stark_tools::drain_images(&ctx.chat_id);
        if !images.is_empty() {
            let images = images
                .into_iter()
                .map(|image| ImagePart {
                    url: image.url,
                    note: image.note,
                })
                .collect();
            messages.push(ChatMessage::user_with_images(
                "[Inline image(s) from tool result]",
                images,
            ));
        }

        if !reasoning.is_empty() {
            state
                .thoughts
                .insert(format!("step-{}", state.state_version), reasoning);
        }
        maybe_compact(&ctx, &mut messages, &affinity).await;
    }
}

/// Plan-review approval (replaces the exit_plan_mode tool): when an
/// ask_user_question result carries [PLAN APPROVED], flip the chat from plan
/// to build mode. Rejection keeps plan mode; the model presents again.
async fn maybe_flip_plan_mode(ctx: &RunContext, mode: &str, outcome: &ToolOutcome) {
    if mode == "plan" && !outcome.is_error && outcome.content.contains("[PLAN APPROVED]") {
        let _ = ctx.store.set_chat_mode(&ctx.chat_id, "build").await;
    }
}

pub fn run_agent(
    ctx: RunContext,
    prompt: String,
) -> Pin<Box<dyn Future<Output = Result<(), RunError>> + Send>> {
    Box::pin(run_agent_inner(ctx, prompt))
}
fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn run_limits() -> (Duration, u64, usize) {
    (
        Duration::from_secs(env_u64("STARK_RUN_MAX_SECONDS", DEFAULT_RUN_SECONDS)),
        env_u64("STARK_RUN_MAX_TOKENS", DEFAULT_RUN_TOKENS),
        env_u64("STARK_RUN_MAX_TOOL_CALLS", DEFAULT_RUN_TOOL_CALLS as u64) as usize,
    )
}

fn is_read_only_tool(name: &str) -> bool {
    matches!(
        name,
        "read_file"
            | "read"
            | "search_code"
            | "grep"
            | "glob"
            | "job"
            | "list_agents"
            | "wait_agent"
            | "web_search"
            | "web_fetch"
            | "skill"
            | "read_pdf"
            | "look_at_screen"
            | "house_get_state"
            | "house_get_device"
            | "house_get_history"
            | "delegation_status"
            | "group_board"
            | "group_item_detail"
            | "group_events"
            | "canvas_mode"
            | "files_changed"
    )
}

async fn maybe_compact(ctx: &RunContext, messages: &mut Vec<ChatMessage>, affinity: &str) {
    let policy = compaction::CompactionPolicy::from_env(&ctx.scope, &ctx.model);
    let tokens = compaction::estimate_tokens(messages);
    if tokens >= policy.hard_limit_tokens {
        tracing::warn!(
            "chat {} is above the compaction hard limit ({} tokens)",
            ctx.chat_id,
            tokens
        );
    }
    if !compaction::should_compact(messages, &policy) {
        return;
    }
    let Some(outcome) = compaction::compact_history(
        &ctx.provider,
        &ctx.cancel,
        messages,
        &policy,
        &ctx.chat_id,
        affinity,
    )
    .await
    else {
        return;
    };
    if compaction::estimate_tokens(&outcome.messages) >= policy.trigger_tokens {
        tracing::warn!(
            "chat {} compaction did not reduce the context below the trigger; keeping the previous history",
            ctx.chat_id
        );
        return;
    }
    *messages = outcome.messages;
    if let Err(error) = ctx
        .store
        .append_session_entry_for(
            &ctx.workspace_id,
            &ctx.workspace,
            &ctx.chat_id,
            json!({
                "type": "compaction",
                "checkpoint_format": "opencode-v1",
                "summary": outcome.summary,
                "retained_tail": outcome.retained_tail,
                "tokens_before": outcome.tokens_before,
                "first_kept_entry_id": null,
            }),
        )
        .await
    {
        tracing::warn!("failed to persist compaction for {}: {error}", ctx.chat_id);
    }
}

#[derive(Default)]
struct PartialCall {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

async fn finish_aborted(ctx: &RunContext, state: &mut AppState) {
    let message = "Run aborted.";
    let _ = ctx
        .store
        .append_message(&ctx.chat_id, "assistant", message, false)
        .await;
    let _ = ctx
        .store
        .append_session_entry_for(
            &ctx.workspace_id,
            &ctx.workspace,
            &ctx.chat_id,
            json!({"type": "message", "role": "assistant", "content": message, "stop_reason": "aborted"}),
        )
        .await;
    state.active_node = "aborted".to_string();
    state.error = Some(message.to_string());
    ctx.emit_state(state);
    ctx.emit_delta(Delta::end("").with_agent(ctx.agent_label()));
}

async fn fail(ctx: &RunContext, state: &mut AppState, message: String) {
    state.active_node = "error".to_string();
    state.error = Some(message.clone());
    ctx.emit_state(state);
    let _ = ctx
        .store
        .append_message(
            &ctx.chat_id,
            "assistant",
            &format!("Error: {message}"),
            false,
        )
        .await;
    ctx.emit_delta(Delta::end("").with_agent(ctx.agent_label()));
}

async fn run_subagent_tool(ctx: &RunContext, args: &Value) -> ToolOutcome {
    let store = match AgentStore::open(ctx.store.harness_root()).await {
        Ok(store) => store,
        Err(error) => return ToolOutcome::err(format!("Error: agent store unavailable: {error}")),
    };
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("run")
        .trim();
    let current = match agents_tools::current_agent(ctx, &store).await {
        Ok(agent) => agent,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };
    match action {
        "list" => {
            let agents = store.list(Some(&current.id)).await.unwrap_or_default();
            if agents.is_empty() {
                return ToolOutcome::ok("No subagent sessions.".to_string());
            }
            let rows = agents
                .iter()
                .map(|agent| {
                    format!(
                        "- {} - {} - {} (chat: {})",
                        agent.id, agent.role, agent.status, agent.chat_id
                    )
                })
                .collect::<Vec<_>>();
            ToolOutcome::ok(format!("[SUBAGENTS]\n{}", rows.join("\n")))
        }
        "status" => {
            let Some(child_id) = args.get("child_id").and_then(Value::as_str) else {
                return ToolOutcome::err("Error: action='status' requires child_id");
            };
            let wait = args
                .get("wait_seconds")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .min(300);
            let result = if wait == 0 {
                match store.get(child_id).await {
                    Ok(Some(agent)) => Ok(format_subagent(&agent)),
                    Ok(None) => Err(format!("Error: unknown subagent {child_id}")),
                    Err(error) => Err(error.to_string()),
                }
            } else {
                agents_tools::wait_for(&store, child_id, wait).await
            };
            match result {
                Ok(text) => ToolOutcome::ok(text),
                Err(error) => ToolOutcome::err(error),
            }
        }
        "run" | "spawn" => {
            if current.kind != "workspace" {
                return ToolOutcome::err("Error: only a workspace agent can spawn subagents");
            }
            if current.depth >= 2 {
                return ToolOutcome::err("Error: subagent nesting limit reached");
            }
            let name = args
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Specialist")
                .trim();
            let task = args
                .get("task")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if task.is_empty() {
                return ToolOutcome::err(format!("Error: action='{action}' requires task"));
            }
            let write_access = args
                .get("write_access")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let spec = agent_for_scope(name)
                .or_else(|| agent_for_scope("general"))
                .unwrap_or(crate::agents::GENERAL);
            let mut access = crate::default_access(spec.key, current.depth.max(0) as u32 + 1, true);
            if write_access && access.is_read_only() {
                access = crate::FileAccess::Worktree;
            }
            let title: String = args
                .get("title")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| task.chars().take(60).collect());
            // A subagent inherits the parent workspace (which may itself be a
            // worktree checkout).
            let parent_workspace = if !current.worktree_path.trim().is_empty()
                && std::path::Path::new(&current.worktree_path).exists()
            {
                let subdir = current.workspace_subdir.trim().trim_matches('/');
                if subdir.is_empty() {
                    std::path::PathBuf::from(&current.worktree_path)
                } else {
                    std::path::PathBuf::from(&current.worktree_path).join(subdir)
                }
            } else {
                ctx.workspace.clone()
            };
            let chat = match ctx
                .store
                .create_child_chat(
                    &ctx.workspace_id,
                    parent_workspace.to_string_lossy().as_ref(),
                    &title,
                    &ctx.model,
                    &current.chat_id,
                )
                .await
            {
                Ok(chat) => chat,
                Err(error) => return ToolOutcome::err(format!("Error: {error}")),
            };
            let chat_id = chat["id"].as_str().unwrap_or("").to_string();
            let short = &uuid::Uuid::new_v4().simple().to_string()[..8];
            let path = format!("{}/sub:{}-{}", current.path, safe_agent_name(name), short);
            let agent = match store
                .create_agent(
                    "subagent",
                    &path,
                    Some(&current.id),
                    &chat_id,
                    &ctx.workspace_id,
                    spec.key,
                    spec.display,
                    &title,
                    &ctx.model,
                    current.depth + 1,
                )
                .await
            {
                Ok(agent) => agent,
                Err(error) => return ToolOutcome::err(format!("Error: {error}")),
            };
            let _ = store.set_access(&agent.id, access.as_str()).await;
            let mut effective_workspace = parent_workspace.clone();
            if access == crate::FileAccess::Worktree {
                match crate::worktree::create(
                    ctx.store.harness_root(),
                    &parent_workspace,
                    &agent.id,
                ) {
                    Ok(info) => {
                        effective_workspace = crate::worktree::effective_workspace(&info);
                        let _ = ctx
                            .store
                            .set_chat_workspace_path(
                                &chat_id,
                                &effective_workspace.to_string_lossy(),
                            )
                            .await;
                        let _ = store
                            .set_worktree(
                                &agent.id,
                                &info.repo_root.to_string_lossy(),
                                &info.subdir.to_string_lossy(),
                                &info.path.to_string_lossy(),
                                &info.branch,
                                &info.base,
                                "active",
                            )
                            .await;
                    }
                    Err(error) => {
                        if crate::agents::worktree_mode() == "required" {
                            return ToolOutcome::err(format!(
                                "Error: worktree isolation required but unavailable: {error}"
                            ));
                        }
                        let _ = store.set_access(&agent.id, "write").await;
                        access = crate::FileAccess::Write;
                    }
                }
            }
            let agent = store.get(&agent.id).await.ok().flatten().unwrap_or(agent);
            if action == "spawn" {
                let _ = store
                    .enqueue(
                        Some(&current.id),
                        &agent.id,
                        "spawn",
                        task,
                        "trigger",
                        json!({"title": title}),
                    )
                    .await;
                let _ = store.wake(&agent.id).await;
                return ToolOutcome::ok(format!(
                    "[SPAWNED] child={} name={} access={}; use action=status/child_id",
                    agent.id, agent.role, agent.access
                ))
                .with_children(vec![json!({
                    "agentId": agent.id,
                    "chatId": agent.chat_id,
                    "agentName": agent.role,
                    "label": title,
                    "workspaceId": ctx.workspace_id,
                })]);
            }
            if let Err(error) = store.set_status(&agent.id, "running", true).await {
                return ToolOutcome::err(format!("Error: {error}"));
            }
            let cancel = ctx.cancel.child_token();
            let steering = Arc::new(Mutex::new(VecDeque::new()));
            let (child, latest) = child_context_with_access(
                ctx,
                &agent.chat_id,
                spec.display,
                access,
                effective_workspace,
                steering,
                cancel.clone(),
            );
            let result = Box::pin(run_agent(child, task.to_string())).await;
            let status = if cancel.is_cancelled() {
                "cancelled"
            } else if result.is_ok() {
                "completed"
            } else {
                "failed"
            };
            let mut report = if result.is_ok() {
                read_report(&latest)
            } else {
                result
                    .err()
                    .map(|error| error.to_string())
                    .unwrap_or_default()
            };
            if access == crate::FileAccess::Worktree {
                let info = crate::worktree::WorktreeInfo {
                    path: std::path::PathBuf::from(&agent.worktree_path),
                    branch: agent.worktree_branch.clone(),
                    base: agent.worktree_base.clone(),
                    repo_root: std::path::PathBuf::from(&agent.repo_root),
                    subdir: std::path::PathBuf::from(&agent.workspace_subdir),
                };
                if !info.path.as_os_str().is_empty() {
                    match crate::worktree::commit_changes(&info, &format!("stark {}", agent.id)) {
                        Ok(outcome) => {
                            let diff = crate::worktree::diff(&info.path, &info.base)
                                .unwrap_or_else(|_| json!({}));
                            let _ = store
                                .set_worktree_state(&agent.id, "active", &diff, "")
                                .await;
                            if outcome["committed"].as_bool().unwrap_or(false) {
                                report.push_str(&format!(
                                    "\n[WORKTREE] branch={}\n{}",
                                    info.branch,
                                    outcome["stat"].as_str().unwrap_or("")
                                ));
                            }
                        }
                        Err(commit_error) => {
                            let _ = store
                                .set_worktree_state(&agent.id, "failed", &json!({}), &commit_error)
                                .await;
                        }
                    }
                }
            }
            let _ = store.finish(&agent.id, status, &report, json!({})).await;
            if status == "failed" {
                return ToolOutcome::err(format!("[ERR] Subagent {}: {report}", agent.role))
                    .with_children(vec![json!({
                        "agentId": agent.id,
                        "chatId": agent.chat_id,
                        "agentName": agent.role,
                        "label": title,
                        "workspaceId": ctx.workspace_id,
                    })]);
            }
            ToolOutcome::ok(format!(
                "[SUBAGENT {}] child={}\n{}",
                agent.role, agent.id, report
            ))
            .with_children(vec![json!({
                "agentId": agent.id,
                "chatId": agent.chat_id,
                "agentName": agent.role,
                "label": title,
                "workspaceId": ctx.workspace_id,
            })])
        }
        "message" => {
            let Some(child_id) = args.get("child_id").and_then(Value::as_str) else {
                return ToolOutcome::err("Error: action='message' requires child_id");
            };
            let Some(message) = args.get("message").and_then(Value::as_str) else {
                return ToolOutcome::err("Error: action='message' requires message");
            };
            match store.get(child_id).await {
                Ok(Some(target)) if target.parent_id.as_deref() == Some(current.id.as_str()) => {
                    let _ = store
                        .enqueue(
                            Some(&current.id),
                            &target.id,
                            "message",
                            message,
                            "trigger",
                            json!({}),
                        )
                        .await;
                    if target.status != "running" {
                        let _ = store.wake(&target.id).await;
                    }
                    ToolOutcome::ok(format!("[OK] queued message for {child_id}"))
                }
                _ => ToolOutcome::err(format!("Error: unknown subagent {child_id}")),
            }
        }
        "stop" => {
            let Some(child_id) = args.get("child_id").and_then(Value::as_str) else {
                return ToolOutcome::err("Error: action='stop' requires child_id");
            };
            match store.get(child_id).await {
                Ok(Some(target)) if target.parent_id.as_deref() == Some(current.id.as_str()) => {
                    crate::cancel_agent_run(&target.id);
                    let _ = store.set_status(&target.id, "cancelled", false).await;
                    ToolOutcome::ok(format!("[OK] stopped {child_id}"))
                }
                _ => ToolOutcome::err(format!("Error: unknown subagent {child_id}")),
            }
        }
        other => ToolOutcome::err(format!("Error: unknown subagent action '{other}'")),
    }
}

fn format_subagent(agent: &stark_store::AgentRecord) -> String {
    let mut text = format!(
        "child={}\nname={}\nstatus={}\naccess={}",
        agent.id,
        agent.role,
        agent.status,
        if agent.access.is_empty() {
            "write"
        } else {
            &agent.access
        },
    );
    if !agent.worktree_branch.is_empty() {
        text.push_str(&format!(
            "\nworktree branch={} base={} state={}",
            agent.worktree_branch, agent.worktree_base, agent.worktree_state
        ));
    }
    if !agent.result.is_empty() {
        text.push('\n');
        text.push_str(&agent.result.chars().take(12_000).collect::<String>());
    }
    text
}

fn safe_agent_name(value: &str) -> String {
    let name: String = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        .take(24)
        .collect();
    if name.is_empty() {
        "agent".to_string()
    } else {
        name
    }
}

pub(crate) fn read_report(latest: &Arc<Mutex<Option<AppState>>>) -> String {
    latest
        .lock()
        .ok()
        .and_then(|latest| latest.clone())
        .and_then(|state| state.outputs.get("agent_report").cloned())
        .and_then(|report| report.as_str().map(str::to_string))
        .unwrap_or_else(|| "(no report)".to_string())
}

#[allow(dead_code)]
pub(crate) fn child_context(
    ctx: &RunContext,
    child_id: &str,
    name: &str,
    steering: Arc<Mutex<VecDeque<String>>>,
    cancel: CancellationToken,
) -> (RunContext, Arc<Mutex<Option<AppState>>>) {
    child_context_with_access(
        ctx,
        child_id,
        name,
        crate::FileAccess::Read,
        ctx.workspace.clone(),
        steering,
        cancel,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn child_context_with_access(
    ctx: &RunContext,
    child_id: &str,
    name: &str,
    access: crate::FileAccess,
    workspace: PathBuf,
    steering: Arc<Mutex<VecDeque<String>>>,
    cancel: CancellationToken,
) -> (RunContext, Arc<Mutex<Option<AppState>>>) {
    let (tx, _) = broadcast::channel(64);
    let latest = Arc::new(Mutex::new(None));
    let child = RunContext {
        chat_id: child_id.to_string(),
        workspace_id: ctx.workspace_id.clone(),
        workspace,
        role: Some(format!("the {name} specialist subagent")),
        scope: name.to_lowercase(),
        depth: ctx.depth + 1,
        file_access: access,
        model: ctx.model.clone(),
        temperature: ctx.temperature,
        images: Vec::new(),
        provider: ctx.provider.clone(),
        store: ctx.store.clone(),
        questions: ctx.questions.clone(),
        tx,
        latest: latest.clone(),
        steering,
        follow_up: Arc::new(Mutex::new(VecDeque::new())),
        checkpoint: None,
        cancel,
        on_job_done: None,
    };
    (child, latest)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::body::Body;
    use axum::http::header;
    use axum::response::Response;
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::{Value, json};

    use super::*;
    use stark_providers::ProviderConfig;

    async fn mock_provider(
        axum::extract::State(counter): axum::extract::State<Arc<AtomicUsize>>,
        Json(_body): Json<Value>,
    ) -> Response {
        let turn = counter.fetch_add(1, Ordering::SeqCst);
        let payload = if turn == 0 {
            vec![
                json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"read_file","arguments":"{\"file_path\":\"a.txt\"}"}}]},"finish_reason":null}]}),
                json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
            ]
        } else {
            vec![
                json!({"choices":[{"delta":{"content":"Done"}}]}),
                json!({"choices":[{"delta":{},"finish_reason":"stop"}]}),
            ]
        };
        let mut body = String::new();
        for item in payload {
            body.push_str(&format!("data: {item}\n\n"));
        }
        body.push_str("data: [DONE]\n\n");
        Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(body))
            .unwrap()
    }

    async fn mock_subagent_provider(
        axum::extract::State(counter): axum::extract::State<Arc<AtomicUsize>>,
        Json(_body): Json<Value>,
    ) -> Response {
        let turn = counter.fetch_add(1, Ordering::SeqCst);
        let payload = if turn == 0 {
            vec![
                json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"call_subagent","arguments":"{\"name\":\"Developer\",\"task\":\"reply DONE\"}"}}]},"finish_reason":null}]}),
                json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
            ]
        } else {
            vec![
                json!({"choices":[{"delta":{"content":"Done"}}]}),
                json!({"choices":[{"delta":{},"finish_reason":"stop"}]}),
            ]
        };
        let mut body = String::new();
        for item in payload {
            body.push_str(&format!(
                "data: {item}

"
            ));
        }
        body.push_str(
            "data: [DONE]

",
        );
        Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(body))
            .unwrap()
    }

    async fn mock_plan_provider(
        axum::extract::State(counter): axum::extract::State<Arc<AtomicUsize>>,
        Json(_body): Json<Value>,
    ) -> Response {
        let turn = counter.fetch_add(1, Ordering::SeqCst);
        let payload = if turn == 0 {
            vec![
                json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"update_plan","arguments":"{\"plan\":[{\"step\":\"inspect\",\"status\":\"in_progress\"}]}"}}]},"finish_reason":null}]}),
                json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
            ]
        } else {
            vec![
                json!({"choices":[{"delta":{"content":"Planned"}}]}),
                json!({"choices":[{"delta":{},"finish_reason":"stop"}]}),
            ]
        };
        let mut body = String::new();
        for item in payload {
            body.push_str(&format!("data: {item}\n\n"));
        }
        body.push_str("data: [DONE]\n\n");
        Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(body))
            .unwrap()
    }

    async fn mock_coder_provider(
        axum::extract::State(counter): axum::extract::State<Arc<AtomicUsize>>,
        Json(_body): Json<Value>,
    ) -> Response {
        let turn = counter.fetch_add(1, Ordering::SeqCst);
        let code = "const t = tools.read({ file_path: \"a.txt\" }); tools.edit({ file_path: \"a.txt\", old_string: \"hello\", new_string: \"bye\" }); return { done: t.includes(\"hello\") };";
        let payload = if turn == 0 {
            vec![
                json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"run_code","arguments": serde_json::to_string(&json!({"code": code})).unwrap()}}]},"finish_reason":null}]}),
                json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
            ]
        } else {
            vec![
                json!({"choices":[{"delta":{"content":"Done"}}]}),
                json!({"choices":[{"delta":{},"finish_reason":"stop"}]}),
            ]
        };
        let mut body = String::new();
        for item in payload {
            body.push_str(&format!("data: {item}\n\n"));
        }
        body.push_str("data: [DONE]\n\n");
        Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(body))
            .unwrap()
    }

    async fn mock_truncated_provider(
        axum::extract::State(counter): axum::extract::State<Arc<AtomicUsize>>,
        Json(_body): Json<Value>,
    ) -> Response {
        let turn = counter.fetch_add(1, Ordering::SeqCst);
        let payload = if turn == 0 {
            vec![
                json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"write","arguments":"{\"file_path\":\"big.txt\",\"content\":\"partial"}}]},"finish_reason":null}]}),
                json!({"choices":[{"delta":{},"finish_reason":"length"}]}),
            ]
        } else {
            vec![
                json!({"choices":[{"delta":{"content":"Split it"}}]}),
                json!({"choices":[{"delta":{},"finish_reason":"stop"}]}),
            ]
        };
        let mut body = String::new();
        for item in payload {
            body.push_str(&format!("data: {item}\n\n"));
        }
        body.push_str("data: [DONE]\n\n");
        Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(body))
            .unwrap()
    }

    #[tokio::test]
    async fn truncated_tool_calls_never_execute() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let counter = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/chat/completions", post(mock_truncated_provider))
            .with_state(counter.clone());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let dir = tempfile::tempdir().unwrap();
        let store = ChatStore::open(dir.path()).await.unwrap();
        let provider = Provider::new(ProviderConfig {
            mode: "deepseek".to_string(),
            base_url: format!("http://{address}"),
            api_key: "test".to_string(),
            model: "mock".to_string(),
        })
        .unwrap();
        let (tx, _rx) = broadcast::channel(256);
        let latest = Arc::new(Mutex::new(None));
        let ctx = RunContext {
            chat_id: "truncated-chat".to_string(),
            workspace_id: "_general".to_string(),
            workspace: dir.path().to_path_buf(),
            role: Some("Coder".to_string()),
            scope: "coder".to_string(),
            depth: 0,
            file_access: crate::FileAccess::Write,
            model: "mock".to_string(),
            temperature: 0.0,
            images: Vec::new(),
            provider,
            store: store.clone(),
            questions: Arc::new(QuestionBroker::new()),
            tx,
            latest: latest.clone(),
            steering: Arc::new(Mutex::new(VecDeque::new())),
            follow_up: Arc::new(Mutex::new(VecDeque::new())),
            checkpoint: None,
            cancel: CancellationToken::new(),
            on_job_done: None,
        };
        run_agent(ctx, "write a big file".to_string())
            .await
            .unwrap();

        // The partial payload never reached disk, and the model got one retry.
        assert!(!dir.path().join("big.txt").exists());
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn coder_turn_completes_multistep_work_in_one_program() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let counter = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/chat/completions", post(mock_coder_provider))
            .with_state(counter.clone());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        let store = ChatStore::open(dir.path()).await.unwrap();
        let provider = Provider::new(ProviderConfig {
            mode: "deepseek".to_string(),
            base_url: format!("http://{address}"),
            api_key: "test".to_string(),
            model: "mock".to_string(),
        })
        .unwrap();
        let (tx, _rx) = broadcast::channel(256);
        let latest = Arc::new(Mutex::new(None));
        let ctx = RunContext {
            chat_id: "coder-chat".to_string(),
            workspace_id: "_general".to_string(),
            workspace: dir.path().to_path_buf(),
            role: Some("Coder".to_string()),
            scope: "coder".to_string(),
            depth: 0,
            file_access: crate::FileAccess::Write,
            model: "mock".to_string(),
            temperature: 0.0,
            images: Vec::new(),
            provider,
            store: store.clone(),
            questions: Arc::new(QuestionBroker::new()),
            tx,
            latest: latest.clone(),
            steering: Arc::new(Mutex::new(VecDeque::new())),
            follow_up: Arc::new(Mutex::new(VecDeque::new())),
            checkpoint: None,
            cancel: CancellationToken::new(),
            on_job_done: None,
        };
        run_agent(ctx, "rename hello to bye".to_string())
            .await
            .unwrap();

        // One outer program call did the read, edit, and verify.
        assert_eq!(counter.load(Ordering::SeqCst), 2);
        let saved = std::fs::read_to_string(dir.path().join("a.txt")).unwrap();
        assert_eq!(saved, "bye");
    }

    #[tokio::test]
    async fn update_plan_populates_run_state_and_task_file() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let counter = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/chat/completions", post(mock_plan_provider))
            .with_state(counter.clone());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let dir = tempfile::tempdir().unwrap();
        let store = ChatStore::open(dir.path()).await.unwrap();
        let provider = Provider::new(ProviderConfig {
            mode: "deepseek".to_string(),
            base_url: format!("http://{address}"),
            api_key: "test".to_string(),
            model: "mock".to_string(),
        })
        .unwrap();
        let (tx, _rx) = broadcast::channel(256);
        let latest = Arc::new(Mutex::new(None));
        let ctx = RunContext {
            chat_id: "plan-chat".to_string(),
            workspace_id: "_general".to_string(),
            workspace: dir.path().to_path_buf(),
            role: None,
            scope: "agent".to_string(),
            depth: 0,
            file_access: crate::FileAccess::Write,
            model: "mock".to_string(),
            temperature: 0.0,
            images: Vec::new(),
            provider,
            store: store.clone(),
            questions: Arc::new(QuestionBroker::new()),
            tx,
            latest: latest.clone(),
            steering: Arc::new(Mutex::new(VecDeque::new())),
            follow_up: Arc::new(Mutex::new(VecDeque::new())),
            checkpoint: None,
            cancel: CancellationToken::new(),
            on_job_done: None,
        };

        run_agent(ctx, "make a plan".to_string()).await.unwrap();

        let state = latest.lock().unwrap().clone().unwrap();
        let plan = state.plan.expect("plan in state");
        assert_eq!(plan["agent"][0].step, "inspect");
        let task = std::fs::read_to_string(
            dir.path()
                .join(".deep_agents")
                .join("tasks")
                .join("task_plan-chat.json"),
        )
        .unwrap();
        assert!(task.contains("\"inspect\""));
    }

    #[tokio::test]
    async fn subagent_run_persists_a_child_chat() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let counter = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/chat/completions", post(mock_subagent_provider))
            .with_state(counter.clone());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let dir = tempfile::tempdir().unwrap();
        let store = ChatStore::open(dir.path()).await.unwrap();
        let provider = Provider::new(ProviderConfig {
            mode: "deepseek".to_string(),
            base_url: format!("http://{address}"),
            api_key: "test".to_string(),
            model: "mock".to_string(),
        })
        .unwrap();
        let (tx, _rx) = broadcast::channel(256);
        let latest = Arc::new(Mutex::new(None));
        let ctx = RunContext {
            chat_id: "parent".to_string(),
            workspace_id: "_general".to_string(),
            workspace: dir.path().to_path_buf(),
            role: Some("Developer".to_string()),
            scope: "developer".to_string(),
            depth: 1,
            file_access: crate::FileAccess::Write,
            model: "mock".to_string(),
            temperature: 0.0,
            images: Vec::new(),
            provider,
            store: store.clone(),
            questions: Arc::new(QuestionBroker::new()),
            tx,
            latest: latest.clone(),
            steering: Arc::new(Mutex::new(VecDeque::new())),
            follow_up: Arc::new(Mutex::new(VecDeque::new())),
            checkpoint: None,
            cancel: CancellationToken::new(),
            on_job_done: None,
        };
        run_agent(ctx, "delegate".to_string()).await.unwrap();
        let chats = store.list_chats("_general").await;
        let children: Vec<&Value> = chats
            .iter()
            .filter(|chat| chat["id"].as_str().is_some_and(|id| id.starts_with("del-")))
            .collect();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0]["title"], "reply DONE");
    }

    #[test]
    fn session_messages_keep_checkpoint_when_truncating() {
        let mut entries = vec![json!({
            "type": "message",
            "role": "user",
            "content": "<conversation-checkpoint>
<summary>s</summary>
</conversation-checkpoint>"
        })];
        for index in 0..100 {
            entries
                .push(json!({"type": "message", "role": "user", "content": format!("m{index}")}));
        }
        let messages = session_messages(&entries);
        assert_eq!(messages[0].role, "user");
        assert!(
            messages[0]
                .content
                .as_deref()
                .unwrap()
                .contains("<conversation-checkpoint>")
        );
    }

    #[test]
    fn session_messages_preserve_tool_pairs() {
        let entries = vec![
            json!({"type": "message", "role": "user", "content": "read it"}),
            json!({"type": "message", "role": "assistant", "content": [
                {"type": "text", "text": "reading"},
                {"type": "toolCall", "id": "c1", "name": "read_file", "arguments": {"file_path": "a.txt"}}
            ]}),
            json!({"type": "message", "role": "toolResult", "tool_call_id": "c1", "tool_name": "read_file", "content": "hello"}),
            json!({"type": "message", "role": "assistant", "content": "done"}),
        ];
        let messages = session_messages(&entries);
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[1].tool_calls.as_ref().unwrap().len(), 1);
        assert_eq!(messages[2].role, "tool");
        assert_eq!(messages[2].tool_call_id.as_deref(), Some("c1"));
        assert_eq!(messages[3].content.as_deref(), Some("done"));
    }

    #[tokio::test]
    async fn loop_executes_a_tool_and_finishes() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let counter = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/chat/completions", post(mock_provider))
            .with_state(counter.clone());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("a.txt"), "hello from tool\n")
            .await
            .unwrap();
        let store = ChatStore::open(dir.path()).await.unwrap();
        let provider = Provider::new(ProviderConfig {
            mode: "deepseek".to_string(),
            base_url: format!("http://{address}"),
            api_key: "test".to_string(),
            model: "mock".to_string(),
        })
        .unwrap();

        let (tx, mut rx) = broadcast::channel(256);
        let latest = Arc::new(Mutex::new(None));
        let ctx = RunContext {
            chat_id: "chat".to_string(),
            workspace_id: "_general".to_string(),
            workspace: dir.path().to_path_buf(),
            role: None,
            scope: "agent".to_string(),
            depth: 0,
            file_access: crate::FileAccess::Write,
            model: "mock".to_string(),
            temperature: 0.0,
            images: Vec::new(),
            provider,
            store: store.clone(),
            questions: Arc::new(QuestionBroker::new()),
            tx,
            latest: latest.clone(),
            steering: Arc::new(Mutex::new(VecDeque::new())),
            follow_up: Arc::new(Mutex::new(VecDeque::new())),
            checkpoint: None,
            cancel: CancellationToken::new(),
            on_job_done: None,
        };

        run_agent(ctx, "read a.txt".to_string()).await.unwrap();

        let state = latest.lock().unwrap().clone().unwrap();
        assert_eq!(state.active_node, "done");
        assert_eq!(state.outputs["agent_report"], "Done");

        let history = store.history("chat", 10).await.unwrap();
        assert_eq!(history[0].role, "user");
        assert_eq!(history.last().unwrap().content, "Done");
        assert_eq!(counter.load(Ordering::SeqCst), 2);

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        assert!(events.iter().any(|event| event.event == "state"));
        assert!(events.iter().any(|event| event.event == "delta"));
        assert!(
            events
                .iter()
                .any(|event| { event.event == "delta" && event.data["kind"] == "tool_started" })
        );
        assert!(
            events
                .iter()
                .any(|event| { event.event == "delta" && event.data["kind"] == "tool_finished" })
        );

        assert!(
            dir.path()
                .join(".deep_agents")
                .join("sessions")
                .join("general")
                .join("_general")
                .join("chat.jsonl")
                .exists()
        );
    }

    #[tokio::test]
    async fn plan_approval_marker_flips_plan_to_build() {
        use stark_tools::ToolOutcome;

        let dir = tempfile::tempdir().unwrap();
        let store = ChatStore::open(dir.path()).await.unwrap();
        store
            .ensure_chat(
                "plan-chat",
                "_general",
                &dir.path().to_string_lossy(),
                "mock",
                "hi",
            )
            .await
            .unwrap();
        assert!(store.set_chat_mode("plan-chat", "plan").await.unwrap());
        assert_eq!(
            store.chat_mode("plan-chat").await.unwrap().as_deref(),
            Some("plan")
        );
        let provider = Provider::new(ProviderConfig {
            mode: "deepseek".to_string(),
            base_url: "http://127.0.0.1:1".to_string(),
            api_key: "test".to_string(),
            model: "mock".to_string(),
        })
        .unwrap();
        let (tx, _) = broadcast::channel(256);
        let ctx = RunContext {
            chat_id: "plan-chat".to_string(),
            workspace_id: "_general".to_string(),
            workspace: dir.path().to_path_buf(),
            role: Some("Developer".to_string()),
            scope: "developer".to_string(),
            depth: 0,
            file_access: crate::FileAccess::Write,
            model: "mock".to_string(),
            temperature: 0.0,
            images: Vec::new(),
            provider,
            store: store.clone(),
            questions: Arc::new(QuestionBroker::new()),
            tx,
            latest: Arc::new(Mutex::new(None)),
            steering: Arc::new(Mutex::new(VecDeque::new())),
            follow_up: Arc::new(Mutex::new(VecDeque::new())),
            checkpoint: None,
            cancel: CancellationToken::new(),
            on_job_done: None,
        };
        let approved = ToolOutcome::ok(
            "[USER ANSWERS]\n- plan-review: Approve plan\n[PLAN APPROVED] Stop here.",
        );
        maybe_flip_plan_mode(&ctx, "plan", &approved).await;
        assert_eq!(
            store.chat_mode("plan-chat").await.unwrap().as_deref(),
            Some("build")
        );
        let rejected = ToolOutcome::ok("[USER ANSWERS]\n- plan-review: Keep planning");
        maybe_flip_plan_mode(&ctx, "plan", &rejected).await;
        assert_eq!(
            store.chat_mode("plan-chat").await.unwrap().as_deref(),
            Some("build")
        );
        let build_mode = ToolOutcome::ok("[PLAN APPROVED]");
        maybe_flip_plan_mode(&ctx, "build", &build_mode).await;
        assert_eq!(
            store.chat_mode("plan-chat").await.unwrap().as_deref(),
            Some("build")
        );
    }
}
