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
        // Keep the verbatim tail that compaction captured. Without this the
        // model loses the most recent tool results after a restart.
        for entry in &payload.retained_tail {
            if let Some(message) = entry_message(entry) {
                messages.push(message);
            }
        }
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
    // A crash can leave tool calls without outputs. Do not silently delete
    // the turn: synthesize an explicit retry result for each missing call so
    // the provider accepts the history and the model knows to re-issue the
    // work. This is the Codex-style resumable turn.
    let answered: std::collections::HashSet<String> = messages
        .iter()
        .filter_map(|message| message.tool_call_id.clone())
        .collect();
    let mut missing = Vec::new();
    for message in &messages {
        if let Some(calls) = &message.tool_calls {
            for call in calls {
                if !answered.contains(&call.id) && !missing.contains(&call.id) {
                    missing.push(call.id.clone());
                }
            }
        }
    }
    for call_id in missing {
        messages.push(ChatMessage::tool(
            &call_id,
            "[harness] This tool call was interrupted before it produced output. Re-issue it if it is still needed.",
        ));
    }
    messages
}

fn entry_message(entry: &Value) -> Option<ChatMessage> {
    let role = entry.get("role")?.as_str()?;
    match role {
        "toolResult" | "tool" => {
            let call_id = entry.get("tool_call_id")?.as_str()?;
            let content = entry
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default();
            Some(ChatMessage::tool(call_id, content))
        }
        "assistant" => {
            let content = content_text(entry.get("content").unwrap_or(&Value::Null));
            let mut calls = Vec::new();
            if let Some(blocks) = entry.get("content").and_then(Value::as_array) {
                for block in blocks {
                    if block.get("type").and_then(Value::as_str) == Some("toolCall")
                        && let (Some(id), Some(name)) = (
                            block.get("id").and_then(Value::as_str),
                            block.get("name").and_then(Value::as_str),
                        )
                    {
                        calls.push(stark_providers::ToolCall::function(
                            id,
                            name,
                            block
                                .get("arguments")
                                .map(Value::to_string)
                                .unwrap_or_else(|| "{}".to_string()),
                        ));
                    }
                }
            }
            if calls.is_empty() {
                (!content.trim().is_empty()).then(|| ChatMessage::assistant(content))
            } else {
                Some(ChatMessage::assistant_tool_calls(
                    (!content.trim().is_empty()).then_some(content),
                    calls,
                ))
            }
        }
        "user" => Some(ChatMessage::user(content_text(
            entry.get("content").unwrap_or(&Value::Null),
        ))),
        _ => None,
    }
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
    fn dangling_tool_call_is_resumable() {
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
        // The call is preserved and a synthetic result is attached so the
        // provider accepts the history and the model can re-issue the work.
        assert!(messages.iter().any(|m| {
            m.tool_calls
                .as_ref()
                .is_some_and(|calls| calls.iter().any(|c| c.id == "call-2"))
        }));
        assert!(messages.iter().any(|m| {
            m.tool_call_id.as_deref() == Some("call-2")
                && m.content
                    .as_deref()
                    .is_some_and(|content| content.contains("interrupted"))
        }));
    }
}
