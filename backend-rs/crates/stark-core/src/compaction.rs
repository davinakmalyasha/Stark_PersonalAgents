use std::env;

use futures_util::StreamExt;
use serde_json::{Value, json};
use stark_providers::{ChatMessage, Provider, ProviderEvent};
use tokio_util::sync::CancellationToken;

const DEFAULT_TRIGGER_TOKENS: usize = 500_000;
const DEFAULT_HARD_LIMIT_TOKENS: usize = 550_000;
const DEFAULT_SUMMARY_TOKENS: usize = 4_096;
const MAX_SUMMARY_INPUT_CHARS: usize = 120_000;
const MAX_SUMMARY_OUTPUT_CHARS: usize = 16_000;
const MIN_TAIL_TOKENS: usize = 8_000;
const MAX_TAIL_TOKENS: usize = 60_000;

pub struct CompactionPolicy {
    pub trigger_tokens: usize,
    pub hard_limit_tokens: usize,
    pub summary_output_tokens: usize,
    pub tail_tokens: usize,
}

impl CompactionPolicy {
    pub fn from_env(role: &str) -> Self {
        let role_key: String = role
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .collect();
        let trigger_tokens = setting(&role_key, "CHECKPOINT_TOKENS", DEFAULT_TRIGGER_TOKENS);
        let hard_limit_tokens =
            setting(&role_key, "HARD_LIMIT", DEFAULT_HARD_LIMIT_TOKENS).max(trigger_tokens);
        let summary_output_tokens =
            setting(&role_key, "SUMMARY_OUTPUT_TOKENS", DEFAULT_SUMMARY_TOKENS);
        let tail_tokens = setting(
            &role_key,
            "TAIL_TOKENS",
            (trigger_tokens / 4).clamp(MIN_TAIL_TOKENS, MAX_TAIL_TOKENS),
        );
        Self {
            trigger_tokens,
            hard_limit_tokens,
            summary_output_tokens,
            tail_tokens,
        }
    }
}

fn setting(role_key: &str, suffix: &str, default: usize) -> usize {
    let role_name = format!("{role_key}_COMPACTION_{suffix}");
    positive_env(&role_name)
        .unwrap_or_else(|| positive_env(&format!("COMPACTION_{suffix}")).unwrap_or(default))
}

fn positive_env(name: &str) -> Option<usize> {
    env::var(name)
        .ok()?
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
}

pub struct CompactionOutcome {
    pub messages: Vec<ChatMessage>,
    pub summary: String,
    pub retained_tail: Vec<Value>,
    pub tokens_before: usize,
}

pub fn estimate_tokens(messages: &[ChatMessage]) -> usize {
    messages.iter().map(message_tokens).sum()
}

pub fn should_compact(messages: &[ChatMessage], policy: &CompactionPolicy) -> bool {
    estimate_tokens(messages) >= policy.trigger_tokens
}

pub fn message_to_entry(message: &ChatMessage) -> Value {
    if message.role == "tool" {
        return json!({
            "type": "message",
            "role": "toolResult",
            "tool_call_id": message.tool_call_id,
            "content": message.content.clone().unwrap_or_default(),
        });
    }
    if message.role == "assistant"
        && let Some(calls) = &message.tool_calls
    {
        let mut blocks = Vec::new();
        if let Some(text) = message.content.as_deref().filter(|text| !text.is_empty()) {
            blocks.push(json!({"type": "text", "text": text}));
        }
        for call in calls {
            let arguments = serde_json::from_str::<Value>(&call.function.arguments)
                .unwrap_or_else(|_| json!({}));
            blocks.push(json!({
                "type": "toolCall",
                "id": call.id,
                "name": call.function.name,
                "arguments": arguments,
            }));
        }
        return json!({
            "type": "message",
            "role": "assistant",
            "content": blocks,
        });
    }
    json!({
        "type": "message",
        "role": message.role,
        "content": message.content.clone().unwrap_or_default(),
    })
}

pub async fn compact_history(
    provider: &Provider,
    cancel: &CancellationToken,
    messages: &[ChatMessage],
    policy: &CompactionPolicy,
) -> Option<CompactionOutcome> {
    let cut = split_index(messages, policy.tail_tokens)?;
    let tokens_before = estimate_tokens(messages);
    let summary = summarize(provider, cancel, &messages[1..cut], policy).await?;
    let mut compacted = Vec::with_capacity(messages.len() - cut + 2);
    compacted.push(messages[0].clone());
    compacted.push(ChatMessage::system(format!("[CONTEXT SUMMARY]\n{summary}")));
    compacted.extend(messages[cut..].iter().cloned());
    let retained_tail = messages[cut..]
        .iter()
        .map(message_to_entry)
        .collect::<Vec<_>>();
    Some(CompactionOutcome {
        messages: compacted,
        summary,
        retained_tail,
        tokens_before,
    })
}

fn split_index(messages: &[ChatMessage], tail_tokens: usize) -> Option<usize> {
    if messages.len() <= 2 || messages.first()?.role != "system" {
        return None;
    }
    let mut tokens = 0;
    let mut cut = messages.len();
    for index in (1..messages.len()).rev() {
        tokens += message_tokens(&messages[index]);
        if tokens > tail_tokens {
            cut = index;
            break;
        }
    }
    if cut >= messages.len() {
        return None;
    }
    cut = cut.max(1);
    while cut > 1 && messages[cut].role != "user" {
        cut -= 1;
    }
    if cut <= 1 {
        return None;
    }
    Some(cut)
}

fn message_tokens(message: &ChatMessage) -> usize {
    let mut chars = message.content.as_deref().map(str::len).unwrap_or(0);
    if let Some(calls) = &message.tool_calls {
        for call in calls {
            chars += call.function.name.len() + call.function.arguments.len();
        }
    }
    chars.div_ceil(3) + 12
}

async fn summarize(
    provider: &Provider,
    cancel: &CancellationToken,
    messages: &[ChatMessage],
    policy: &CompactionPolicy,
) -> Option<String> {
    let transcript = transcript(messages);
    let request = vec![
        ChatMessage::system(
            "You compress conversation history for an autonomous coding agent. \
             Preserve user goals, decisions, file paths, commands, errors, test results, \
             tool outcomes, and unfinished work. Be factual and compact. Do not call tools.",
        ),
        ChatMessage::user(format!(
            "Summarize the conversation segment below. Return only the summary.\n\n{transcript}"
        )),
    ];
    let mut stream = provider.stream_chat(&request, &[], 0.0).await.ok()?;
    let mut text = String::new();
    loop {
        let event = tokio::select! {
            _ = cancel.cancelled() => return None,
            event = stream.next() => event,
        };
        match event {
            Some(Ok(ProviderEvent::Text(chunk))) => {
                text.push_str(&chunk);
                if text.len() > MAX_SUMMARY_OUTPUT_CHARS {
                    break;
                }
            }
            Some(Ok(ProviderEvent::Done { .. })) | None => break,
            Some(Ok(_)) => {}
            Some(Err(_)) => return None,
        }
    }
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let limit = (policy.summary_output_tokens * 4).min(MAX_SUMMARY_OUTPUT_CHARS);
    Some(text.chars().take(limit).collect())
}

fn transcript(messages: &[ChatMessage]) -> String {
    let mut text = String::new();
    for message in messages {
        let content = message.content.as_deref().unwrap_or("");
        text.push_str(&format!("[{}] {content}", message.role));
        if let Some(calls) = &message.tool_calls {
            for call in calls {
                text.push_str(&format!(
                    "\n  tool_call {} {}",
                    call.function.name, call.function.arguments
                ));
            }
        }
        if let Some(id) = &message.tool_call_id {
            text.push_str(&format!("\n  tool_call_id {id}"));
        }
        text.push('\n');
        if text.len() >= MAX_SUMMARY_INPUT_CHARS {
            break;
        }
    }
    text.chars().take(MAX_SUMMARY_INPUT_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use stark_providers::ToolCall;

    use super::*;

    fn conversation(turns: usize) -> Vec<ChatMessage> {
        let mut messages = vec![ChatMessage::system("system")];
        for index in 0..turns {
            messages.push(ChatMessage::user(format!("user {index}")));
            messages.push(ChatMessage::assistant_tool_calls(
                None,
                vec![ToolCall::function(
                    format!("call-{index}"),
                    "read_file",
                    json!({"file_path": "a.txt"}).to_string(),
                )],
            ));
            messages.push(ChatMessage::tool(format!("call-{index}"), "result"));
            messages.push(ChatMessage::assistant("done"));
        }
        messages
    }

    #[test]
    fn split_keeps_the_system_and_starts_at_a_user_turn() {
        let messages = conversation(20);
        let cut = split_index(&messages, 20).unwrap();
        assert!(cut > 1);
        assert_eq!(messages[cut].role, "user");
    }

    #[test]
    fn estimation_is_positive_and_grows_with_content() {
        let small = vec![ChatMessage::user("hi")];
        let large = vec![ChatMessage::user("word ".repeat(1000))];
        assert!(estimate_tokens(&large) > estimate_tokens(&small));
    }

    #[test]
    fn retained_tool_pairs_survive_serialization() {
        let messages = conversation(3);
        let entry = message_to_entry(&messages[2]);
        assert_eq!(entry["role"], "assistant");
        assert_eq!(entry["content"][0]["type"], "toolCall");
        let tool = message_to_entry(&messages[3]);
        assert_eq!(tool["role"], "toolResult");
        assert_eq!(tool["tool_call_id"], "call-0");
    }

    #[tokio::test]
    async fn compact_history_summarizes_old_turns_and_keeps_tail() {
        use axum::body::Body;
        use axum::http::header;
        use axum::response::Response;
        use axum::routing::post;
        use axum::{Json, Router};

        async fn mock_provider(Json(_body): Json<Value>) -> Response {
            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"SUMMARY\"}}]}

",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}

",
                "data: [DONE]

"
            );
            Response::builder()
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from(body))
                .unwrap()
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route("/chat/completions", post(mock_provider)),
            )
            .await
            .unwrap();
        });
        let provider = Provider::new(stark_providers::ProviderConfig {
            mode: "deepseek".to_string(),
            base_url: format!("http://{address}"),
            api_key: "test".to_string(),
            model: "mock".to_string(),
        })
        .unwrap();
        let policy = CompactionPolicy {
            trigger_tokens: 10,
            hard_limit_tokens: 20,
            summary_output_tokens: 128,
            tail_tokens: 12,
        };
        let messages = conversation(20);
        let outcome = compact_history(&provider, &CancellationToken::new(), &messages, &policy)
            .await
            .expect("compacted");
        assert_eq!(outcome.summary, "SUMMARY");
        assert_eq!(outcome.messages[1].role, "system");
        assert!(
            outcome.messages[1]
                .content
                .as_deref()
                .unwrap()
                .contains("SUMMARY")
        );
        assert!(outcome.retained_tail.len() < messages.len());
        assert_eq!(outcome.retained_tail.first().unwrap()["role"], "user");
    }
}
