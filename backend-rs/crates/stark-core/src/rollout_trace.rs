//! Rollout recording for agent runs.
//!
//! The rollout JSONL file is the canonical trace. `chats.db` stays as a
//! projection for lists and search. Every user turn, assistant message,
//! reasoning burst, tool call, tool result, usage record, compaction, error,
//! and warning is appended as it happens, so a crash or network failure
//! leaves the partial work intact.

use serde_json::{Value, json};
use stark_providers::ChatMessage;
use stark_store::rollout::{
    CompactedPayload, EventMsgPayload, ResponseItemPayload, RolloutItem, RolloutLine,
    RolloutWriter, TokenUsagePayload,
};

/// Rebuilds provider messages from a rollout. Reasoning, warnings, errors,
/// and usage are model-invisible and skipped. A compaction becomes the
/// checkpoint user message the model already expects.
pub fn messages_from_rollout(lines: &[RolloutLine]) -> Vec<ChatMessage> {
    let mut messages = Vec::new();
    // Skip everything up to and including the newest compaction; its summary
    // replaces the earlier history.
    let mut start = 0;
    if let Some(index) = lines
        .iter()
        .rposition(|line| matches!(line.item, RolloutItem::Compacted { .. }))
        && let RolloutItem::Compacted { payload } = &lines[index].item
        && !payload.summary.trim().is_empty()
    {
        messages.push(ChatMessage::user(format!(
            "<conversation-checkpoint>\n{}\n</conversation-checkpoint>",
            payload.summary
        )));
        start = index + 1;
    } else if let Some(index) = lines
        .iter()
        .rposition(|line| matches!(line.item, RolloutItem::Compacted { .. }))
    {
        start = index + 1;
    }
    for line in &lines[start..] {
        match &line.item {
            RolloutItem::ResponseItem { payload, .. } => match payload {
                ResponseItemPayload::Message { role, content, .. } => {
                    let text = content_text(content);
                    if role == "user" {
                        messages.push(ChatMessage::user(text));
                    } else if role == "assistant" {
                        if !text.trim().is_empty() {
                            messages.push(ChatMessage::assistant(&text));
                        }
                    } else if role == "toolResult" || role == "tool" {
                        // Tool outputs are re-attached by call below.
                    }
                }
                ResponseItemPayload::FunctionCall {
                    call_id,
                    name,
                    arguments,
                    ..
                } => {
                    messages.push(ChatMessage::assistant_tool_calls(
                        None,
                        vec![stark_providers::ToolCall::function(
                            call_id.clone(),
                            name.clone(),
                            arguments.clone(),
                        )],
                    ));
                }
                ResponseItemPayload::FunctionCallOutput {
                    call_id, output, ..
                } => {
                    messages.push(ChatMessage::tool(call_id, output));
                }
                ResponseItemPayload::Reasoning { .. } => {}
            },
            RolloutItem::TurnContext { .. }
            | RolloutItem::SessionMeta { .. }
            | RolloutItem::EventMsg { .. }
            | RolloutItem::TokenUsageRecord { .. }
            | RolloutItem::Compacted { .. } => {}
        }
    }
    // Drop trailing tool calls with no matching output (crash mid-tool).
    // Sending an unanswered call would make the provider reject the history;
    // the next turn re-issues the work instead. The file itself is unchanged.
    while messages.last().is_some_and(|message| {
        message
            .tool_calls
            .as_ref()
            .is_some_and(|calls| !calls.is_empty())
    }) {
        messages.pop();
    }
    messages
}

fn content_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => {
            let mut out = String::new();
            for block in blocks {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    out.push_str(text);
                } else if let Some(text) = block.get("content").and_then(Value::as_str) {
                    out.push_str(text);
                }
            }
            out
        }
        other => {
            if other.is_null() {
                String::new()
            } else {
                other.to_string()
            }
        }
    }
}

/// Appends an item and records the index row. Failures are logged, never fatal.
pub async fn record(
    store: &stark_store::ChatStore,
    writer: &mut RolloutWriter,
    chat_id: &str,
    item: RolloutItem,
) {
    let ordinal = writer.append(item).await.unwrap_or(0);
    let relative = writer.relative_path(store.harness_root());
    let _ = store
        .record_rollout(chat_id, &relative, ordinal as i64, None)
        .await;
}

pub async fn record_user(
    store: &stark_store::ChatStore,
    writer: &mut RolloutWriter,
    chat_id: &str,
    content: &str,
) {
    record(
        store,
        writer,
        chat_id,
        RolloutItem::ResponseItem {
            payload: ResponseItemPayload::Message {
                id: None,
                role: "user".to_string(),
                content: Value::String(content.to_string()),
                provider: None,
                model: None,
                stop_reason: None,
            },
            metadata: None,
        },
    )
    .await;
}

pub async fn record_assistant(
    store: &stark_store::ChatStore,
    writer: &mut RolloutWriter,
    chat_id: &str,
    content: &str,
    model: &str,
    stop_reason: Option<String>,
) {
    if content.trim().is_empty() {
        return;
    }
    record(
        store,
        writer,
        chat_id,
        RolloutItem::ResponseItem {
            payload: ResponseItemPayload::Message {
                id: None,
                role: "assistant".to_string(),
                content: Value::String(content.to_string()),
                provider: Some("stark-rs".to_string()),
                model: Some(model.to_string()),
                stop_reason,
            },
            metadata: None,
        },
    )
    .await;
}

pub async fn record_reasoning(
    store: &stark_store::ChatStore,
    writer: &mut RolloutWriter,
    chat_id: &str,
    text: &str,
) {
    if text.trim().is_empty() {
        return;
    }
    record(
        store,
        writer,
        chat_id,
        RolloutItem::ResponseItem {
            payload: ResponseItemPayload::Reasoning {
                id: None,
                summary: vec![stark_store::rollout::ReasoningSummary {
                    text: text.to_string(),
                }],
                content: Some(text.to_string()),
            },
            metadata: None,
        },
    )
    .await;
}

pub async fn record_function_call(
    store: &stark_store::ChatStore,
    writer: &mut RolloutWriter,
    chat_id: &str,
    call_id: &str,
    name: &str,
    arguments: &str,
) {
    record(
        store,
        writer,
        chat_id,
        RolloutItem::ResponseItem {
            payload: ResponseItemPayload::FunctionCall {
                id: None,
                call_id: call_id.to_string(),
                name: name.to_string(),
                arguments: arguments.to_string(),
            },
            metadata: None,
        },
    )
    .await;
}

pub async fn record_function_output(
    store: &stark_store::ChatStore,
    writer: &mut RolloutWriter,
    chat_id: &str,
    call_id: &str,
    name: &str,
    output: &str,
    is_error: bool,
) {
    record(
        store,
        writer,
        chat_id,
        RolloutItem::ResponseItem {
            payload: ResponseItemPayload::FunctionCallOutput {
                call_id: call_id.to_string(),
                name: name.to_string(),
                output: output.to_string(),
                is_error,
            },
            metadata: None,
        },
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
pub async fn record_usage(
    store: &stark_store::ChatStore,
    writer: &mut RolloutWriter,
    chat_id: &str,
    input: u64,
    output: u64,
    cached: Option<u64>,
    reasoning: Option<u64>,
    total: Option<u64>,
    cost: Option<f64>,
) {
    record(
        store,
        writer,
        chat_id,
        RolloutItem::TokenUsageRecord {
            payload: TokenUsagePayload {
                input_tokens: input,
                output_tokens: output,
                cached_tokens: cached,
                reasoning_tokens: reasoning,
                total_tokens: total,
                cost,
            },
        },
    )
    .await;
}

pub async fn record_compacted(
    store: &stark_store::ChatStore,
    writer: &mut RolloutWriter,
    chat_id: &str,
    summary: &str,
    retained_tail: Vec<Value>,
) {
    record(
        store,
        writer,
        chat_id,
        RolloutItem::Compacted {
            payload: CompactedPayload {
                summary: summary.to_string(),
                retained_tail,
            },
        },
    )
    .await;
}

pub async fn record_error(
    store: &stark_store::ChatStore,
    writer: &mut RolloutWriter,
    chat_id: &str,
    message: &str,
    code: Option<&str>,
) {
    record(
        store,
        writer,
        chat_id,
        RolloutItem::EventMsg {
            payload: EventMsgPayload::Error {
                message: message.to_string(),
                code: code.map(str::to_string),
            },
        },
    )
    .await;
}

pub async fn record_warning(
    store: &stark_store::ChatStore,
    writer: &mut RolloutWriter,
    chat_id: &str,
    message: &str,
) {
    record(
        store,
        writer,
        chat_id,
        RolloutItem::EventMsg {
            payload: EventMsgPayload::Warning {
                message: message.to_string(),
            },
        },
    )
    .await;
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

pub async fn record_turn_started(
    store: &stark_store::ChatStore,
    writer: &mut RolloutWriter,
    chat_id: &str,
    turn_id: &str,
    model: &str,
) {
    record(
        store,
        writer,
        chat_id,
        RolloutItem::EventMsg {
            payload: EventMsgPayload::TurnStarted {
                turn_id: turn_id.to_string(),
                root_turn_id: None,
                started_at: Some(now_secs()),
            },
        },
    )
    .await;
    // Turn context is a separate item so resume can find the model.
    record(
        store,
        writer,
        chat_id,
        RolloutItem::TurnContext {
            payload: stark_store::rollout::TurnContextPayload {
                turn_id: turn_id.to_string(),
                model: Some(model.to_string()),
            },
        },
    )
    .await;
}

pub async fn record_turn_complete(
    store: &stark_store::ChatStore,
    writer: &mut RolloutWriter,
    chat_id: &str,
    turn_id: &str,
    last_message: Option<&str>,
    error: Option<(String, Option<String>)>,
) {
    record(
        store,
        writer,
        chat_id,
        RolloutItem::EventMsg {
            payload: EventMsgPayload::TurnComplete {
                turn_id: turn_id.to_string(),
                last_agent_message: last_message.map(str::to_string),
                error: error.map(|(message, code)| stark_store::rollout::TurnErrorDetails {
                    message,
                    code,
                }),
                started_at: None,
                completed_at: Some(now_secs()),
            },
        },
    )
    .await;
}

pub async fn record_turn_aborted(
    store: &stark_store::ChatStore,
    writer: &mut RolloutWriter,
    chat_id: &str,
    turn_id: Option<&str>,
    reason: &str,
    error: Option<(String, Option<String>)>,
) {
    record(
        store,
        writer,
        chat_id,
        RolloutItem::EventMsg {
            payload: EventMsgPayload::TurnAborted {
                turn_id: turn_id.map(str::to_string),
                reason: reason.to_string(),
                error: error.map(|(message, code)| stark_store::rollout::TurnErrorDetails {
                    message,
                    code,
                }),
                started_at: None,
                completed_at: Some(now_secs()),
            },
        },
    )
    .await;
}

/// Legacy wrapper removed: call `record_turn_started`, `record_turn_complete`,
/// or `record_turn_aborted` directly so the turn lifecycle is explicit.
/// Resume helper: the rollout file is never mutated here. Dangling tool
/// calls are dropped on read by `messages_from_rollout`, so the next turn
/// simply re-issues the work. Kept for call-site compatibility.
pub async fn ensure_resumable(
    _store: &stark_store::ChatStore,
    _harness_root: &std::path::Path,
    _chat_id: &str,
) {
}

/// Metadata stored on the `chats.db` projection so reloads show the trace.
pub fn projection_metadata(thinking: &str, segments: &[Value], stopped: bool) -> Value {
    let mut metadata = json!({});
    if !thinking.trim().is_empty() {
        metadata["thinking"] = Value::String(thinking.to_string());
    }
    if !segments.is_empty() {
        metadata["liveTimeline"] = Value::Array(segments.to_vec());
    }
    if stopped {
        metadata["stopped"] = Value::Bool(true);
    }
    metadata
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollout_messages_skip_reasoning_and_events() {
        let lines = vec![
            RolloutLine {
                timestamp: "t0".to_string(),
                ordinal: Some(0),
                item: RolloutItem::SessionMeta {
                    payload: stark_store::rollout::SessionMetaPayload {
                        session_id: "c".to_string(),
                        id: "c".to_string(),
                        timestamp: "t0".to_string(),
                        cwd: "%PROJECTS_ROOT%/api".to_string(),
                        workspace_id: "ws-1".to_string(),
                        rel: Some("api".to_string()),
                        model: None,
                        provider: None,
                        originator: None,
                    },
                },
            },
            RolloutLine {
                timestamp: "t1".to_string(),
                ordinal: Some(1),
                item: RolloutItem::ResponseItem {
                    payload: ResponseItemPayload::Message {
                        id: None,
                        role: "user".to_string(),
                        content: Value::String("hi".to_string()),
                        provider: None,
                        model: None,
                        stop_reason: None,
                    },
                    metadata: None,
                },
            },
            RolloutLine {
                timestamp: "t2".to_string(),
                ordinal: Some(2),
                item: RolloutItem::ResponseItem {
                    payload: ResponseItemPayload::Reasoning {
                        id: None,
                        summary: vec![stark_store::rollout::ReasoningSummary {
                            text: "thinking".to_string(),
                        }],
                        content: None,
                    },
                    metadata: None,
                },
            },
        ];
        let messages = messages_from_rollout(&lines);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "user");
    }

    #[test]
    fn compaction_replaces_earlier_history() {
        let lines = vec![
            RolloutLine {
                timestamp: "t0".to_string(),
                ordinal: Some(0),
                item: RolloutItem::ResponseItem {
                    payload: ResponseItemPayload::Message {
                        id: None,
                        role: "user".to_string(),
                        content: Value::String("old".to_string()),
                        provider: None,
                        model: None,
                        stop_reason: None,
                    },
                    metadata: None,
                },
            },
            RolloutLine {
                timestamp: "t1".to_string(),
                ordinal: Some(1),
                item: RolloutItem::Compacted {
                    payload: CompactedPayload {
                        summary: "summary".to_string(),
                        retained_tail: Vec::new(),
                    },
                },
            },
            RolloutLine {
                timestamp: "t2".to_string(),
                ordinal: Some(2),
                item: RolloutItem::ResponseItem {
                    payload: ResponseItemPayload::Message {
                        id: None,
                        role: "user".to_string(),
                        content: Value::String("new".to_string()),
                        provider: None,
                        model: None,
                        stop_reason: None,
                    },
                    metadata: None,
                },
            },
        ];
        let messages = messages_from_rollout(&lines);
        assert_eq!(messages.len(), 2);
        assert!(
            messages[0]
                .content
                .as_deref()
                .unwrap_or("")
                .contains("summary")
        );
    }

    #[test]
    fn dangling_tool_call_is_dropped_on_resume() {
        let call = RolloutLine {
            timestamp: "t0".to_string(),
            ordinal: Some(0),
            item: RolloutItem::ResponseItem {
                payload: ResponseItemPayload::FunctionCall {
                    id: None,
                    call_id: "call-1".to_string(),
                    name: "read_file".to_string(),
                    arguments: "{}".to_string(),
                },
                metadata: None,
            },
        };
        let output = RolloutLine {
            timestamp: "t1".to_string(),
            ordinal: Some(1),
            item: RolloutItem::ResponseItem {
                payload: ResponseItemPayload::FunctionCallOutput {
                    call_id: "call-1".to_string(),
                    name: "read_file".to_string(),
                    output: "ok".to_string(),
                    is_error: false,
                },
                metadata: None,
            },
        };
        // Matched pair survives.
        let messages = messages_from_rollout(&[call.clone(), output.clone()]);
        assert!(
            messages
                .iter()
                .any(|m| m.tool_call_id.as_deref() == Some("call-1"))
        );

        // Trailing call without output is dropped so the provider never sees
        // an unanswered tool call.
        let dangling = RolloutLine {
            timestamp: "t2".to_string(),
            ordinal: Some(2),
            item: RolloutItem::ResponseItem {
                payload: ResponseItemPayload::FunctionCall {
                    id: None,
                    call_id: "call-2".to_string(),
                    name: "read_file".to_string(),
                    arguments: "{}".to_string(),
                },
                metadata: None,
            },
        };
        let messages = messages_from_rollout(&[call.clone(), output.clone(), dangling]);
        assert!(!messages.iter().any(|m| {
            m.tool_calls
                .as_ref()
                .is_some_and(|calls| calls.iter().any(|c| c.id == "call-2"))
        }));
    }
}
