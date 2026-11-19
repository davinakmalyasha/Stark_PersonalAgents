//! Conversation compaction.
//!
//! The shape follows Stark's checkpoint model:
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
use stark_providers::{ChatMessage, Provider, ProviderEvent, ProviderSession, Usage};
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
        // A large window can afford more recent verbatim context; 15k is the
        // provider default, 30k keeps working state for 500k-class models.
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
/// reply: 15% of the window, or 16k, whichever is larger.
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
    pub tokens_after: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
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
        return attach_images(
            json!({
                "type": "message",
                "role": "toolResult",
                "tool_call_id": message.tool_call_id,
                "content": message.content.clone().unwrap_or_default(),
            }),
            message,
        );
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
        return attach_images(
            json!({
                "type": "message",
                "role": "assistant",
                "content": blocks,
            }),
            message,
        );
    }
    attach_images(
        json!({
            "type": "message",
            "role": message.role,
            "content": message.content.clone().unwrap_or_default(),
        }),
        message,
    )
}

/// Images ride on the entry beside the text so a retained tail survives
/// compaction with its visuals intact (Codex parity: image budget). Entries
/// without images are unchanged, keeping old readers compatible.
fn attach_images(mut entry: Value, message: &ChatMessage) -> Value {
    if message.images.is_empty() {
        return entry;
    }
    if let Some(object) = entry.as_object_mut() {
        object.insert(
            "images".to_string(),
            Value::Array(
                message
                    .images
                    .iter()
                    .map(|image| json!({"url": image.url, "note": image.note}))
                    .collect(),
            ),
        );
    }
    entry
}

/// Cap on the retained tail so one compaction cannot blow the next window.
/// Newest entries win; pairs are never split: a kept output pulls its call
/// (plus the adjacent outputs of that call) back in.
const MAX_RETAINED_TAIL_CHARS: usize = 60_000;

fn cap_retained_tail(entries: Vec<Value>) -> Vec<Value> {
    if entries.is_empty() {
        return entries;
    }
    let sizes: Vec<usize> = entries
        .iter()
        .map(|entry| entry.to_string().len())
        .collect();
    let mut keep = vec![false; entries.len()];
    let mut used = 0usize;
    // Newest entry is always kept: an empty tail would discard the turn the
    // model needs to continue.
    for index in (0..entries.len()).rev() {
        if used + sizes[index] > MAX_RETAINED_TAIL_CHARS && used > 0 {
            break;
        }
        keep[index] = true;
        used += sizes[index];
    }
    // Pair completion: a kept output without its call pulls the call (and
    // the call's adjacent outputs) back in. Call entries are small; the
    // budget is a soft cap here.
    let mut call_index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (index, entry) in entries.iter().enumerate() {
        for (id, _) in tool_calls_in(entry) {
            call_index.entry(id).or_insert(index);
        }
    }
    loop {
        let mut changed = false;
        for index in 0..entries.len() {
            if !keep[index] || entry_role(&entries[index]) != "toolResult" {
                continue;
            }
            let Some(call_id) = entries[index].get("tool_call_id").and_then(Value::as_str) else {
                continue;
            };
            let Some(&defining) = call_index.get(call_id) else {
                continue;
            };
            if keep[defining] {
                continue;
            }
            keep[defining] = true;
            changed = true;
            // The call's other outputs sit right behind it; keep them too
            // so the group replays whole.
            let mut next = defining + 1;
            while next < entries.len() && entry_role(&entries[next]) == "toolResult" {
                keep[next] = true;
                next += 1;
            }
        }
        if !changed {
            break;
        }
    }
    entries
        .into_iter()
        .enumerate()
        .filter_map(|(index, entry)| keep[index].then_some(entry))
        .collect()
}

fn entry_role(entry: &Value) -> &str {
    entry.get("role").and_then(Value::as_str).unwrap_or("")
}

/// `(call_id, name)` pairs defined by an assistant entry.
fn tool_calls_in(entry: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if entry_role(entry) != "assistant" {
        return out;
    }
    if let Some(blocks) = entry.get("content").and_then(Value::as_array) {
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("toolCall") {
                continue;
            }
            if let (Some(id), Some(name)) = (
                block.get("id").and_then(Value::as_str),
                block.get("name").and_then(Value::as_str),
            ) {
                out.push((id.to_string(), name.to_string()));
            }
        }
    }
    out
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
        reasoning_effort: None,
    };
    let (summary, usage) = summarize(provider, cancel, &messages[1..cut], policy, session).await?;
    // Codex keeps content-filter guidance as a developer fragment across
    // compaction. Re-attach the newest blocks to the retained context so a
    // retry after compaction still knows why the previous sample was blocked.
    let guidance = retained_guidance(&messages[1..cut]);
    let tail = &messages[cut..];
    let mut tail_text = String::new();
    for fragment in &guidance {
        tail_text.push_str(fragment.content.as_deref().unwrap_or(""));
        tail_text.push('\n');
    }
    tail_text.push_str(&tail_transcript(tail));
    let checkpoint = format_checkpoint(&summary, &tail_text);
    let compacted = vec![messages[0].clone(), ChatMessage::user(checkpoint)];
    let mut retained_entries: Vec<Value> = guidance.iter().map(message_to_entry).collect();
    retained_entries.extend(tail.iter().map(message_to_entry));
    let retained_tail = cap_retained_tail(retained_entries);
    let tokens_after = estimate_tokens(&compacted);
    Some(CompactionOutcome {
        messages: compacted,
        summary,
        retained_tail,
        tokens_before,
        tokens_after,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
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

/// Marker shared with `content_filter_guidance` in the turn loop.
const CONTENT_FILTER_GUIDANCE_MARKER: &str = "<content_filter_guidance>";

fn is_content_filter_guidance(message: &ChatMessage) -> bool {
    message.content.as_deref().is_some_and(|content| {
        content
            .trim_start()
            .starts_with(CONTENT_FILTER_GUIDANCE_MARKER)
    })
}

/// Newest guidance blocks kept across compaction (bounded like Codex's
/// contextual fragments).
fn retained_guidance(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    const MAX_FRAGMENTS: usize = 4;
    let mut fragments: Vec<ChatMessage> = messages
        .iter()
        .filter(|message| is_content_filter_guidance(message))
        .cloned()
        .collect();
    if fragments.len() > MAX_FRAGMENTS {
        fragments.drain(..fragments.len() - MAX_FRAGMENTS);
    }
    fragments
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
) -> Option<(String, Usage)> {
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
    let (mut text, mut usage) = stream_summary(provider, cancel, &request, session).await?;
    if !has_summary_heading(&text) {
        let mut retry = request;
        retry.push(ChatMessage::user(NUDGE));
        let (retry_text, retry_usage) = stream_summary(provider, cancel, &retry, session).await?;
        text = retry_text;
        usage.input_tokens = usage.input_tokens.saturating_add(retry_usage.input_tokens);
        usage.output_tokens = usage
            .output_tokens
            .saturating_add(retry_usage.output_tokens);
    }
    if !has_summary_heading(&text) {
        return None;
    }
    let limit = (policy.summary_output_tokens * 4).min(MAX_SUMMARY_OUTPUT_CHARS);
    Some((text.chars().take(limit).collect(), usage))
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
) -> Option<(String, Usage)> {
    let mut stream = provider
        .stream_chat_for_session(request, &[], 0.0, session)
        .await
        .ok()?;
    let mut text = String::new();
    let mut usage = Usage::default();
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
            Some(Ok(ProviderEvent::Usage(value))) => usage = value,
            Some(Ok(ProviderEvent::Done { .. })) | None => break,
            Some(Ok(_)) => {}
            Some(Err(_)) => return None,
        }
    }
    let text = text.trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some((text, usage))
    }
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
    // Images never survive as bytes in a text transcript; keep their notes
    // so the summary still knows what the model saw.
    for image in &message.images {
        let note = if image.note.trim().is_empty() {
            "(image)".to_string()
        } else {
            clip_text(image.note.trim(), clip.min(500))
        };
        parts.push(format!("[Attached image]: {note}"));
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

    fn pair_entries(call_id: &str, output_len: usize) -> Vec<Value> {
        let call = ChatMessage::assistant_tool_calls(
            None,
            vec![ToolCall::function(
                call_id,
                "read",
                json!({"file_path": "a.txt"}).to_string(),
            )],
        );
        let output = ChatMessage::tool(call_id, "x".repeat(output_len));
        vec![message_to_entry(&call), message_to_entry(&output)]
    }

    #[test]
    fn retained_tail_never_splits_a_pair() {
        // One pair (~30KB output) exceeds the 60KB cap only when combined
        // with two more pairs: the newest pair wins, and if its output is
        // kept its call is pulled back in.
        let mut entries = Vec::new();
        for id in ["old-1", "old-2", "new"] {
            entries.extend(pair_entries(id, 30_000));
        }
        let capped = cap_retained_tail(entries);
        let ids: Vec<String> = capped
            .iter()
            .filter_map(|entry| {
                if entry_role(entry) == "toolResult" {
                    entry
                        .get("tool_call_id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                } else {
                    None
                }
            })
            .collect();
        assert!(ids.contains(&"new".to_string()), "{ids:?}");
        // Every kept output still has its call entry.
        for id in &ids {
            assert!(
                capped.iter().any(|entry| tool_calls_in(entry)
                    .iter()
                    .any(|(call_id, _)| call_id == id)),
                "output {id} lost its call"
            );
        }
    }

    #[test]
    fn retained_tail_keeps_newest_entry_over_budget() {
        let entries = pair_entries("only", 100_000);
        let capped = cap_retained_tail(entries);
        assert!(!capped.is_empty());
    }

    #[test]
    fn retained_tail_empty_stays_empty() {
        assert!(cap_retained_tail(Vec::new()).is_empty());
    }

    #[test]
    fn images_survive_entry_round_trip() {
        use stark_providers::ImagePart;
        let message = ChatMessage::user_with_images(
            "look",
            vec![ImagePart {
                url: "data:image/png;base64,abc".to_string(),
                note: "screen".to_string(),
            }],
        );
        let entry = message_to_entry(&message);
        assert_eq!(entry["images"][0]["note"], "screen");
        let rendered = render_message(&message, 0, 500);
        assert!(rendered.contains("[Attached image]: screen"), "{rendered}");
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
        let mut messages = conversation(20);
        messages.insert(
            1,
            ChatMessage::user(
                "<content_filter_guidance>\nblocked once\n</content_filter_guidance>",
            ),
        );
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
        let checkpoint = outcome.messages[1].content.as_deref().unwrap();
        assert!(
            checkpoint.contains("<content_filter_guidance>"),
            "{checkpoint}"
        );
        assert!(checkpoint.contains("blocked once"), "{checkpoint}");
        assert!(
            outcome
                .retained_tail
                .iter()
                .any(|entry| entry.to_string().contains("content_filter_guidance")),
            "{:?}",
            outcome.retained_tail
        );
    }

    #[test]
    fn retained_guidance_is_bounded_to_the_newest_four() {
        let mut messages = vec![ChatMessage::system("system")];
        for index in 0..6 {
            messages.push(ChatMessage::user(format!(
                "<content_filter_guidance>\nblock {index}\n</content_filter_guidance>"
            )));
            messages.push(ChatMessage::assistant("ok"));
        }
        let guidance = retained_guidance(&messages);
        assert_eq!(guidance.len(), 4);
        assert!(guidance[0].content.as_deref().unwrap().contains("block 2"));
        assert!(guidance[3].content.as_deref().unwrap().contains("block 5"));
    }
}
