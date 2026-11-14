//! Port of `frontend/src/utils/streamPresentation.ts`.
//!
//! Splits one stream into the two visual lanes the chat renders: the ordered
//! activity feed (reasoning, tools, intermediate updates) and the final
//! assistant answer. Also owns the transport-text cleanups that keep legacy
//! persisted rows readable.

use crate::stream_accum::{collapse_newlines, StreamSegment, TraceStep};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Presentation {
    /// Text that belongs in the normal assistant answer area.
    pub final_text: String,
    /// Ordered reasoning, tool, and intermediate agent updates.
    pub activity: Vec<StreamSegment>,
}

fn text_of(segments: &[StreamSegment]) -> String {
    segments
        .iter()
        .filter_map(|segment| match segment {
            StreamSegment::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn contains_text(segments: &[StreamSegment], text: &str) -> bool {
    if text.trim().is_empty() {
        return false;
    }
    segments
        .iter()
        .filter_map(|segment| match segment {
            StreamSegment::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>()
        .contains(text.trim())
}

/// Strip old reconnect notices persisted as answer text.
pub fn strip_retry_notices(text: &str) -> String {
    if text.is_empty() || !text.to_ascii_lowercase().contains("reconnecting") {
        return text.to_string();
    }
    let (_, rest) = crate::stream_accum::extract_retry_notices(text, 0);
    rest
}

fn clean_stream_segments(segments: &[StreamSegment]) -> Vec<StreamSegment> {
    if !segments.iter().any(|segment| {
        segment
            .text_of()
            .is_some_and(|t| t.to_ascii_lowercase().contains("reconnecting"))
    }) {
        return segments.to_vec();
    }
    segments
        .iter()
        .filter_map(|segment| match segment {
            StreamSegment::Text { text, agent } => {
                let cleaned = strip_retry_notices(text);
                if cleaned.trim().is_empty() {
                    None
                } else {
                    Some(StreamSegment::Text {
                        text: cleaned,
                        agent: agent.clone(),
                    })
                }
            }
            other => Some(other.clone()),
        })
        .collect()
}

/// Some older runs persisted one assistant string containing the handoff and
/// the completion (`Handed to General … ✓ Done — …`).
fn split_handoff_text(text: &str) -> Option<(String, String)> {
    let lower = text.to_ascii_lowercase();
    let mut search = 0;
    while search < lower.len() {
        let done = lower[search..].find("done").map(|i| search + i);
        let completed = lower[search..].find("completed").map(|i| search + i);
        let Some(index) = (match (done, completed) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }) else {
            break;
        };
        let word_end = index
            + if lower[index..].starts_with("completed") {
                9
            } else {
                4
            };
        // Word boundaries.
        let before_ok = index == 0
            || !lower.as_bytes()[index - 1].is_ascii_alphanumeric()
                && lower.as_bytes()[index - 1] != b'_';
        let after_ok = word_end >= lower.len()
            || !lower.as_bytes()[word_end].is_ascii_alphanumeric()
                && lower.as_bytes()[word_end] != b'_';
        if before_ok && after_ok {
            let mut i = word_end;
            let limit = (word_end + 30).min(text.len());
            while i < limit {
                let ch = lower[i..].chars().next().unwrap_or('\0');
                if matches!(ch, '—' | '–' | ':' | '-') {
                    // JS `match.index` starts at the optional ✓ before
                    // Done/Completed; the answer keeps that prefix.
                    let mut start = index;
                    let mut probe = start;
                    while probe > 0 {
                        let prev = text[..probe].chars().next_back().unwrap_or('\0');
                        if prev == ' ' || prev == '\t' {
                            probe -= prev.len_utf8();
                        } else {
                            break;
                        }
                    }
                    if probe > 0 && text[..probe].ends_with('✓') {
                        start = probe - '✓'.len_utf8();
                    }
                    let handoff = text[..start].trim().to_string();
                    let answer = text[start..].trim().to_string();
                    let handoff_lower = handoff.to_ascii_lowercase();
                    let valid_handoff = [
                        "calling",
                        "asked",
                        "sent",
                        "handed",
                        "relay",
                        "answer",
                        "delegat",
                        "working",
                        "wait",
                        "job is out",
                    ]
                    .iter()
                    .any(|needle| handoff_lower.contains(needle));
                    if !handoff.is_empty() && !answer.is_empty() && valid_handoff {
                        return Some((handoff, answer));
                    }
                    break;
                }
                if matches!(ch, '.' | '!' | '?' | '\n') {
                    break;
                }
                i += ch.len_utf8();
            }
        }
        search = word_end;
        if search <= index {
            break;
        }
    }
    None
}

/// Codex-lite sequence formatting: insert paragraph breaks at `✓` turns.
pub fn format_final_sequence(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(text.len() + 8);
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '✓' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            let word: String = chars[j..]
                .iter()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect();
            let word = word.to_ascii_lowercase();
            let turn = matches!(
                word.as_str(),
                "asked"
                    | "done"
                    | "completed"
                    | "handed"
                    | "sent"
                    | "relayed"
                    | "working"
                    | "waiting"
            );
            if turn {
                // Remove the horizontal whitespace run already emitted, unless
                // it follows a newline (the marker already starts a line).
                let mut trimmed = 0;
                while out.ends_with(' ') || out.ends_with('\t') {
                    out.pop();
                    trimmed += 1;
                }
                let at_line_start = out.is_empty() || out.ends_with('\n');
                if !at_line_start {
                    out.push_str("\n\n");
                } else if trimmed == 0 && !out.is_empty() {
                    // Marker at the start of a line: leave it there.
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    // Fallback when no ✓: "..., done. Done — ..." → break before Done.
    if !out.contains('✓') {
        out = break_before_done_sentence(&out);
    }
    collapse_newlines(out.trim(), 2)
}

fn break_before_done_sentence(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut search = 0;
    let mut insert_at = None;
    while let Some(rel) = lower[search..].find("done") {
        let index = search + rel;
        let before_ok = index == 0
            || !lower.as_bytes()[index - 1].is_ascii_alphanumeric()
                && lower.as_bytes()[index - 1] != b'_';
        let word_end = index + 4;
        let after_ok = word_end >= lower.len()
            || !lower.as_bytes()[word_end].is_ascii_alphanumeric()
                && lower.as_bytes()[word_end] != b'_';
        if before_ok && after_ok {
            // Find a delimiter before the next sentence punctuation.
            let mut i = word_end;
            while i < text.len() {
                let ch = lower[i..].chars().next().unwrap_or('\0');
                if matches!(ch, '—' | '–' | ':' | '-') {
                    // Walk back over the whitespace before "Done".
                    let mut start = index;
                    while start > 0 {
                        let prev = text[..start].chars().next_back().unwrap_or('\0');
                        if prev == ' ' || prev == '\t' {
                            start -= prev.len_utf8();
                        } else {
                            break;
                        }
                    }
                    let prev = text[..start].chars().next_back().unwrap_or('\0');
                    if matches!(prev, '.' | '!' | '?') {
                        insert_at = Some(start);
                    }
                    break;
                }
                if matches!(ch, '.' | '!' | '?' | '\n') {
                    break;
                }
                i += ch.len_utf8();
            }
            if insert_at.is_some() {
                break;
            }
        }
        search = word_end;
        if search <= index {
            break;
        }
    }
    if let Some(at) = insert_at {
        let mut out = String::with_capacity(text.len() + 2);
        out.push_str(&text[..at]);
        out.push_str("\n\n");
        out.push_str(&text[at..]);
        out
    } else {
        text.to_string()
    }
}

fn split_leading_handoff_status(text: &str) -> Option<(String, String)> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut status_lines = Vec::new();
    let mut cursor = 0;
    let mut saw_status = false;
    while cursor < lines.len() {
        let line = lines[cursor].trim();
        if line.is_empty() && saw_status {
            cursor += 1;
            continue;
        }
        let lower = line.to_ascii_lowercase();
        let is_status = !line.is_empty()
            && (lower.starts_with("sent ")
                || lower.starts_with("handed ")
                || lower.starts_with("delegated ")
                || lower.starts_with("delegating ")
                || lower.starts_with("the delegation ")
                || lower.starts_with("i'll pass ")
                || lower.starts_with("i’ll pass ")
                || lower.starts_with("waiting ")
                || (lower.starts_with("general ")
                    && (lower.contains("on it")
                        || lower.contains("will report")
                        || lower.contains("running")))
                || (lower.starts_with("the job ")
                    && (lower.contains("running")
                        || lower.contains("handed")
                        || lower.contains("done"))));
        if !is_status {
            break;
        }
        status_lines.push(line);
        saw_status = true;
        cursor += 1;
    }
    let answer = lines[cursor..].join("\n").trim().to_string();
    if status_lines.is_empty() || answer.is_empty() {
        return None;
    }
    Some((status_lines.join("\n"), answer))
}

/// `stripFinalAnswerDupes` — drop activity text that is just an answer echo.
fn strip_final_answer_dupes(segments: Vec<StreamSegment>, final_text: &str) -> Vec<StreamSegment> {
    let normalize = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let final_norm = normalize(final_text);
    if final_norm.is_empty() {
        return segments;
    }
    segments
        .into_iter()
        .filter(|segment| {
            let StreamSegment::Text { text, .. } = segment else {
                return true;
            };
            let seg_norm = normalize(text);
            if seg_norm.chars().count() < 40 {
                return true;
            }
            !(final_norm.contains(&seg_norm) || seg_norm.contains(&final_norm))
        })
        .collect()
}

/// Turn the raw stream into the two visual lanes.
pub fn present_stream(
    segments: &[StreamSegment],
    fallback_text: &str,
    streaming: bool,
    steps: &[TraceStep],
) -> Presentation {
    let safe_segments = clean_stream_segments(segments);

    // While a run is live, render everything in strict chronological order.
    if streaming && !safe_segments.is_empty() {
        return Presentation {
            final_text: String::new(),
            activity: safe_segments,
        };
    }

    let mut final_text = if safe_segments.is_empty() {
        strip_retry_notices(fallback_text.trim())
    } else {
        String::new()
    };
    let mut activity = safe_segments;

    // The final answer is the last contiguous text run, and only when it is
    // actually at the end of the stream.
    if let Some(StreamSegment::Text { .. }) = activity.last() {
        let mut start = activity.len() - 1;
        while start > 0 && matches!(activity[start - 1], StreamSegment::Text { .. }) {
            start -= 1;
        }
        let trailing = text_of(&activity[start..]);
        if !trailing.is_empty() {
            final_text = trailing;
            activity.truncate(start);
        }
    }

    if let Some((handoff, answer)) = split_handoff_text(&final_text) {
        final_text = answer;
        if !contains_text(&activity, &handoff) {
            let first_tool = activity.iter().position(|segment| segment.is_tool());
            let handoff = StreamSegment::Text {
                text: handoff,
                agent: None,
            };
            match first_tool {
                Some(index) => activity.insert(index, handoff),
                None => activity.push(handoff),
            }
        }
    }

    // Older persisted runs sometimes put a pre-result handoff and the child
    // answer in one assistant message.
    if let Some((status, answer)) = split_leading_handoff_status(&final_text) {
        final_text = answer;
        if !contains_text(&activity, &status) {
            activity.push(StreamSegment::Text {
                text: status,
                agent: None,
            });
        }
    }

    final_text = format_final_sequence(&strip_retry_notices(&strip_mailbox_envelope(&final_text)));
    activity = activity
        .into_iter()
        .filter_map(|segment| match segment {
            StreamSegment::Text { text, agent } => {
                let text = strip_retry_notices(&strip_mailbox_envelope(&text));
                if text.trim().is_empty() {
                    None
                } else {
                    Some(StreamSegment::Text { text, agent })
                }
            }
            other => Some(other),
        })
        .collect();
    activity = strip_final_answer_dupes(activity, &final_text);
    Presentation {
        final_text: strip_delegation_narration(&final_text, steps),
        activity,
    }
}

/// Internal wake prompts are transport, never user content.
pub fn is_internal_transport(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with("[DELEGATION RESULT]")
        || trimmed.starts_with("[AGENT RESULT]")
        || trimmed.starts_with("[GROUP RESULT]")
}

/// Drop mailbox envelopes (`[… from …]`, `[TASK] …`) from legacy text.
pub fn strip_mailbox_envelope(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut lines = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        let mailbox = trimmed.starts_with('[')
            && trimmed.ends_with(']')
            && trimmed.to_ascii_lowercase().contains(" from ");
        if mailbox || trimmed.starts_with("[TASK]") {
            continue;
        }
        lines.push(line);
    }
    collapse_newlines(lines.join("\n").trim(), 2)
}

/// Delegation announcements are transport/status text, not the child agent's
/// answer. Only applied when a persisted trace has a child session.
pub fn strip_delegation_narration(text: &str, steps: &[TraceStep]) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut cleaned = strip_mailbox_envelope(text);
    if !steps.iter().any(|step| step.child_id.is_some()) {
        return cleaned;
    }
    let mut kept = Vec::new();
    let mut skipping_spawned = false;
    for line in cleaned.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        if skipping_spawned {
            if trimmed.starts_with('-') || trimmed.starts_with('•') {
                continue;
            }
            skipping_spawned = false;
        }
        let created = lower.starts_with("created an agent")
            || (lower.starts_with("created ")
                && (lower.contains(" with instructions")
                    || lower.contains(" with the instructions")));
        let spawning = lower.starts_with("sub-agent spawning is available")
            || lower.starts_with("subagent spawning is available");
        if created || spawning {
            continue;
        }
        if lower.starts_with("spawned ")
            && (lower.contains("sub-agent") || lower.contains("subagent"))
        {
            skipping_spawned = true;
            continue;
        }
        kept.push(line);
    }
    cleaned = kept.join("\n");

    // A pre-fix parent trace could contain the child's full arithmetic working
    // after its concise answer. Keep the answer paragraph, let the child card
    // lead to the complete transcript.
    let lower = cleaned.to_ascii_lowercase();
    let markers = ["worked breakdown", "column check", "units:", "```"];
    let mut detail_at = None;
    for marker in markers {
        if let Some(at) = lower.find(marker) {
            if at > 0 && detail_at.is_none_or(|current: usize| at < current) {
                detail_at = Some(at);
            }
        }
    }
    if let Some((start, _)) = lower.match_indices("split ").find_map(|(at, _)| {
        let rest = &lower[at + 6..];
        let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 {
            return None;
        }
        rest[digits..]
            .find(" into")
            .map(|offset| (at, at + 6 + digits + offset))
    }) {
        if start > 0 && detail_at.is_none_or(|current| start < current) {
            detail_at = Some(start);
        }
    }
    if let Some(at) = detail_at {
        let concise = cleaned[..at].trim();
        if !concise.is_empty() {
            cleaned = concise.to_string();
        }
    }
    collapse_newlines(cleaned.trim(), 2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream_accum::{LiveToolEvent, ToolStatus};

    fn text(t: &str) -> StreamSegment {
        StreamSegment::Text {
            text: t.to_string(),
            agent: None,
        }
    }

    fn reasoning(t: &str) -> StreamSegment {
        StreamSegment::Reasoning {
            text: t.to_string(),
            agent: None,
        }
    }

    fn tool(name: &str) -> StreamSegment {
        StreamSegment::Tool {
            ev: LiveToolEvent {
                id: name.to_string(),
                name: name.to_string(),
                args: None,
                status: ToolStatus::Done,
                ok: Some(true),
                ms: None,
                preview: None,
                agent: None,
                children: Vec::new(),
            },
        }
    }

    #[test]
    fn live_keeps_everything_in_order() {
        let segments = vec![reasoning("think"), tool("grep"), text("answer")];
        let p = present_stream(&segments, "fallback", true, &[]);
        assert_eq!(p.final_text, "");
        assert_eq!(p.activity, segments);
    }

    #[test]
    fn completed_trailing_text_is_the_answer() {
        let segments = vec![reasoning("think"), tool("grep"), text("answer")];
        let p = present_stream(&segments, "fallback", false, &[]);
        assert_eq!(p.final_text, "answer");
        assert_eq!(p.activity, vec![reasoning("think"), tool("grep")]);
    }

    #[test]
    fn legacy_fallback_uses_content() {
        let p = present_stream(&[], " hello ", false, &[]);
        assert_eq!(p.final_text, "hello");
        assert!(p.activity.is_empty());
    }

    #[test]
    fn duplicate_answer_is_not_in_activity() {
        let answer = "This is the final answer, long enough to be deduplicated by the rule.";
        let segments = vec![text(answer), tool("grep"), text(answer)];
        let p = present_stream(&segments, "", false, &[]);
        assert_eq!(p.final_text, answer);
        assert_eq!(p.activity, vec![tool("grep")]);
    }

    #[test]
    fn handoff_split_keeps_status_in_activity() {
        let p = present_stream(
            &[],
            "Handed to General, working. ✓ Done — Here is the answer.",
            false,
            &[],
        );
        assert_eq!(p.final_text, "✓ Done — Here is the answer.");
        assert!(!p.activity.is_empty());
    }

    #[test]
    fn leading_handoff_status_moves_into_activity() {
        let p = present_stream(
            &[],
            "Sent to General, working on it.\n\nThe full answer.",
            false,
            &[],
        );
        assert_eq!(p.final_text, "The full answer.");
        assert_eq!(p.activity.len(), 1);
        let StreamSegment::Text { text, .. } = &p.activity[0] else {
            panic!("expected status text");
        };
        assert!(text.starts_with("Sent to General"));
    }

    #[test]
    fn mailbox_envelope_removed() {
        assert_eq!(
            strip_mailbox_envelope("[AGENT RESULT from Confucius]\nhello\n[TASK] wake"),
            "hello"
        );
    }

    #[test]
    fn format_sequence_breaks_turns() {
        assert_eq!(
            format_final_sequence("Asked.✓ Done — ok"),
            "Asked.\n\n✓ Done — ok"
        );
    }

    #[test]
    fn internal_transport_detected() {
        assert!(is_internal_transport(" [DELEGATION RESULT] x"));
        assert!(!is_internal_transport("hello"));
    }

    #[test]
    fn delegation_narration_only_with_child_steps() {
        let text = "Created an agent with the instructions: do it\n\nReal answer";
        assert_eq!(strip_delegation_narration(text, &[]), text);
        let steps = vec![TraceStep {
            child_id: Some("c1".into()),
            ..TraceStep::default()
        }];
        assert_eq!(strip_delegation_narration(text, &steps), "Real answer");
    }
}
