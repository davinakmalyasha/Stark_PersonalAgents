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
use stark_store::{ChatStore, StoreError};
use stark_tools::{Checkpoint, PlanStore, QuestionBroker, ToolContext, ToolOutcome};
use thiserror::Error;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

pub mod agents;
mod agents_tools;
pub mod canvas;
mod compaction;
mod delegation;
mod groups;
mod instructions;
mod notes;
mod projects;
mod schedules;
pub use agents::{AgentSpec, for_scope as agent_for_scope, resolve_agent};

pub type JobDoneHook = Arc<dyn Fn(String) + Send + Sync>;

const HISTORY_LIMIT: usize = 40;
const DEFAULT_RUN_SECONDS: u64 = 3_600;
const DEFAULT_RUN_TOKENS: u64 = 4_000_000;
const DEFAULT_RUN_TOOL_CALLS: usize = 500;
const REPEAT_READ_LIMIT: usize = 4;
const REPEAT_WRITE_LIMIT: usize = 5;
const MAX_CONSECUTIVE_ERRORS: usize = 5;

#[derive(Debug, Clone)]
enum ChildStatus {
    Running,
    Done(String),
    Failed(String),
    Stopped,
}

impl ChildStatus {
    fn label(&self) -> String {
        match self {
            Self::Running => "running".to_string(),
            Self::Done(_) => "done".to_string(),
            Self::Failed(_) => "failed".to_string(),
            Self::Stopped => "stopped".to_string(),
        }
    }

    fn report(&self) -> Option<&str> {
        match self {
            Self::Done(report) => Some(report),
            Self::Failed(error) => Some(error),
            _ => None,
        }
    }
}

struct ChildHandle {
    parent_chat_id: String,
    name: String,
    status: Arc<Mutex<ChildStatus>>,
    steering: Arc<Mutex<VecDeque<String>>>,
    cancel: CancellationToken,
}

static CHILDREN: OnceLock<Mutex<HashMap<String, Arc<ChildHandle>>>> = OnceLock::new();
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

fn children() -> &'static Mutex<HashMap<String, Arc<ChildHandle>>> {
    CHILDREN.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn subagent_status(parent_chat_id: &str) -> Vec<Value> {
    let children = children().lock().unwrap();
    children
        .iter()
        .filter(|(_, child)| child.parent_chat_id == parent_chat_id)
        .map(|(id, child)| {
            let status = child.status.lock().unwrap();
            json!({
                "child_id": id,
                "name": child.name,
                "label": child.name,
                "status": status.label(),
                "report": status.report(),
                "created": 0,
                "parent_session": parent_chat_id,
            })
        })
        .collect()
}

pub fn subagent_status_for(child_id: &str) -> Option<Value> {
    let children = children().lock().unwrap();
    children.get(child_id).map(|child| {
        let status = child.status.lock().unwrap();
        json!({
            "child_id": child_id,
            "name": child.name,
            "label": child.name,
            "status": status.label(),
            "report": status.report(),
            "created": 0,
            "parent_session": child.parent_chat_id,
        })
    })
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
    let preamble = match agent_for_scope(&ctx.scope) {
        Some(agent) => agent.instructions.to_string(),
        None => format!("You are Stark, {role}."),
    };
    let mut system_prompt = format!(
        "{preamble} Working in {workspace_label}. \
         Use the provided tools to inspect and change files. Read before editing. \
         Never invent tool results. Keep the final answer concise."
    );
    if let Some(instructions) = instructions::load(ctx.store.harness_root(), &ctx.workspace) {
        system_prompt.push_str("\n\n");
        system_prompt.push_str(&instructions);
    }
    if ctx.depth <= 1
        && stark_store::memory_enabled()
        && let Ok(memory) = stark_store::MemoryStore::open(ctx.store.harness_root()).await
        && let Some(summary) = memory
            .summary(&ctx.workspace_id, stark_store::memory_max_chars())
            .await
    {
        system_prompt.push_str("\n\n[WORKSPACE MEMORY]\n");
        system_prompt.push_str(&summary);
    }
    if mode == "plan" {
        system_prompt.push_str(
            " You are in PLAN mode: inspect and plan only. Use update_plan to maintain the plan. \
             Do not edit files, spawn subagents, or run commands with side effects.",
        );
    }
    if mode != "plan" && ctx.scope == "developer" {
        system_prompt.push_str(
            " For multi-step coding work, prefer run_code: one short JavaScript program can call              tools.read_file/search_code/edit_file/run_command synchronously, loop over results,              and return one compact value without putting every intermediate result in context.",
        );
    }
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
    if let Ok(block) = plan_store.progress_block(&ctx.chat_id, &ctx.scope).await
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

    let tool_specs = stark_tools::tool_specs_for_scope(&mode, &ctx.scope);
    let agent_label = ctx.agent_label();
    let mut tools = ToolContext::new(
        ctx.workspace.clone(),
        ctx.chat_id.clone(),
        ctx.questions.clone(),
    )
    .with_plan_store(Arc::new(plan_store))
    .with_harness_root(ctx.store.harness_root().to_path_buf())
    .with_scope(ctx.scope.clone())
    .with_mode(mode.clone());
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
                && stark_store::memory_enabled()
                && let Ok(memory) = stark_store::MemoryStore::open(ctx.store.harness_root()).await
            {
                let _ = memory
                    .record(&ctx.workspace_id, &ctx.chat_id, None, &prompt, &text)
                    .await;
                let _ = memory.consolidate(&ctx.workspace_id, 30).await;
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
            | "search_code"
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
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("run")
        .trim();
    match action {
        "list" => {
            let prefix = format!("cont-{}-", ctx.chat_id);
            let children = children().lock().unwrap();
            let rows: Vec<String> = children
                .iter()
                .filter(|(id, child)| {
                    id.starts_with(&prefix) && child.parent_chat_id == ctx.chat_id
                })
                .map(|(id, child)| {
                    let status = child.status.lock().unwrap();
                    format!("- {id} - {} - {}", child.name, status.label())
                })
                .collect();
            ToolOutcome::ok(if rows.is_empty() {
                "No subagent sessions.".to_string()
            } else {
                rows.join("\n")
            })
        }
        "status" => {
            let Some(child_id) = args.get("child_id").and_then(Value::as_str) else {
                return ToolOutcome::err("Error: action='status' requires child_id");
            };
            let wait_seconds = args
                .get("wait_seconds")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .min(300);
            let deadline = Instant::now() + Duration::from_secs(wait_seconds);
            loop {
                let handle = children().lock().unwrap().get(child_id).cloned();
                let Some(handle) = handle else {
                    return match ctx.store.get_chat(child_id).await {
                        Ok(Some(chat)) => ToolOutcome::ok(format!(
                            "child={child_id}\nstatus=idle\ntitle={}",
                            chat["title"].as_str().unwrap_or("")
                        )),
                        _ => ToolOutcome::err(format!("Error: unknown subagent {child_id}")),
                    };
                };
                let status = handle.status.lock().unwrap().clone();
                if !matches!(status, ChildStatus::Running) || Instant::now() >= deadline {
                    let mut text = format!(
                        "child={child_id}\nname={}\nstatus={}",
                        handle.name,
                        status.label()
                    );
                    if let Some(report) = status.report() {
                        text.push('\n');
                        text.push_str(report);
                    }
                    return ToolOutcome::ok(text);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        "run" | "spawn" => {
            if ctx.depth >= 2 {
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
            let safe_name: String = name
                .chars()
                .filter(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
                })
                .take(24)
                .collect();
            let child_id = format!(
                "cont-{}-{}-{}",
                ctx.chat_id,
                if safe_name.is_empty() {
                    "subagent"
                } else {
                    safe_name.as_str()
                },
                &uuid::Uuid::new_v4().simple().to_string()[..8]
            );
            let steering = Arc::new(Mutex::new(VecDeque::new()));
            let cancel = ctx.cancel.child_token();
            let status = Arc::new(Mutex::new(ChildStatus::Running));
            let handle = Arc::new(ChildHandle {
                parent_chat_id: ctx.chat_id.clone(),
                name: name.to_string(),
                status: status.clone(),
                steering: steering.clone(),
                cancel: cancel.clone(),
            });
            children()
                .lock()
                .unwrap()
                .insert(child_id.clone(), handle.clone());
            let (child, latest) = child_context(ctx, &child_id, name, steering, cancel.clone());

            if action == "spawn" {
                let handle = handle.clone();
                let task_for_run = task.to_string();
                let task_for_label = task.to_string();
                tokio::spawn(async move {
                    let result = Box::pin(run_agent(child, task_for_run)).await;
                    let next = if handle.cancel.is_cancelled() {
                        ChildStatus::Stopped
                    } else {
                        match result {
                            Ok(()) => ChildStatus::Done(read_report(&latest)),
                            Err(error) => ChildStatus::Failed(error.to_string()),
                        }
                    };
                    *handle.status.lock().unwrap() = next;
                });
                return ToolOutcome::ok(format!(
                    "[SPAWNED] child={child_id} name={name}; use action=status/child_id"
                ))
                .with_children(vec![json!({
                    "chatId": child_id,
                    "agentName": name,
                    "label": task_for_label,
                    "workspaceId": ctx.workspace_id,
                })]);
            }

            let result = Box::pin(run_agent(child, task.to_string())).await;
            let next = if cancel.is_cancelled() {
                ChildStatus::Stopped
            } else {
                match result {
                    Ok(()) => ChildStatus::Done(read_report(&latest)),
                    Err(error) => ChildStatus::Failed(error.to_string()),
                }
            };
            *status.lock().unwrap() = next.clone();
            if let ChildStatus::Failed(error) = next {
                return ToolOutcome::err(format!("[ERR] Subagent {name}: {error}")).with_children(
                    vec![json!({
                        "chatId": child_id,
                        "agentName": name,
                        "label": task,
                        "workspaceId": ctx.workspace_id,
                    })],
                );
            }
            ToolOutcome::ok(format!(
                "[SUBAGENT {name}] child={child_id}\n{}",
                read_report(&latest)
            ))
            .with_children(vec![json!({
                "chatId": child_id,
                "agentName": name,
                "label": task,
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
            let handle = children().lock().unwrap().get(child_id).cloned();
            match handle {
                Some(handle) => {
                    handle
                        .steering
                        .lock()
                        .unwrap()
                        .push_back(message.to_string());
                    ToolOutcome::ok(format!("[OK] queued steering for {child_id}"))
                }
                None => ToolOutcome::err(format!("Error: unknown subagent {child_id}")),
            }
        }
        "stop" => {
            let Some(child_id) = args.get("child_id").and_then(Value::as_str) else {
                return ToolOutcome::err("Error: action='stop' requires child_id");
            };
            let handle = children().lock().unwrap().get(child_id).cloned();
            match handle {
                Some(handle) => {
                    handle.cancel.cancel();
                    *handle.status.lock().unwrap() = ChildStatus::Stopped;
                    ToolOutcome::ok(format!("[OK] stopped {child_id}"))
                }
                None => ToolOutcome::err(format!("Error: unknown subagent {child_id}")),
            }
        }
        other => ToolOutcome::err(format!("Error: unknown subagent action '{other}'")),
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

pub(crate) fn child_context(
    ctx: &RunContext,
    child_id: &str,
    name: &str,
    steering: Arc<Mutex<VecDeque<String>>>,
    cancel: CancellationToken,
) -> (RunContext, Arc<Mutex<Option<AppState>>>) {
    let (tx, _) = broadcast::channel(64);
    let latest = Arc::new(Mutex::new(None));
    let child = RunContext {
        chat_id: child_id.to_string(),
        workspace_id: ctx.workspace_id.clone(),
        workspace: ctx.workspace.clone(),
        role: Some(format!("the {name} specialist subagent")),
        scope: name.to_lowercase(),
        depth: ctx.depth + 1,
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
            role: None,
            scope: "agent".to_string(),
            depth: 0,
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
            .filter(|chat| {
                chat["id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("cont-parent-"))
            })
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
}
