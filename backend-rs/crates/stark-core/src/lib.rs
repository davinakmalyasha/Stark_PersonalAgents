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
    ChatMessage, EventStream, ImagePart, Provider, ProviderEvent, ProviderSession, ToolCall,
    pricing_for,
};
use stark_store::{AgentStore, ChatStore, StoreError};
use stark_tools::{Checkpoint, PlanStore, QuestionBroker, ToolContext, ToolOutcome};
use thiserror::Error;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

mod agent_tools;
pub mod agents;
mod agents_tools;
pub mod canvas;
mod compaction;
mod delegation;
mod groups;
mod instructions;
pub mod memory_agent;
mod notes;
mod prompt;
mod rollout_trace;
mod schedules;
mod status_budget;
mod workspace_state;
mod workspaces;
pub mod worktree;
pub use agents::{
    AgentSpec, FileAccess, default_access, for_scope as agent_for_scope, resolve_agent,
};

pub type JobDoneHook = Arc<dyn Fn(String) + Send + Sync>;

const HISTORY_LIMIT: usize = 200;
const REPEAT_READ_LIMIT: usize = 4;
const REPEAT_WRITE_LIMIT: usize = 5;
const MAX_CONSECUTIVE_ERRORS: usize = 24;
const ERROR_NUDGE_INTERVAL: usize = 6;

static AGENT_CANCELS: OnceLock<Mutex<HashMap<String, CancellationToken>>> = OnceLock::new();

fn agent_cancels() -> &'static Mutex<HashMap<String, CancellationToken>> {
    AGENT_CANCELS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Live handles for delegated child runs. The top-level run manager only knows
/// about runs started from HTTP; child agents run inside an already-running
/// parent. Registering them here lets the streaming/status endpoints mirror a
/// child's live reasoning and tools when it is opened in the sidebar.
#[derive(Clone)]
pub struct ChildRunHandle {
    pub tx: broadcast::Sender<ServerEvent>,
    pub latest: Arc<Mutex<Option<AppState>>>,
    pub cancel: CancellationToken,
    pub started_at: String,
}

static CHILD_RUNS: OnceLock<Mutex<HashMap<String, ChildRunHandle>>> = OnceLock::new();

fn child_runs() -> &'static Mutex<HashMap<String, ChildRunHandle>> {
    CHILD_RUNS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_child_run(ctx: &RunContext) {
    if let Ok(mut runs) = child_runs().lock() {
        runs.insert(
            ctx.chat_id.clone(),
            ChildRunHandle {
                tx: ctx.tx.clone(),
                latest: ctx.latest.clone(),
                cancel: ctx.cancel.clone(),
                started_at: chrono::Utc::now().to_rfc3339(),
            },
        );
    }
}

fn unregister_child_run(chat_id: &str) {
    if let Ok(mut runs) = child_runs().lock() {
        runs.remove(chat_id);
    }
}

pub fn child_run_handle(chat_id: &str) -> Option<ChildRunHandle> {
    child_runs()
        .lock()
        .ok()
        .and_then(|runs| runs.get(chat_id).cloned())
}

pub fn subscribe_child_run(chat_id: &str) -> Option<broadcast::Receiver<ServerEvent>> {
    child_run_handle(chat_id).map(|handle| handle.tx.subscribe())
}

pub fn child_run_active(chat_id: &str) -> bool {
    child_runs()
        .lock()
        .map(|runs| runs.contains_key(chat_id))
        .unwrap_or(false)
}

pub fn child_run_state(chat_id: &str) -> Option<AppState> {
    child_run_handle(chat_id)
        .and_then(|handle| handle.latest.lock().ok().and_then(|state| state.clone()))
}

pub fn child_run_started_at(chat_id: &str) -> Option<String> {
    child_run_handle(chat_id).map(|handle| handle.started_at)
}

pub fn cancel_child_run(chat_id: &str) -> bool {
    child_run_handle(chat_id)
        .map(|handle| {
            handle.cancel.cancel();
            true
        })
        .unwrap_or(false)
}

struct ChildRunGuard(String);

impl Drop for ChildRunGuard {
    fn drop(&mut self) {
        unregister_child_run(&self.0);
    }
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
    /// Effective agent configuration resolved from `.deep_agents/config`.
    /// `None` in tests and background jobs keeps the built-in contract.
    pub config: Option<Arc<stark_config::EffectiveAgent>>,
    pub on_job_done: Option<JobDoneHook>,
}

impl RunContext {
    fn emit(&self, event: impl Into<String>, data: impl serde::Serialize) {
        let _ = self.tx.send(ServerEvent::new(event, data));
    }

    fn emit_delta(&self, delta: Delta) {
        // Codex App Server lifecycle: every tool delta also carries an
        // item_started/item_completed event so clients can render streaming
        // items without replaying the full log. The `delta` event is kept
        // byte-identical for existing clients.
        match delta.kind.as_str() {
            "tool_started" => self.emit("item_started", &delta),
            "tool_finished" => self.emit("item_completed", &delta),
            _ => {}
        }
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

    /// Reasoning effort resolved from the agent config, if any. Providers and
    /// models that do not advertise reasoning support get `None`.
    fn thinking(&self) -> Option<&str> {
        self.config
            .as_ref()
            .filter(|config| config.reasoning)
            .and_then(|config| config.thinking.as_deref())
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
    let mut messages = Vec::new();
    // Resume from the newest compaction: its summary replaces everything that
    // came before. A fixed message cap (previously 80) discarded real history
    // on long tasks; token-based compaction is the only limit now.
    let start = if let Some(index) = entries
        .iter()
        .rposition(|entry| entry.get("type").and_then(Value::as_str) == Some("compaction"))
    {
        if let Some(summary) = entries[index].get("summary").and_then(Value::as_str)
            && !summary.is_empty()
        {
            messages.push(ChatMessage::user(format!(
                "<conversation-checkpoint>\n{summary}\n</conversation-checkpoint>"
            )));
        }
        index + 1
    } else {
        0
    };
    // Legacy: the first entry may be a system summary or a checkpoint.
    if start == 0
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
    let mut seen_message = false;
    for entry in &entries[start..] {
        let role = entry.get("role").and_then(Value::as_str).unwrap_or("");
        // A compaction boundary can cut a tool call from its result; drop
        // leading orphans so the provider never sees an unanswered tool.
        if !seen_message && role == "toolResult" {
            continue;
        }
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
            let text = content_text(&content);
            // Legacy rows persisted internal wake prompts as user messages
            // ([AGENT/GROUP/DELEGATION RESULT]); never feed them back to the
            // model as if the user typed them.
            let trimmed = text.trim_start();
            if trimmed.starts_with("[DELEGATION RESULT]")
                || trimmed.starts_with("[AGENT RESULT]")
                || trimmed.starts_with("[GROUP RESULT]")
            {
                seen_message = true;
                continue;
            }
            messages.push(ChatMessage::user(text));
            seen_message = true;
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
        seen_message = true;
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

fn is_internal_followup(prompt: &str) -> bool {
    let trimmed = prompt.trim_start();
    trimmed.starts_with("[DELEGATION RESULT]")
        || trimmed.starts_with("[AGENT RESULT]")
        || trimmed.starts_with("[GROUP RESULT]")
}

async fn run_agent_inner(ctx: RunContext, prompt: String) -> Result<(), RunError> {
    crate::status_budget::begin_turn(&ctx.chat_id);
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

    // Canonical trace: one rollout file per chat. Open (or resume) first so
    // every subsequent event is recorded, even when the provider fails.
    let mut rollout = stark_store::rollout::RolloutWriter::open(
        ctx.store.harness_root(),
        &ctx.chat_id,
        &ctx.workspace_id,
        &ctx.workspace,
        &ctx.model,
    )
    .await
    .ok();
    if let Some(writer) = rollout.as_mut() {
        let relative = writer.relative_path(ctx.store.harness_root());
        let _ = ctx
            .store
            .record_rollout(&ctx.chat_id, &relative, 0, None)
            .await;
        // One-time migration: a brand-new rollout with legacy history copies
        // the old user/assistant messages into the canonical log so the next
        // turn still sees the conversation. The file itself is the migration;
        // no separate command is needed. Existing rollouts are untouched.
        let lines = stark_store::rollout::read_rollout(writer.path());
        let is_new = lines.len() <= 1;
        if is_new && (!history.is_empty() || !session_entries.is_empty()) {
            if !history.is_empty() {
                for message in &history {
                    if message.role == "user" || message.role == "assistant" {
                        rollout_trace::record(
                            &ctx.store,
                            writer,
                            &ctx.chat_id,
                            stark_store::rollout::RolloutItem::ResponseItem {
                                payload: stark_store::rollout::ResponseItemPayload::Message {
                                    id: None,
                                    role: message.role.clone(),
                                    content: serde_json::Value::String(message.content.clone()),
                                    provider: None,
                                    model: None,
                                    stop_reason: None,
                                },
                                metadata: None,
                            },
                        )
                        .await;
                    }
                }
            } else {
                for entry in &session_entries {
                    if entry.get("type").and_then(Value::as_str) != Some("message") {
                        continue;
                    }
                    let role = entry.get("role").and_then(Value::as_str).unwrap_or("");
                    if role != "user" && role != "assistant" {
                        continue;
                    }
                    let content = entry.get("content").cloned().unwrap_or(Value::Null);
                    rollout_trace::record(
                        &ctx.store,
                        writer,
                        &ctx.chat_id,
                        stark_store::rollout::RolloutItem::ResponseItem {
                            payload: stark_store::rollout::ResponseItemPayload::Message {
                                id: None,
                                role: role.to_string(),
                                content,
                                provider: None,
                                model: None,
                                stop_reason: None,
                            },
                            metadata: None,
                        },
                    )
                    .await;
                }
            }
        }
    }
    let turn_id = format!("turn-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    if let Some(writer) = rollout.as_mut() {
        rollout_trace::record_turn_started(&ctx.store, writer, &ctx.chat_id, &turn_id, &ctx.model)
            .await;
    }
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
    if !is_internal_followup(&prompt) {
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
        if let Some(writer) = rollout.as_mut() {
            rollout_trace::record_user(&ctx.store, writer, &ctx.chat_id, &prompt).await;
        }
    }

    let workspace_label = ctx.workspace.to_string_lossy().to_string();
    let role = ctx.role.as_deref().unwrap_or("a coding agent");
    let system_prompt = if ctx.scope == "developer" || ctx.scope == "coder" {
        // Developer: stable content first and the volatile runtime snapshot
        // last so prefix-cache reuse survives plan switches. Memory/worktree
        // blocks are intentionally absent. Coder shares the shape and adds the
        // program-tool-calling section.
        let mut prompt = crate::prompt::system_prompt_for_agent(crate::prompt::AgentPrompt {
            workspace: &ctx.workspace,
            workspace_id: &ctx.workspace_id,
            harness_root: ctx.store.harness_root(),
            model: &ctx.model,
            mode: &mode,
            depth: ctx.depth,
            code_mode: ctx.scope == "coder",
            access: ctx.file_access.as_str(),
            skills: ctx
                .config
                .as_ref()
                .and_then(|config| config.skills.as_deref()),
        })
        .await;
        if let Some(custom) = ctx
            .config
            .as_ref()
            .and_then(|config| config.system_prompt.as_deref())
        {
            prompt.push_str("\n\n# Agent instructions\n");
            prompt.push_str(custom);
        }
        prompt
    } else {
        let preamble = ctx
            .config
            .as_ref()
            .and_then(|config| config.system_prompt.clone())
            .unwrap_or_else(|| match agent_for_scope(&ctx.scope) {
                Some(agent) => agent.instructions.to_string(),
                None => format!("You are Stark, {role}."),
            });
        let mut prompt = format!(
            "{preamble} Use the provided tools to inspect and change files. Read before editing. \
             Never invent tool results. Persist until the task is resolved: if a tool fails, read \
             the error, fix the arguments or try an alternative tool, and continue. Keep the final answer concise.\n\n{}",
            workspace_state::render(ctx.store.harness_root(), &ctx.workspace_id, &ctx.workspace)
        );
        prompt.push('\n');
        prompt.push_str(workspace_state::USAGE_HINT);
        if ctx.file_access.is_read_only() {
            prompt.push_str(
                " You are READ-ONLY in this task. Do not edit files, run mutating commands, or call write tools. \
                 Inspect with the read-only tools available to your scope (read/grep/glob, read-only bash, files_changed) and memory. \
                 If the task needs writes, delegate it to a workspace agent instead of doing it yourself.",
            );
        }
        if let Some(instructions) = instructions::load(ctx.store.harness_root(), &ctx.workspace) {
            prompt.push_str("\n\n");
            prompt.push_str(&instructions);
        }
        if let Some(memory) =
            crate::prompt::memory_block(ctx.store.harness_root(), &ctx.workspace_id).await
        {
            prompt.push_str("\n\n");
            prompt.push_str(&memory);
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
    // Rollout is canonical when present; legacy JSONL/SQLite is the fallback.
    let rollout_lines = rollout
        .as_ref()
        .map(|writer| stark_store::rollout::read_rollout(writer.path()))
        .unwrap_or_default();
    let rollout_messages = rollout_trace::messages_from_rollout(&rollout_lines);
    if !rollout_messages.is_empty() {
        messages.extend(rollout_messages);
    } else if session_entries.is_empty() {
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
    // Codex parity: every executing scope, including Developer/Coder, sees the
    // current plan progress. Excluding them let the two main coding agents
    // drift from the plan they were asked to maintain.
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
    maybe_compact(&ctx, &mut messages, &affinity, None, rollout.as_mut()).await;

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
    .with_depth(ctx.depth)
    .with_allowed_tools(ctx.config.as_ref().and_then(|config| config.tools.clone()))
    .with_mcp_servers(ctx.config.as_ref().and_then(|config| config.mcp.clone()));
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
    let mut repeat_counts: HashMap<(String, String), usize> = HashMap::new();
    let mut doom_allowed: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Provider-reported context usage for this turn, when the gateway returns it.
    let mut measured_tokens: Option<usize> = None;
    // One reactive compaction retry per run on a context-window rejection.
    let mut overflow_recovered = false;
    let context_window =
        compaction::CompactionPolicy::from_env(&ctx.scope, &ctx.model).hard_limit_tokens as u64;
    state.context_window = context_window;
    let mut consecutive_errors = 0usize;
    // Provider output-cap recovery: a text response cut by `finish_reason:
    // length` is incomplete, not an answer. Ask the model to continue instead
    // of silently ending the run (this truncated real work before).
    let mut length_continuations = 0usize;
    let mut carried_text = String::new();
    let mut stream_retries = 0u64;
    let stream_budget = stark_providers::stream_max_retries();

    'turn_loop: loop {
        if ctx.cancel.is_cancelled() {
            finish_aborted(&ctx, &mut state, rollout.as_mut(), "", "", Some(&turn_id)).await;
            return Ok(());
        }
        // No wall-clock, token, or tool-call budgets: the run continues until
        // the task is done or the user cancels.

        let steering_message = { ctx.steering.lock().unwrap().pop_front() };
        if let Some(message) = steering_message {
            messages.push(ChatMessage::user(&message));
            let _ = ctx
                .store
                .append_session_entry_for(
                    &ctx.workspace_id,
                    &ctx.workspace,
                    &ctx.chat_id,
                    json!({
                        "type": "message",
                        "role": "user",
                        "content": message,
                    }),
                )
                .await;
            if let Some(writer) = rollout.as_mut() {
                rollout_trace::record_user(&ctx.store, writer, &ctx.chat_id, &message).await;
            }
        }

        // Recompute per turn so `tool_search` can enable deferred MCP tools.
        let active_mcp = tools
            .active_mcp
            .lock()
            .map(|set| set.clone())
            .unwrap_or_default();
        let tool_specs = stark_tools::tool_specs_for_run(
            &mode,
            &ctx.scope,
            ctx.file_access.as_str(),
            &active_mcp,
            ctx.config
                .as_ref()
                .and_then(|config| config.tools.as_deref()),
            ctx.config.as_ref().and_then(|config| config.mcp.as_deref()),
        );

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

        let mut stream: EventStream = {
            let max_attempts = stark_providers::stream_max_retries();
            let mut attempt = 0u64;
            loop {
                match ctx
                    .provider
                    .stream_chat_for_session(
                        &messages,
                        &tool_specs,
                        ctx.temperature,
                        ProviderSession {
                            session_id: &ctx.chat_id,
                            affinity: &affinity,
                            reasoning_effort: ctx.thinking(),
                        },
                    )
                    .await
                {
                    Ok(stream) => break stream,
                    Err(error) => {
                        if !overflow_recovered && is_context_overflow(&error) {
                            overflow_recovered = true;
                            if force_compact(&ctx, &mut messages, &affinity, rollout.as_mut()).await
                            {
                                // Compaction rewrote history; restart the
                                // turn with the checkpoint instead of
                                // retrying the same oversized request.
                                continue 'turn_loop;
                            }
                        }
                        let retryable = error.is_retryable();
                        if retryable && attempt < max_attempts {
                            attempt += 1;
                            let delay = error
                                .retry_delay(attempt)
                                .unwrap_or_else(|| stark_providers::backoff_delay(attempt));
                            if let Some(writer) = rollout.as_mut() {
                                rollout_trace::record_warning(
                                    &ctx.store,
                                    writer,
                                    &ctx.chat_id,
                                    &format!(
                                        "Reconnecting ({attempt}/{max_attempts}) after provider error: {error}"
                                    ),
                                )
                                .await;
                            }
                            ctx.emit_delta(
                                Delta::text(format!("_Reconnecting ({attempt}/{max_attempts})…_"))
                                    .with_agent(agent_label.clone()),
                            );
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                        if let Some(writer) = rollout.as_mut() {
                            fail_with_trace(
                                &ctx,
                                &mut state,
                                writer,
                                &turn_id,
                                error.to_string(),
                                "",
                                "",
                                None,
                            )
                            .await;
                        } else {
                            fail(&ctx, &mut state, error.to_string()).await;
                        }
                        return Err(RunError::Provider(error));
                    }
                }
            }
        };

        let mut text = String::new();
        let mut reasoning = String::new();
        let mut calls: BTreeMap<usize, PartialCall> = BTreeMap::new();
        let mut finish_reason: Option<String> = None;
        let mut retry_after_compaction = false;
        // Watchdog: a provider that stops emitting (dead socket, stalled
        // gateway) must not hang a run forever and burn wall-clock. Reset on
        // every event; 0 disables it.
        let idle_timeout = std::env::var("STARK_RUN_IDLE_TIMEOUT_SEC")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(900);

        loop {
            let event = tokio::select! {
                _ = ctx.cancel.cancelled() => {
                    finish_aborted(&ctx, &mut state, rollout.as_mut(), &text, &reasoning, Some(&turn_id)).await;
                    return Ok(());
                }
                _ = tokio::time::sleep(Duration::from_secs(idle_timeout)), if idle_timeout > 0 => {
                    ctx.emit_state(&mut state);
                    let message = format!(
                        "run stalled: no provider output for {idle_timeout}s (set STARK_RUN_IDLE_TIMEOUT_SEC to change)"
                    );
                    if let Some(writer) = rollout.as_mut() {
                        fail_with_trace(&ctx, &mut state, writer, &turn_id, message.clone(), &text, &reasoning, Some("timeout")).await;
                    } else {
                        fail(&ctx, &mut state, message.clone()).await;
                    }
                    return Err(RunError::Message(message));
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
                    measured_tokens = Some((usage.input_tokens + usage.output_tokens) as usize);
                    state.context_tokens = usage.input_tokens + usage.output_tokens;
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
                    if let Some(writer) = rollout.as_mut() {
                        rollout_trace::record_usage(
                            &ctx.store,
                            writer,
                            &ctx.chat_id,
                            usage.input_tokens,
                            usage.output_tokens,
                            Some(cached),
                            Some(usage.reasoning_tokens),
                            Some(usage.total_tokens),
                            Some(cost),
                        )
                        .await;
                    }
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
                    // A context-window rejection is recoverable: compact hard
                    // and retry once instead of killing the run.
                    if !overflow_recovered && is_context_overflow(&error) {
                        overflow_recovered = true;
                        retry_after_compaction = true;
                        break;
                    }
                    // Transient mid-stream failures retry the turn with a
                    // warning; the partial text/reasoning of this attempt is
                    // discarded (never recorded) and rebuilt on retry.
                    if error.is_retryable() && stream_retries < stream_budget {
                        stream_retries += 1;
                        let delay = error
                            .retry_delay(stream_retries)
                            .unwrap_or_else(|| stark_providers::backoff_delay(stream_retries));
                        if let Some(writer) = rollout.as_mut() {
                            rollout_trace::record_warning(
                                &ctx.store,
                                writer,
                                &ctx.chat_id,
                                &format!(
                                    "Reconnecting ({stream_retries}/{stream_budget}) after stream error: {error}"
                                ),
                            )
                            .await;
                        }
                        ctx.emit_delta(
                            Delta::text(format!(
                                "_Reconnecting ({stream_retries}/{stream_budget})…_"
                            ))
                            .with_agent(agent_label.clone()),
                        );
                        tokio::time::sleep(delay).await;
                        // Discard this attempt and restart the turn. The
                        // per-turn locals are dropped; the next iteration
                        // rebuilds them from the same messages.
                        continue 'turn_loop;
                    }
                    if let Some(writer) = rollout.as_mut() {
                        fail_with_trace(
                            &ctx,
                            &mut state,
                            writer,
                            &turn_id,
                            error.to_string(),
                            &text,
                            &reasoning,
                            None,
                        )
                        .await;
                    } else {
                        fail(&ctx, &mut state, error.to_string()).await;
                    }
                    return Err(RunError::Provider(error));
                }
            }
        }

        if ctx.cancel.is_cancelled() {
            finish_aborted(
                &ctx,
                &mut state,
                rollout.as_mut(),
                &text,
                &reasoning,
                Some(&turn_id),
            )
            .await;
            return Ok(());
        }

        if retry_after_compaction {
            if force_compact(&ctx, &mut messages, &affinity, rollout.as_mut()).await {
                continue;
            }
            let message = "Provider rejected the context size and compaction could not reduce it."
                .to_string();
            if let Some(writer) = rollout.as_mut() {
                fail_with_trace(
                    &ctx,
                    &mut state,
                    writer,
                    &turn_id,
                    message.clone(),
                    &text,
                    &reasoning,
                    None,
                )
                .await;
            } else {
                fail(&ctx, &mut state, message.clone()).await;
            }
            return Err(RunError::Message(message));
        }

        if calls.is_empty() {
            if !text.trim().is_empty()
                && matches!(
                    finish_reason.as_deref(),
                    Some("length") | Some("max_tokens")
                )
            {
                const MAX_LENGTH_CONTINUATIONS: usize = 8;
                length_continuations += 1;
                if length_continuations > MAX_LENGTH_CONTINUATIONS {
                    let message = format!(
                        "Run stopped: the provider returned an output-limit stop {MAX_LENGTH_CONTINUATIONS} times in a row. Split the remaining work into smaller writes."
                    );
                    if let Some(writer) = rollout.as_mut() {
                        fail_with_trace(
                            &ctx,
                            &mut state,
                            writer,
                            &turn_id,
                            message.clone(),
                            &text,
                            &reasoning,
                            None,
                        )
                        .await;
                    } else {
                        fail(&ctx, &mut state, message.clone()).await;
                    }
                    return Err(RunError::Message(message));
                }
                carried_text.push_str(&text);
                messages.push(ChatMessage::assistant(&text));
                messages.push(ChatMessage::user(
                    "Your previous message was cut by the provider output limit. Continue exactly where you stopped. Do not repeat completed text. If a single write/edit is too large, split it into several smaller calls.",
                ));
                continue;
            }
            let final_text = format!("{carried_text}{text}");
            carried_text.clear();
            length_continuations = 0;
            if !reasoning.trim().is_empty() {
                messages.push(ChatMessage::assistant(&reasoning));
                if let Some(writer) = rollout.as_mut() {
                    rollout_trace::record_reasoning(&ctx.store, writer, &ctx.chat_id, &reasoning)
                        .await;
                }
            }
            if !final_text.is_empty() {
                let metadata = rollout_trace::projection_metadata(&reasoning, &[], false);
                let _ = ctx
                    .store
                    .append_message_with_metadata(
                        &ctx.chat_id,
                        "assistant",
                        &final_text,
                        false,
                        Some(&metadata),
                    )
                    .await;
                ctx.store
                    .append_session_entry_for(
                        &ctx.workspace_id,
                        &ctx.workspace,
                        &ctx.chat_id,
                        json!({
                            "type": "message",
                            "role": "assistant",
                            "content": final_text.clone(),
                            "provider": "stark-rs",
                            "model": ctx.model.clone(),
                            "stop_reason": finish_reason,
                        }),
                    )
                    .await?;
                if let Some(writer) = rollout.as_mut() {
                    rollout_trace::record_assistant(
                        &ctx.store,
                        writer,
                        &ctx.chat_id,
                        &final_text,
                        &ctx.model,
                        finish_reason.clone(),
                    )
                    .await;
                }
            }
            let next_follow_up = ctx.follow_up.lock().unwrap().pop_front();
            if let Some(follow_up) = next_follow_up {
                if !final_text.is_empty() {
                    messages.push(ChatMessage::assistant(&final_text));
                }
                if !is_internal_followup(&follow_up) {
                    let user_id = ctx
                        .store
                        .append_message(&ctx.chat_id, "user", &follow_up, false)
                        .await?;
                    ctx.store
                        .append_session_entry_for(
                            &ctx.workspace_id,
                            &ctx.workspace,
                            &ctx.chat_id,
                            json!({"type": "message", "role": "user", "content": follow_up.clone()}),
                        )
                        .await?;
                    if let Some(writer) = rollout.as_mut() {
                        rollout_trace::record_user(&ctx.store, writer, &ctx.chat_id, &follow_up)
                            .await;
                    }
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
            state.outputs = json!({"agent_report": final_text});
            ctx.emit_state(&mut state);
            if let Some(writer) = rollout.as_mut() {
                rollout_trace::record_turn_complete(
                    &ctx.store,
                    writer,
                    &ctx.chat_id,
                    &turn_id,
                    if final_text.trim().is_empty() {
                        None
                    } else {
                        Some(&final_text)
                    },
                    None,
                )
                .await;
            }
            if ctx.depth <= 1
                && stark_store::memory_enabled()
                && let Ok(memory) = stark_store::MemoryStore::open(ctx.store.harness_root()).await
            {
                let _ = memory
                    .record(
                        &ctx.workspace_id,
                        &ctx.chat_id,
                        Some(&ctx.scope),
                        &prompt,
                        &final_text,
                    )
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

        // A continuation text burst followed by a tool call belongs to the
        // same assistant turn in history.
        let assistant_text = if carried_text.is_empty() {
            text.clone()
        } else {
            format!("{carried_text}{text}")
        };
        carried_text.clear();
        length_continuations = 0;
        if !reasoning.trim().is_empty()
            && let Some(writer) = rollout.as_mut()
        {
            rollout_trace::record_reasoning(&ctx.store, writer, &ctx.chat_id, &reasoning).await;
        }
        if !assistant_text.is_empty()
            && let Some(writer) = rollout.as_mut()
        {
            rollout_trace::record_assistant(
                &ctx.store,
                writer,
                &ctx.chat_id,
                &assistant_text,
                &ctx.model,
                finish_reason.clone(),
            )
            .await;
        }
        if tool_calls.is_empty() {
            if !assistant_text.is_empty() {
                messages.push(ChatMessage::assistant(&assistant_text));
            }
        } else {
            // One assistant message carries both the text and the calls.
            // Pushing a separate text message here duplicated every
            // tool-using turn in the live and replayed history.
            messages.push(ChatMessage::assistant_tool_calls(
                if assistant_text.is_empty() {
                    None
                } else {
                    Some(assistant_text.clone())
                },
                tool_calls.clone(),
            ));
        }
        for call in &tool_calls {
            if let Some(writer) = rollout.as_mut() {
                rollout_trace::record_function_call(
                    &ctx.store,
                    writer,
                    &ctx.chat_id,
                    &call.id,
                    &call.function.name,
                    &call.function.arguments,
                )
                .await;
            }
        }

        let mut assistant_blocks = Vec::new();
        if !assistant_text.is_empty() {
            assistant_blocks.push(json!({"type": "text", "text": assistant_text}));
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

        // Parallel batch: independent calls in one assistant message run
        // concurrently. Calls touching the same file are grouped and run in
        // order, so parallel edits cannot race. Shell, subagents, groups and
        // long-running work stay sequential.
        let parallel_batch = tool_calls.len() > 1
            && tool_calls.iter().all(|call| {
                let args: Value =
                    serde_json::from_str(&call.function.arguments).unwrap_or_else(|_| json!({}));
                call_supports_parallel(&call.function.name, &args)
            });
        let mut precomputed: Vec<Option<ToolOutcome>> =
            (0..tool_calls.len()).map(|_| None).collect();
        if parallel_batch {
            let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
            for (index, call) in tool_calls.iter().enumerate() {
                let args: Value =
                    serde_json::from_str(&call.function.arguments).unwrap_or_else(|_| json!({}));
                let args_text = serde_json::to_string(&args).unwrap_or_default();
                ctx.emit_delta(
                    Delta::tool_started(&call.function.name, &args_text)
                        .with_agent(agent_label.clone()),
                );
                groups
                    .entry(resource_key(&call.function.name, &args, index))
                    .or_default()
                    .push(index);
            }
            let mut group_futures = Vec::new();
            for indices in groups.into_values() {
                let mut work = Vec::new();
                for index in indices {
                    let call = &tool_calls[index];
                    let name = call.function.name.clone();
                    let args: Value = serde_json::from_str(&call.function.arguments)
                        .unwrap_or_else(|_| json!({}));
                    work.push((index, name, args));
                }
                let tools_ctx = tools.clone();
                group_futures.push(async move {
                    let mut out = Vec::new();
                    for (index, name, args) in work {
                        out.push((index, stark_tools::dispatch(&name, &args, &tools_ctx).await));
                    }
                    out
                });
            }
            for group in futures_util::future::join_all(group_futures).await {
                for (index, outcome) in group {
                    precomputed[index] = Some(outcome);
                }
            }
        }

        for (call_index, call) in tool_calls.into_iter().enumerate() {
            if ctx.cancel.is_cancelled() {
                finish_aborted(&ctx, &mut state, rollout.as_mut(), "", "", Some(&turn_id)).await;
                return Ok(());
            }

            let args: Value =
                serde_json::from_str(&call.function.arguments).unwrap_or_else(|_| json!({}));
            let args_text = serde_json::to_string(&args).unwrap_or_default();
            let ready = precomputed[call_index].is_some();
            if !ready {
                ctx.emit_delta(
                    Delta::tool_started(&call.function.name, &args_text)
                        .with_agent(agent_label.clone()),
                );
            }

            // Repeat guard blocks only this duplicate call and tells the model,
            // instead of killing a long-horizon run.
            if !matches!(
                call.function.name.as_str(),
                "delegation_status" | "wait_agent"
            ) {
                let repeat_key = (call.function.name.clone(), args_text.clone());
                let count = {
                    let entry = repeat_counts.entry(repeat_key).or_insert(0);
                    *entry += 1;
                    *entry
                };
                let limit = if is_read_only_tool(&call.function.name) {
                    REPEAT_READ_LIMIT
                } else {
                    REPEAT_WRITE_LIMIT
                };
                if count >= limit && !doom_allowed.contains(&call.function.name) {
                    // The doom_loop rule decides: deny blocks, ask prompts, allow
                    // lets the repeat through for the rest of this run.
                    if let Some(blocked) =
                        stark_tools::doom_loop_gate(&tools, &call.function.name).await
                    {
                        let notice = blocked.content;
                        messages.push(ChatMessage::tool(&call.id, &notice));
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
                            Delta::tool_finished(
                                &call.function.name,
                                &args_text,
                                false,
                                0,
                                &notice,
                            )
                            .with_agent(agent_label.clone()),
                        );
                        continue;
                    }
                    doom_allowed.insert(call.function.name.clone());
                }
            }

            let started = Instant::now();
            let outcome = if let Some(outcome) = precomputed[call_index].take() {
                outcome
            } else if call.function.name == "call_subagent" {
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
                    | "resume_agent"
            ) {
                if let Some(reason) =
                    stark_tools::plan_mode_denial(&mode, &call.function.name, &args)
                {
                    ToolOutcome::err(reason)
                } else {
                    agent_tools::handle(&ctx, &call.function.name, &args).await
                }
            } else if call.function.name == "workspace" {
                // Registry primitive: list/info are always fine; create and
                // remove are write-class in plan mode.
                let action = args
                    .get("action")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase();
                if mode == "plan" && (action == "create" || action == "remove") {
                    ToolOutcome::err(
                        "[BLOCKED] Plan mode is read-only. Type /build to leave plan mode. (workspace create/remove is a write-class action.)",
                    )
                } else {
                    workspaces::workspace(&ctx, &args).await
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
            if let Some(writer) = rollout.as_mut() {
                rollout_trace::record_function_output(
                    &ctx.store,
                    writer,
                    &ctx.chat_id,
                    &call.id,
                    &call.function.name,
                    &outcome.content,
                    outcome.is_error,
                )
                .await;
            }

            if outcome.is_error {
                consecutive_errors += 1;
            } else {
                consecutive_errors = 0;
            }
            if consecutive_errors > 0 && consecutive_errors.is_multiple_of(ERROR_NUDGE_INTERVAL) {
                messages.push(ChatMessage::user(format!(
                    "[RETRY GUIDANCE] {consecutive_errors} consecutive tool errors. Do not repeat the same call. \
                     Read the exact error, fix the arguments, or switch to an alternative tool (read/grep/glob, \
                     bash vs job, tool_search for MCP). If the blocker is user-owned, ask once; otherwise continue."
                )));
            }
            if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                let message =
                    format!("Run stopped: {MAX_CONSECUTIVE_ERRORS} consecutive tool errors.");
                if let Some(writer) = rollout.as_mut() {
                    fail_with_trace(
                        &ctx,
                        &mut state,
                        writer,
                        &turn_id,
                        message.clone(),
                        &text,
                        &reasoning,
                        None,
                    )
                    .await;
                } else {
                    fail(&ctx, &mut state, message.clone()).await;
                }
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
        maybe_compact(
            &ctx,
            &mut messages,
            &affinity,
            measured_tokens,
            rollout.as_mut(),
        )
        .await;
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
    if ctx.depth == 0 {
        return Box::pin(run_agent_inner(ctx, prompt));
    }
    register_child_run(&ctx);
    let chat_id = ctx.chat_id.clone();
    Box::pin(async move {
        let _live = ChildRunGuard(chat_id);
        run_agent_inner(ctx, prompt).await
    })
}

/// Tools that may run inside a parallel batch. Same-path calls are grouped and
/// serialized; shell, subagents, group and schedule tools stay sequential.
fn parallel_safe(name: &str) -> bool {
    matches!(
        name,
        "read"
            | "read_file"
            | "edit"
            | "edit_file"
            | "glob"
            | "grep"
            | "search_code"
            | "web_search"
            | "web_fetch"
            | "skill"
            | "memory"
            | "read_pdf"
            | "files_changed"
    )
}

/// Codex parity: shell calls participate in a parallel batch only when the
/// command is on the strict safe-command allowlist. A mutating command keeps
/// the whole batch sequential.
fn call_supports_parallel(name: &str, args: &Value) -> bool {
    if name == "bash" {
        let command = args.get("command").and_then(Value::as_str).unwrap_or("");
        let background = args
            .get("run_in_background")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let process_action = args
            .get("process_action")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty());
        return !background && !process_action && stark_policy::is_read_only_command(command);
    }
    parallel_safe(name)
}

/// Serialization key for the parallel batch: file tools share a key per path,
/// everything else is independent.
fn resource_key(name: &str, args: &Value, index: usize) -> String {
    if matches!(
        name,
        "read" | "read_file" | "edit" | "edit_file" | "read_pdf"
    ) && let Some(path) = args
        .get("file_path")
        .or_else(|| args.get("path"))
        .and_then(Value::as_str)
    {
        return format!("path:{}", path.trim().to_ascii_lowercase());
    }
    format!("call:{index}")
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

/// Does this provider error mean "the prompt no longer fits the window"?
fn is_context_overflow(error: &impl std::fmt::Display) -> bool {
    let text = error.to_string().to_ascii_lowercase();
    text.contains("context length")
        || text.contains("maximum context")
        || text.contains("too many tokens")
        || text.contains("context_length_exceeded")
}

/// Compact unconditionally (reactive recovery from a context-window 400).
async fn force_compact(
    ctx: &RunContext,
    messages: &mut Vec<ChatMessage>,
    affinity: &str,
    writer: Option<&mut stark_store::rollout::RolloutWriter>,
) -> bool {
    let policy = compaction::CompactionPolicy::from_env(&ctx.scope, &ctx.model);
    ctx.emit_delta(Delta::status(json!({
        "kind": "compaction_started",
        "reason": "context-overflow",
    })));
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
        ctx.emit_delta(Delta::status(json!({
            "kind": "compaction_failed",
            "reason": "context-overflow",
        })));
        return false;
    };
    if compaction::estimate_tokens(&outcome.messages) >= compaction::estimate_tokens(messages) {
        ctx.emit_delta(Delta::status(json!({
            "kind": "compaction_failed",
            "reason": "no-reduction",
        })));
        return false;
    }
    *messages = outcome.messages;
    let _ = ctx
        .store
        .append_message_with_metadata(
            &ctx.chat_id,
            "system",
            "Session compacted",
            false,
            Some(&json!({
                "kind": "compaction",
                "tokens_before": outcome.tokens_before,
                "tokens_after": outcome.tokens_after,
                "input_tokens": outcome.input_tokens,
                "output_tokens": outcome.output_tokens,
            })),
        )
        .await;
    let _ = ctx
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
        .await;
    if let Some(writer) = writer {
        rollout_trace::record_compacted(
            &ctx.store,
            writer,
            &ctx.chat_id,
            &outcome.summary,
            outcome.retained_tail.clone(),
        )
        .await;
    }
    ctx.emit_delta(Delta::status(json!({
        "kind": "compaction_completed",
        "reason": "context-overflow",
        "tokens_before": outcome.tokens_before,
        "tokens_after": outcome.tokens_after,
        "input_tokens": outcome.input_tokens,
        "output_tokens": outcome.output_tokens,
    })));
    true
}

async fn maybe_compact(
    ctx: &RunContext,
    messages: &mut Vec<ChatMessage>,
    affinity: &str,
    measured_tokens: Option<usize>,
    writer: Option<&mut stark_store::rollout::RolloutWriter>,
) {
    let policy = compaction::CompactionPolicy::from_env(&ctx.scope, &ctx.model);
    // Provider-reported usage beats the character estimate whenever we have it.
    let tokens = measured_tokens
        .filter(|tokens| *tokens > 0)
        .unwrap_or_else(|| compaction::estimate_tokens(messages));
    if tokens >= policy.hard_limit_tokens {
        tracing::warn!(
            "chat {} is above the compaction hard limit ({} tokens)",
            ctx.chat_id,
            tokens
        );
    }
    if !compaction::should_compact_with(messages, &policy, tokens) {
        return;
    }
    ctx.emit_delta(Delta::status(json!({
        "kind": "compaction_started",
        "reason": "context-limit",
        "tokens_before": tokens,
    })));
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
        ctx.emit_delta(Delta::status(json!({
            "kind": "compaction_failed",
            "reason": "summarize-failed",
        })));
        return;
    };
    if compaction::estimate_tokens(&outcome.messages) >= policy.trigger_tokens {
        tracing::warn!(
            "chat {} compaction did not reduce the context below the trigger; keeping the previous history",
            ctx.chat_id
        );
        ctx.emit_delta(Delta::status(json!({
            "kind": "compaction_failed",
            "reason": "no-reduction",
            "tokens_before": tokens,
        })));
        return;
    }
    *messages = outcome.messages;
    if let Err(error) = ctx
        .store
        .append_message_with_metadata(
            &ctx.chat_id,
            "system",
            "Session compacted",
            false,
            Some(&json!({
                "kind": "compaction",
                "tokens_before": outcome.tokens_before,
                "tokens_after": outcome.tokens_after,
                "input_tokens": outcome.input_tokens,
                "output_tokens": outcome.output_tokens,
            })),
        )
        .await
    {
        tracing::warn!(
            "failed to persist compacted marker for {}: {error}",
            ctx.chat_id
        );
    }
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
    if let Some(writer) = writer {
        rollout_trace::record_compacted(
            &ctx.store,
            writer,
            &ctx.chat_id,
            &outcome.summary,
            outcome.retained_tail.clone(),
        )
        .await;
    }
    ctx.emit_delta(Delta::status(json!({
        "kind": "compaction_completed",
        "reason": "context-limit",
        "tokens_before": outcome.tokens_before,
        "tokens_after": outcome.tokens_after,
        "input_tokens": outcome.input_tokens,
        "output_tokens": outcome.output_tokens,
    })));
}

#[derive(Default)]
struct PartialCall {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

async fn finish_aborted(
    ctx: &RunContext,
    state: &mut AppState,
    writer: Option<&mut stark_store::rollout::RolloutWriter>,
    partial_text: &str,
    partial_reasoning: &str,
    turn_id: Option<&str>,
) {
    let message = "Run aborted.";
    if let Some(writer) = writer {
        if !partial_reasoning.trim().is_empty() {
            rollout_trace::record_reasoning(&ctx.store, writer, &ctx.chat_id, partial_reasoning)
                .await;
        }
        if !partial_text.trim().is_empty() {
            rollout_trace::record_assistant(
                &ctx.store,
                writer,
                &ctx.chat_id,
                partial_text,
                &ctx.model,
                None,
            )
            .await;
        }
        rollout_trace::record_warning(&ctx.store, writer, &ctx.chat_id, message).await;
        rollout_trace::record_turn_aborted(
            &ctx.store,
            writer,
            &ctx.chat_id,
            turn_id,
            "cancelled",
            None,
        )
        .await;
    }
    // Keep the partial response as an assistant projection, then a durable
    // aborted marker. The UI renders the marker as a session divider and the
    // latest-message metadata drives the card's "Aborted" status.
    if !partial_text.trim().is_empty() || !partial_reasoning.trim().is_empty() {
        let metadata = rollout_trace::projection_metadata(partial_reasoning, &[], true);
        let _ = ctx
            .store
            .append_message_with_metadata(
                &ctx.chat_id,
                "assistant",
                partial_text,
                false,
                Some(&metadata),
            )
            .await;
    }
    let _ = ctx
        .store
        .append_message_with_metadata(
            &ctx.chat_id,
            "assistant",
            "Session aborted",
            false,
            Some(&json!({
                "kind": "aborted",
                "stopped": true,
                "reason": "cancelled",
            })),
        )
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

#[allow(clippy::too_many_arguments)]
async fn fail_with_trace(
    ctx: &RunContext,
    state: &mut AppState,
    writer: &mut stark_store::rollout::RolloutWriter,
    turn_id: &str,
    message: String,
    partial_text: &str,
    partial_reasoning: &str,
    code: Option<&str>,
) {
    state.active_node = "error".to_string();
    state.error = Some(message.clone());
    ctx.emit_state(state);
    // The partial work stays: reasoning and text are recorded first, then the
    // error event. Nothing is replaced by the error.
    if !partial_reasoning.trim().is_empty() {
        rollout_trace::record_reasoning(&ctx.store, writer, &ctx.chat_id, partial_reasoning).await;
    }
    if !partial_text.trim().is_empty() {
        rollout_trace::record_assistant(
            &ctx.store,
            writer,
            &ctx.chat_id,
            partial_text,
            &ctx.model,
            None,
        )
        .await;
    }
    rollout_trace::record_error(&ctx.store, writer, &ctx.chat_id, &message, code).await;
    rollout_trace::record_turn_complete(
        &ctx.store,
        writer,
        &ctx.chat_id,
        turn_id,
        if partial_text.trim().is_empty() {
            None
        } else {
            Some(partial_text)
        },
        Some((message.clone(), code.map(str::to_string))),
    )
    .await;
    // Projection keeps the same order: partial assistant output, then the
    // error as its own `error` role (the UI renders it inline, never as a
    // banner that replaces history).
    if !partial_text.trim().is_empty() || !partial_reasoning.trim().is_empty() {
        let metadata = rollout_trace::projection_metadata(partial_reasoning, &[], true);
        let _ = ctx
            .store
            .append_message_with_metadata(
                &ctx.chat_id,
                "assistant",
                partial_text,
                false,
                Some(&metadata),
            )
            .await;
    }
    let _ = ctx
        .store
        .append_message(&ctx.chat_id, "error", &message, false)
        .await;
    ctx.emit_delta(Delta::end("").with_agent(ctx.agent_label()));
}

async fn fail(ctx: &RunContext, state: &mut AppState, message: String) {
    state.active_node = "error".to_string();
    state.error = Some(message.clone());
    ctx.emit_state(state);
    let _ = ctx
        .store
        .append_message(&ctx.chat_id, "error", &message, false)
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
        config: None,
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
                json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"apply_patch","arguments":"{\"input\":\"*** Begin Patch\n*** Add File: big.txt\n+partial"}}]},"finish_reason":null}]}),
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

    async fn mock_text_truncated_provider(
        axum::extract::State(counter): axum::extract::State<Arc<AtomicUsize>>,
        Json(_body): Json<Value>,
    ) -> Response {
        let turn = counter.fetch_add(1, Ordering::SeqCst);
        let payload = if turn == 0 {
            vec![
                json!({"choices":[{"delta":{"content":"Now I'll build"},"finish_reason":null}]}),
                json!({"choices":[{"delta":{},"finish_reason":"length"}]}),
            ]
        } else {
            vec![
                json!({"choices":[{"delta":{"content":" the page."}}]}),
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

    async fn mock_parallel_provider(
        axum::extract::State(counter): axum::extract::State<Arc<AtomicUsize>>,
        Json(_body): Json<Value>,
    ) -> Response {
        let turn = counter.fetch_add(1, Ordering::SeqCst);
        let payload = if turn == 0 {
            vec![
                json!({"choices":[{"delta":{"tool_calls":[
                    {"index":0,"id":"c1","function":{"name":"read","arguments":"{\"file_path\":\"a.txt\"}"}},
                    {"index":1,"id":"c2","function":{"name":"read","arguments":"{\"file_path\":\"b.txt\"}"}}
                ]},"finish_reason":null}]}),
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
            config: None,
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
    async fn text_output_limit_continues_instead_of_ending_the_run() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let counter = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/chat/completions", post(mock_text_truncated_provider))
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
            chat_id: "text-truncated-chat".to_string(),
            workspace_id: "_general".to_string(),
            workspace: dir.path().to_path_buf(),
            role: Some("General".to_string()),
            scope: "general".to_string(),
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
            config: None,
            on_job_done: None,
        };
        run_agent(ctx, "build the page".to_string()).await.unwrap();

        assert_eq!(counter.load(Ordering::SeqCst), 2);
        let final_state = latest.lock().unwrap().clone().expect("final state");
        assert_eq!(
            final_state.outputs["agent_report"].as_str().unwrap(),
            "Now I'll build the page."
        );
    }

    #[test]
    fn codex_safe_bash_is_parallel_but_mutating_bash_is_not() {
        assert!(call_supports_parallel(
            "bash",
            &json!({"command": "ls -la", "description": "list"})
        ));
        assert!(call_supports_parallel(
            "bash",
            &json!({"command": "git status --short", "description": "status"})
        ));
        assert!(!call_supports_parallel(
            "bash",
            &json!({"command": "rm -rf build", "description": "remove"})
        ));
        assert!(!call_supports_parallel(
            "bash",
            &json!({
                "command": "ls",
                "description": "background",
                "run_in_background": true
            })
        ));
    }

    #[tokio::test]
    async fn parallel_reads_preserve_call_order() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let counter = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/chat/completions", post(mock_parallel_provider))
            .with_state(counter.clone());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "alpha").unwrap();
        std::fs::write(dir.path().join("b.txt"), "beta").unwrap();
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
            chat_id: "parallel-chat".to_string(),
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
            config: None,
            on_job_done: None,
        };
        run_agent(ctx, "read both".to_string()).await.unwrap();

        // Both reads ran in one batch; results are appended in call order.
        let entries = store
            .read_session_entries("_general", dir.path(), "parallel-chat")
            .await
            .unwrap();
        let results: Vec<(String, String)> = entries
            .iter()
            .filter(|entry| entry["role"] == "toolResult")
            .map(|entry| {
                (
                    entry["tool_call_id"].as_str().unwrap_or("").to_string(),
                    entry["content"].as_str().unwrap_or("").to_string(),
                )
            })
            .collect();
        assert_eq!(results.len(), 2, "{results:?}");
        assert_eq!(results[0].0, "c1");
        assert!(results[0].1.contains("alpha"), "{results:?}");
        assert_eq!(results[1].0, "c2");
        assert!(results[1].1.contains("beta"), "{results:?}");
    }

    async fn mock_write_batch_provider(
        axum::extract::State(counter): axum::extract::State<Arc<AtomicUsize>>,
        Json(_body): Json<Value>,
    ) -> Response {
        let turn = counter.fetch_add(1, Ordering::SeqCst);
        let p1 = json!({"input": "*** Begin Patch
*** Add File: a.txt
+one
*** End Patch"})
        .to_string();
        let p2 = json!({"input": "*** Begin Patch
*** Add File: b.txt
+two
*** End Patch"})
        .to_string();
        let p3 = json!({"input": "*** Begin Patch
*** Update File: a.txt
@@
-one
+three
*** End Patch"})
        .to_string();
        let payload = if turn == 0 {
            vec![
                json!({"choices":[{"delta":{"tool_calls":[
                    {"index":0,"id":"w1","function":{"name":"apply_patch","arguments":p1}},
                    {"index":1,"id":"w2","function":{"name":"apply_patch","arguments":p2}},
                    {"index":2,"id":"w3","function":{"name":"apply_patch","arguments":p3}}
                ]},"finish_reason":null}]}),
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

    #[tokio::test]
    async fn patch_writes_serialize_same_path() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let counter = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/chat/completions", post(mock_write_batch_provider))
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
            chat_id: "write-batch".to_string(),
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
            config: None,
            on_job_done: None,
        };
        run_agent(ctx, "write files".to_string()).await.unwrap();

        // Different files were written; the two writes to a.txt ran in order,
        // so the last one wins instead of racing.
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "three
"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("b.txt")).unwrap(),
            "two
"
        );
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
            config: None,
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
            config: None,
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
            config: None,
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

    #[test]
    fn child_run_registry_exposes_live_handle_and_cancels() {
        let (tx, _) = broadcast::channel(8);
        let cancel = CancellationToken::new();
        child_runs().lock().unwrap().insert(
            "child-live-registry".to_string(),
            ChildRunHandle {
                tx,
                latest: Arc::new(Mutex::new(None)),
                cancel: cancel.clone(),
                started_at: "2026-01-01T00:00:00Z".to_string(),
            },
        );
        assert!(child_run_active("child-live-registry"));
        assert!(subscribe_child_run("child-live-registry").is_some());
        assert_eq!(
            child_run_started_at("child-live-registry").as_deref(),
            Some("2026-01-01T00:00:00Z")
        );
        assert!(cancel_child_run("child-live-registry"));
        assert!(cancel.is_cancelled());
        unregister_child_run("child-live-registry");
        assert!(!child_run_active("child-live-registry"));
    }

    #[test]
    fn internal_followups_are_never_user_visible() {
        assert!(is_internal_followup("[DELEGATION RESULT] job done"));
        assert!(is_internal_followup(
            "[AGENT RESULT] workspace:_general:dev · Developer · completed chat: del-1 4 Review the child result and continue."
        ));
        assert!(is_internal_followup(
            "[GROUP RESULT] project=p item=i (t) status=completed\n\nreport"
        ));
        assert!(!is_internal_followup("Hello"));
        assert!(!is_internal_followup("[TASK] something"));
    }

    #[test]
    fn session_messages_skip_legacy_internal_wakes() {
        let entries = vec![
            json!({"type": "message", "role": "user", "content": "do the math"}),
            json!({"type": "message", "role": "user", "content": "[AGENT RESULT] workspace:_general:developer · Developer · completed chat: del-1 4 Review the child result and continue."}),
            json!({"type": "message", "role": "user", "content": "[GROUP RESULT] project=p item=i (t) status=completed\n\nreport"}),
            json!({"type": "message", "role": "assistant", "content": "done"}),
        ];
        let messages = session_messages(&entries);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content.as_deref(), Some("do the math"));
        assert_eq!(messages[1].content.as_deref(), Some("done"));
    }

    #[test]
    fn context_overflow_errors_are_detected() {
        assert!(is_context_overflow(&"maximum context length exceeded"));
        assert!(is_context_overflow(
            &"This model's context length is 128000"
        ));
        assert!(is_context_overflow(&"context_length_exceeded"));
        assert!(!is_context_overflow(&"rate limited (HTTP 429)"));
        assert!(!is_context_overflow(&"connection lost"));
    }

    #[test]
    fn session_messages_resume_from_latest_compaction() {
        let entries = vec![
            json!({"type": "message", "role": "user", "content": "old work"}),
            json!({"type": "message", "role": "assistant", "content": "old reply"}),
            json!({"type": "compaction", "summary": "## Objective\n- coffee site"}),
            json!({"type": "message", "role": "toolResult", "tool_call_id": "orphan", "content": "stale"}),
            json!({"type": "message", "role": "user", "content": "continue"}),
        ];
        let messages = session_messages(&entries);
        // Checkpoint summary first, the earlier turns gone, and the orphan
        // tool result dropped so the provider never sees an unanswered tool.
        assert_eq!(messages.len(), 2);
        assert!(
            messages[0]
                .content
                .as_deref()
                .unwrap()
                .contains("coffee site")
        );
        assert_eq!(messages[1].content.as_deref(), Some("continue"));
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
            config: None,
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
            config: None,
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
