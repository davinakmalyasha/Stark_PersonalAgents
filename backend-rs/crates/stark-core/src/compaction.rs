//! Conversation compaction.
//!
//! The shape follows opencode's checkpoint model:
//!
//! 1. When the request approaches the model window, summarize the older
//!    history into a structured Markdown checkpoint.
//! 2. Keep the newest `keep` tokens as a textual `<recent-context>` beside
//!    the summary, so the next request starts with one synthetic user
//!    message instead of a second system prompt.
//! 3. On a later compaction, update the existing checkpoint in place rather
//!    than summarizing a summary.
//!
//! Token counts are estimates: provider usage is not available inside the
//! compaction step, so characters/4 plus a small per-message overhead is used.

use std::env;

use futures_util::StreamExt;
use serde_json::{Value, json};
use stark_providers::{ChatMessage, Provider, ProviderEvent, ProviderSession};
use tokio_util::sync::CancellationToken;

const DEFAULT_CONTEXT_WINDOW: usize = 128_000;
const DEFAULT_OUTPUT_TOKENS: usize = 32_000;
const DEFAULT_SUMMARY_TOKENS: usize = 8_000;
const DEFAULT_KEEP_TOKENS: usize = 15_000;
const RESERVE_SHARE: f64 = 0.15;
const RESERVE_MIN: usize = 16_000;
const MAX_SUMMARY_INPUT_CHARS: usize = 1_500_000;
const MAX_SUMMARY_OUTPUT_CHARS: usize = 32_000;
const MAX_TAIL_MESSAGE_CHARS: usize = 12_000;
const TRANSCRIPT_CLIP: usize = 2_000;

const SUMMARY_TEMPLATE: &str = r#"You MUST use this format for your response (you may omit sections that aren't applicable). Do not include the <template> tags in your response.
<template>
## Objective
- [one or two brief sentences describing what the user is trying to accomplish]

## Requirements
- [constraints, preferences, requirements, and scope boundaries stated by the user, or "(none)"]

## Decisions
- [decisions already made and why, or "(none)"]

## Work State
Break the objective into smaller goals and report which are completed, which are being worked on, and which are blocked.
### Completed
- [goals that have been completed; otherwise "(none)"]

### Active
- [goals currently being worked on; otherwise "(none)"]

### Blocked
- [anything blocking progress, and why; otherwise "(none)"]

## Next Move
1. [ordered list of next actions, or "(none)"]

## Relevant Files
List the files and directories, other than the current working directory, that another agent would need to open to continue this work. Include at most 15, most important first. Do not list every file that was read or changed. Include paths outside the current working directory when relevant. If none, write "(none)".
- `[file or directory path]`: [brief reason it matters]

## Important Context
- [facts the next agent cannot continue without and cannot easily find on its own; or "(none)"]
</template>"#;

const NUDGE: &str = "The previous response did not fill in the required summary template. Do not call tools.      Return the summary as text using the exact section headings from the template.";

const SUMMARY_RULES: &str = r#"Rules:
- Keep each section concise. Use terse, single-line bullets, not prose paragraphs or nested lists.
- Prefer short references over detailed restatement. It is fine to leave out information the next agent can recover from the code or the files listed above.
- Preserve exact file paths, symbols, commands, error strings, URLs, and identifiers.
- Carry forward only user questions or requests that remain unanswered or require further action. Do not repeat ones that newer history has answered or resolved. Preserve exact wording when carrying one forward.
- Preserve consequential workflow state, including whether changes are uncommitted, committed, pushed, under review, or merged.
- Do not mention the summary process or that context was compacted."#;

#[derive(Clone, Debug)]
pub struct CompactionPolicy {
    pub trigger_tokens: usize,
    pub hard_limit_tokens: usize,
    pub summary_output_tokens: usize,
    pub tail_tokens: usize,
}

impl CompactionPolicy {
    pub fn from_env(role: &str, model: &str) -> Self {
        let role_key = env_key(role);
        let model_key = env_key(model);
        let context_window = positive_env(&format!("STARK_MODEL_CONTEXT_{model_key}"))
            .or_else(|| positive_env("STARK_MODEL_CONTEXT"))
            .or_else(|| stark_providers::context_window(model))
            .unwrap_or(DEFAULT_CONTEXT_WINDOW);
        let output_tokens = positive_env(&format!("STARK_MODEL_OUTPUT_{model_key}"))
            .or_else(|| positive_env("STARK_MODEL_OUTPUT"))
            .unwrap_or(DEFAULT_OUTPUT_TOKENS);
        let ceiling = ceiling_for(context_window);
        let trigger_tokens = setting(&role_key, "CHECKPOINT_TOKENS", ceiling);
        let hard_limit_tokens =
            setting(&role_key, "HARD_LIMIT", context_window.max(trigger_tokens));
        let summary_output_tokens = setting(
            &role_key,
            "SUMMARY_OUTPUT_TOKENS",
            output_tokens.clamp(1_024, DEFAULT_SUMMARY_TOKENS),
        );
        // A large window can afford more recent verbatim context; 15k is
        // opencode's default, 30k keeps working state for 500k-class models.
        let default_keep = if context_window >= 400_000 {
            30_000
        } else {
            DEFAULT_KEEP_TOKENS
        };
        let tail_tokens = setting(&role_key, "KEEP_TOKENS", default_keep);
        Self {
            trigger_tokens,
            hard_limit_tokens,
            summary_output_tokens,
            tail_tokens,
        }
    }
}

fn env_key(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn setting(role_key: &str, suffix: &str, default: usize) -> usize {
    let role_name = format!("{role_key}_COMPACTION_{suffix}");
    positive_env(&role_name)
        .or_else(|| positive_env(&format!("COMPACTION_{suffix}")))
        .unwrap_or(default)
}

fn positive_env(name: &str) -> Option<usize> {
    env::var(name)
        .ok()?
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
}

/// The largest prompt the model should receive while leaving room for the
/// reply: 15% of the window, or 16k, whichever is larger. Mirrors opencode's
/// `calculateCeiling`.
pub fn ceiling_for(window: usize) -> usize {
    if window == 0 {
        return 0;
    }
    let reserve =
        ((window as f64 * RESERVE_SHARE).floor() as usize).max(if window >= RESERVE_MIN * 2 {
            RESERVE_MIN
        } else {
            0
        });
    window.saturating_sub(reserve)
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

/// Compaction decision from a known token count (provider usage when
/// available, estimate otherwise).
pub fn should_compact_with(
    _messages: &[ChatMessage],
    policy: &CompactionPolicy,
    tokens: usize,
) -> bool {
    tokens >= policy.trigger_tokens
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
    session_id: &str,
    affinity: &str,
) -> Option<CompactionOutcome> {
    let cut = split_index(messages, policy.tail_tokens)?;
    let tokens_before = estimate_tokens(messages);
    let session = ProviderSession {
        session_id,
        affinity,
    };
    let summary = summarize(provider, cancel, &messages[1..cut], policy, session).await?;
    let tail = &messages[cut..];
    let tail_text = tail_transcript(tail);
    let checkpoint = format_checkpoint(&summary, &tail_text);
    let compacted = vec![messages[0].clone(), ChatMessage::user(checkpoint)];
    let retained_tail = tail.iter().map(message_to_entry).collect::<Vec<_>>();
    Some(CompactionOutcome {
        messages: compacted,
        summary,
        retained_tail,
        tokens_before,
    })
}

pub fn format_checkpoint(summary: &str, recent: &str) -> String {
    format!(
        "<conversation-checkpoint>\n<summary>\n{summary}\n</summary>\n<recent-context>\n{recent}\n</recent-context>\n</conversation-checkpoint>"
    )
}

pub fn is_checkpoint(message: &ChatMessage) -> bool {
    message.role == "user"
        && message.content.as_deref().is_some_and(|content| {
            content
                .trim_start()
                .starts_with("<conversation-checkpoint>")
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
        // Everything fits in the tail. Keep only the latest exchange rather
        // than duplicating the whole conversation beside the summary.
        let latest_user = messages
            .iter()
            .rposition(|message| message.role == "user")?;
        if latest_user <= 1 {
            return None;
        }
        cut = latest_user;
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
    // ~3.6 chars/token: slightly conservative so compaction fires before a
    // provider context overflow on code-heavy transcripts.
    (chars * 10).div_ceil(36) + 12
}

async fn summarize(
    provider: &Provider,
    cancel: &CancellationToken,
    messages: &[ChatMessage],
    policy: &CompactionPolicy,
    session: ProviderSession<'_>,
) -> Option<String> {
    let update = messages.first().is_some_and(is_checkpoint);
    let budget = policy
        .trigger_tokens
        .saturating_mul(3)
        .clamp(64_000, MAX_SUMMARY_INPUT_CHARS);
    let input = transcript(messages, budget);
    if input.trim().is_empty() {
        return None;
    }
    let prompt = build_prompt(update);
    let request = vec![
        ChatMessage::system(
            "You compress conversation history for an autonomous coding agent. \
             Preserve user goals, decisions, file paths, commands, errors, test results, \
             tool outcomes, and unfinished work. Be factual and compact. Do not call tools.",
        ),
        ChatMessage::user(input),
        ChatMessage::user(prompt),
    ];
    let mut text = stream_summary(provider, cancel, &request, session).await?;
    if !has_summary_heading(&text) {
        let mut retry = request;
        retry.push(ChatMessage::user(NUDGE));
        text = stream_summary(provider, cancel, &retry, session).await?;
    }
    if !has_summary_heading(&text) {
        return None;
    }
    let limit = (policy.summary_output_tokens * 4).min(MAX_SUMMARY_OUTPUT_CHARS);
    Some(text.chars().take(limit).collect())
}

fn has_summary_heading(text: &str) -> bool {
    ["## Objective", "## Requirements", "## Work State"]
        .iter()
        .any(|heading| text.contains(heading))
}

async fn stream_summary(
    provider: &Provider,
    cancel: &CancellationToken,
    request: &[ChatMessage],
    session: ProviderSession<'_>,
) -> Option<String> {
    let mut stream = provider
        .stream_chat_for_session(request, &[], 0.0, session)
        .await
        .ok()?;
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
    let text = text.trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

fn build_prompt(update: bool) -> String {
    let mut parts = Vec::new();
    if update {
        parts.push(
            "Update the existing checkpoint in the conversation above into one consolidated summary."
                .to_string(),
        );
        parts.push(
            "Newer history always takes precedence over the existing checkpoint. Preserve previous \
             information unless newer history clearly contradicts, supersedes, resolves, or makes it \
             stale. Reconcile Work State and Next Move: move completed work out of Active, remove \
             resolved blockers and answered questions, and preserve unresolved or pending work."
                .to_string(),
        );
    } else {
        parts.push(
            "Summarize the conversation above into a structured summary that will be given to another \
             agent to resume the work."
                .to_string(),
        );
    }
    parts.push(SUMMARY_TEMPLATE.to_string());
    parts.push(SUMMARY_RULES.to_string());
    parts.push("Do not continue the task or call tools.".to_string());
    parts.push(
        "Return only the structured summary in the requested format. Do not include a preamble, \
         explanation, or other commentary."
            .to_string(),
    );
    parts.join("\n\n")
}

fn tail_transcript(messages: &[ChatMessage]) -> String {
    let mut out = String::new();
    for message in messages {
        let rendered = render_message(message, 0, MAX_TAIL_MESSAGE_CHARS);
        if rendered.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&rendered);
        if out.len() >= MAX_TAIL_MESSAGE_CHARS {
            break;
        }
    }
    out.chars().take(MAX_TAIL_MESSAGE_CHARS).collect()
}

fn transcript(messages: &[ChatMessage], max_chars: usize) -> String {
    for level in 0..=3 {
        let text = messages
            .iter()
            .map(|message| render_message(message, level, TRANSCRIPT_CLIP))
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        if text.len() <= max_chars {
            return text;
        }
    }
    // Last resort: keep the newest messages that fit, with a marker.
    let mut start = 0;
    while start < messages.len() {
        let text = messages[start..]
            .iter()
            .map(|message| render_message(message, 2, 600))
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        if text.len() <= max_chars || start + 1 >= messages.len() {
            return format!("[… older history omitted …]\n{text}");
        }
        start += 1;
    }
    String::new()
}

fn render_message(message: &ChatMessage, level: usize, clip: usize) -> String {
    if message.role == "system" {
        return String::new();
    }
    if level >= 3 && message.role != "user" {
        return String::new();
    }
    let mut parts = Vec::new();
    if let Some(content) = message
        .content
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    {
        let label = match message.role.as_str() {
            "user" => "User",
            "assistant" => "Assistant",
            "tool" => "Tool result",
            other => other,
        };
        let text = if level >= 3 {
            clip_text(content, clip)
        } else {
            content.to_string()
        };
        parts.push(format!("[{label}]: {text}"));
    }
    if let Some(calls) = &message.tool_calls {
        for call in calls {
            if level >= 2 {
                parts.push(format!("[Assistant tool call]: {}", call.function.name));
            } else {
                let input = if level >= 1 {
                    clip_text(&call.function.arguments, clip)
                } else {
                    call.function.arguments.clone()
                };
                parts.push(format!(
                    "[Assistant tool call]: {}({input})",
                    call.function.name
                ));
            }
        }
    }
    parts.join("\n")
}

fn clip_text(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_string();
    }
    let half = limit / 2;
    let start: String = value.chars().take(half).collect();
    let end: String = value
        .chars()
        .rev()
        .take(half)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!(
        "{start}\n[… {} characters omitted …]\n{end}",
        value.len() - limit
    )
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
    fn measured_tokens_drive_the_compaction_decision() {
        let policy = CompactionPolicy {
            trigger_tokens: 1_000,
            hard_limit_tokens: 1_200,
            summary_output_tokens: 4_000,
            tail_tokens: 400,
        };
        assert!(should_compact_with(&[], &policy, 1_200));
        assert!(!should_compact_with(&[], &policy, 900));
    }

    #[test]
    fn ceiling_keeps_fifteen_percent_or_sixteen_k() {
        assert_eq!(ceiling_for(500_000), 425_000);
        assert_eq!(ceiling_for(100_000), 84_000);
        assert_eq!(ceiling_for(20_000), 17_000);
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

    #[test]
    fn checkpoint_is_one_user_message() {
        let checkpoint = format_checkpoint("summary", "recent");
        assert!(checkpoint.starts_with("<conversation-checkpoint>"));
        assert!(checkpoint.contains("<summary>\nsummary\n</summary>"));
        assert!(checkpoint.contains("<recent-context>\nrecent\n</recent-context>"));
    }

    #[tokio::test]
    async fn compact_history_summarizes_old_turns_and_keeps_tail() {
        use axum::body::Body;
        use axum::http::header;
        use axum::response::Response;
        use axum::routing::post;
        use axum::{Json, Router};

        async fn mock_provider(
            headers: axum::http::HeaderMap,
            Json(_body): Json<Value>,
        ) -> Response {
            assert_eq!(
                headers
                    .get("x-opencode-session")
                    .and_then(|value| value.to_str().ok()),
                Some("chat-test")
            );
            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"## Objective\\n- ship it\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n"
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
            mode: "opencode".to_string(),
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
        let outcome = compact_history(
            &provider,
            &CancellationToken::new(),
            &messages,
            &policy,
            "chat-test",
            "chat-test",
        )
        .await
        .expect("compacted");
        assert!(outcome.summary.contains("ship it"));
        assert_eq!(outcome.messages.len(), 2);
        assert_eq!(outcome.messages[1].role, "user");
        assert!(
            outcome.messages[1]
                .content
                .as_deref()
                .unwrap()
                .contains("<conversation-checkpoint>")
        );
        assert!(outcome.retained_tail.len() < messages.len());
    }
}
