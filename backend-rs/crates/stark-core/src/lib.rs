use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use futures_util::StreamExt;
use serde_json::{Value, json};
use stark_contract::{AppState, Delta, ServerEvent};
use stark_providers::{ChatMessage, Provider, ProviderEvent, ToolCall};
use stark_store::{ChatStore, StoreError};
use stark_tools::{QuestionBroker, ToolContext};
use thiserror::Error;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

const MAX_STEPS: usize = 12;
const HISTORY_LIMIT: usize = 40;

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
    pub workspace: PathBuf,
    pub model: String,
    pub temperature: f64,
    pub provider: Provider,
    pub store: ChatStore,
    pub questions: Arc<QuestionBroker>,
    pub tx: broadcast::Sender<ServerEvent>,
    pub latest: Arc<Mutex<Option<AppState>>>,
    pub cancel: CancellationToken,
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
}

pub async fn run_agent(ctx: RunContext, prompt: String) -> Result<(), RunError> {
    let history = ctx
        .store
        .history(&ctx.chat_id, HISTORY_LIMIT)
        .await
        .unwrap_or_default();

    ctx.store
        .ensure_chat(
            &ctx.chat_id,
            ctx.workspace.to_string_lossy().as_ref(),
            &ctx.model,
            &prompt,
        )
        .await?;
    ctx.store
        .append_message(&ctx.chat_id, "user", &prompt, false)
        .await?;
    ctx.store
        .append_session_entry(
            &ctx.workspace,
            &ctx.chat_id,
            json!({"type": "message", "role": "user", "content": prompt.clone()}),
        )
        .await?;

    let workspace_label = ctx.workspace.to_string_lossy().to_string();
    let mut messages = vec![ChatMessage::system(format!(
        "You are Stark, a coding agent working in {workspace_label}. \
         Use the provided tools to inspect and change files. Read before editing. \
         Never invent tool results. Keep the final answer concise."
    ))];
    for message in history {
        if message.role == "user" || message.role == "assistant" {
            messages.push(ChatMessage {
                role: message.role,
                content: Some(message.content),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            });
        }
    }
    messages.push(ChatMessage::user(&prompt));

    let mut state = AppState {
        active_node: "agent".to_string(),
        next_agent: String::new(),
        client_request: prompt.clone(),
        project_path: workspace_label.clone(),
        selected_model: ctx.model.clone(),
        selected_temp: ctx.temperature,
        outputs: json!({}),
        ..AppState::default()
    };
    ctx.emit_state(&mut state);

    let tool_specs = stark_tools::tool_specs();
    let tools = ToolContext::new(
        ctx.workspace.clone(),
        ctx.chat_id.clone(),
        ctx.questions.clone(),
    );

    for _step in 0..MAX_STEPS {
        if ctx.cancel.is_cancelled() {
            finish_aborted(&ctx, &mut state).await;
            return Ok(());
        }

        let mut stream = match ctx
            .provider
            .stream_chat(&messages, &tool_specs, ctx.temperature)
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
                    ctx.emit_delta(Delta::text(chunk));
                }
                Ok(ProviderEvent::Reasoning(chunk)) => {
                    reasoning.push_str(&chunk);
                    ctx.emit_delta(Delta::reasoning(chunk));
                }
                Ok(ProviderEvent::Usage(usage)) => {
                    state.token_usage.total_input_tokens += usage.input_tokens;
                    state.token_usage.total_output_tokens += usage.output_tokens;
                    state.token_usage.calls.push(json!({
                        "input_tokens": usage.input_tokens,
                        "output_tokens": usage.output_tokens,
                        "total_tokens": usage.total_tokens,
                    }));
                    ctx.emit_delta(Delta::usage(
                        json!({
                            "input_tokens": usage.input_tokens,
                            "output_tokens": usage.output_tokens,
                            "total_tokens": usage.total_tokens,
                        })
                        .to_string(),
                    ));
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
                    .append_session_entry(
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
            state.active_node = "done".to_string();
            state.outputs = json!({"agent_report": text});
            ctx.emit_state(&mut state);
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

        for call in tool_calls {
            if ctx.cancel.is_cancelled() {
                finish_aborted(&ctx, &mut state).await;
                return Ok(());
            }

            let args: Value =
                serde_json::from_str(&call.function.arguments).unwrap_or_else(|_| json!({}));
            let args_text = serde_json::to_string(&args).unwrap_or_default();
            ctx.emit_delta(Delta::tool_started(&call.function.name, &args_text));

            let started = Instant::now();
            let outcome = stark_tools::dispatch(&call.function.name, &args, &tools).await;
            let elapsed = started.elapsed().as_millis() as u64;
            let preview: String = outcome.content.chars().take(300).collect();
            ctx.emit_delta(Delta::tool_finished(
                &call.function.name,
                &args_text,
                !outcome.is_error,
                elapsed,
                preview,
            ));

            messages.push(ChatMessage::tool(&call.id, &outcome.content));
            let _ = ctx
                .store
                .append_session_entry(
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
        }

        if !reasoning.is_empty() {
            state
                .thoughts
                .insert(format!("step-{}", state.state_version), reasoning);
        }
    }

    let message = format!("Stopped after {MAX_STEPS} steps without a final answer.");
    state.error = Some(message.clone());
    ctx.store
        .append_message(&ctx.chat_id, "assistant", &message, false)
        .await?;
    ctx.emit_state(&mut state);
    Err(RunError::Message(message))
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
        .append_session_entry(
            &ctx.workspace,
            &ctx.chat_id,
            json!({"type": "message", "role": "assistant", "content": message, "stop_reason": "aborted"}),
        )
        .await;
    state.active_node = "aborted".to_string();
    state.error = Some(message.to_string());
    ctx.emit_state(state);
    ctx.emit_delta(Delta::end(""));
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
    ctx.emit_delta(Delta::end(""));
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
            workspace: dir.path().to_path_buf(),
            model: "mock".to_string(),
            temperature: 0.0,
            provider,
            store: store.clone(),
            questions: Arc::new(QuestionBroker::new()),
            tx,
            latest: latest.clone(),
            cancel: CancellationToken::new(),
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
                .join("chat.jsonl")
                .exists()
        );
    }
}
