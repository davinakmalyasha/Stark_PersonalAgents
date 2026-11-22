//! Shared SSE stream accumulator — direct port of
//! The original design stream accumulator.
//!
//! Every surface (chat, pipeline, subagents) folds deltas through this one
//! implementation so live reasoning, tool pills, retries and the final
//! timeline behave identically. Pure data + `serde_json`: no UI, no async.

use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

/// One tool call in the live timeline (`LiveToolEvent`).
#[derive(Debug, Clone, PartialEq)]
pub struct LiveToolEvent {
    pub id: String,
    pub name: String,
    pub args: Option<String>,
    pub status: ToolStatus,
    pub ok: Option<bool>,
    pub ms: Option<u64>,
    pub preview: Option<String>,
    pub agent: Option<String>,
    pub children: Vec<ChildRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Running,
    Done,
}

impl ToolStatus {
    pub fn is_running(self) -> bool {
        self == ToolStatus::Running
    }
}

/// Structured live child session reference (`ChildReference`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChildRef {
    pub chat_id: String,
    pub job_id: Option<String>,
    pub agent_name: Option<String>,
    pub label: Option<String>,
    pub workspace_id: Option<String>,
}

/// Ordered live timeline (`StreamSegment`).
#[derive(Debug, Clone, PartialEq)]
pub enum StreamSegment {
    Reasoning { text: String, agent: Option<String> },
    Text { text: String, agent: Option<String> },
    Tool { ev: LiveToolEvent },
}

impl StreamSegment {
    pub fn is_tool(&self) -> bool {
        matches!(self, StreamSegment::Tool { .. })
    }

    pub fn text_of(&self) -> Option<&str> {
        match self {
            StreamSegment::Reasoning { text, .. } | StreamSegment::Text { text, .. } => Some(text),
            StreamSegment::Tool { .. } => None,
        }
    }
}

/// Persisted trace step (`TraceStep` in `logParser.ts`), trimmed to the
/// fields the chats UI reads. Legacy rows only; new runs persist
/// `liveTimeline`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TraceStep {
    pub id: String,
    /// `"tool" | "command" | "system" | "thought" | …`
    pub kind: String,
    pub agent: String,
    pub title: String,
    pub content: String,
    pub tool_name: String,
    pub child_id: Option<String>,
    pub label: Option<String>,
    pub child_name: Option<String>,
    pub child_workspace: Option<String>,
    pub is_error: bool,
}

impl TraceStep {
    /// The reference UI `hasRenderableTraceSteps` for one step.
    pub fn renderable(&self) -> bool {
        self.child_id.is_some()
            || self.kind == "tool"
            || self.kind == "command"
            || !self.content.trim().is_empty()
    }
}

/// Reconnect status (`RetryNotice`). Ephemeral transport state.
#[derive(Debug, Clone, PartialEq)]
pub struct RetryNotice {
    pub attempt: u64,
    pub max: u64,
    /// Wall-clock millis when the notice arrived.
    pub at: u64,
}

/// Compaction / context notice (`RunStatusNotice`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunStatusNotice {
    pub kind: String,
    pub reason: Option<String>,
    pub tokens_before: Option<u64>,
    pub tokens_after: Option<u64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

/// Per-run token totals used by the message footer and persistence.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunUsage {
    pub input: u64,
    pub output: u64,
    pub cache_hit: u64,
    pub cost: f64,
}

impl RunUsage {
    pub fn add(&mut self, input: u64, output: u64, cache_hit: u64, cost: f64) {
        self.input += input;
        self.output += output;
        self.cache_hit += cache_hit;
        self.cost += cost;
    }

    /// `TokenUsage` shape the backend persists in `metadata.usage`.
    pub fn to_json(&self) -> Value {
        json!({
            "calls": [],
            "total_input_tokens": self.input,
            "total_output_tokens": self.output,
            "total_cache_hit_tokens": self.cache_hit,
            "total_cache_miss_tokens": self.input.saturating_sub(self.cache_hit),
            "total_cost": self.cost,
        })
    }

    pub fn from_json(v: &Value) -> Self {
        let u = |key: &str| v.get(key).and_then(Value::as_u64).unwrap_or(0);
        Self {
            input: u("total_input_tokens").max(u("input")),
            output: u("total_output_tokens").max(u("output")),
            cache_hit: u("total_cache_hit_tokens").max(u("cached_tokens")),
            cost: v
                .get("total_cost")
                .or_else(|| v.get("cost"))
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
        }
    }
}

/// Tokens/sec smoothing (`RateAccum`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RateAccum {
    pub last_ts: u64,
    pub last_chars: usize,
    pub ema: f64,
}

impl RateAccum {
    pub fn rounded(&self) -> f64 {
        self.ema.round()
    }
}

/// Full live accumulator (`StreamAccum`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StreamAccum {
    pub text: String,
    pub reasoning: String,
    pub agent: String,
    pub segments: Vec<StreamSegment>,
    pub tool_events: Vec<LiveToolEvent>,
    pub retry: Option<RetryNotice>,
    pub status: Option<RunStatusNotice>,
    /// `outputs.agent_report` from the last `state` frame — the persistence
    /// fallback when no text delta arrived.
    pub report: String,
    /// Latest `state.token_usage` payload, preserved verbatim for persistence.
    pub usage_json: Option<Value>,
    /// Per-run totals folded from usage deltas (fallback when no state frame).
    pub usage: RunUsage,
    /// Tok/s smoothing for the live dock pill.
    pub rate: RateAccum,
    /// `state.artifacts` (kept for parity; chats never render the panel, but
    /// The persisted metadata shape matches the reference UI).
    pub artifacts: Option<Value>,
    /// Last backend state error, if any.
    pub state_error: Option<String>,
}

/// One delta frame, normalized from JSON. `usage` is handled separately by
/// The transport layer (it feeds the dock totals too).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Delta {
    pub kind: String,
    pub text: String,
    pub agent: Option<String>,
    pub tool: Option<String>,
    pub args: Option<String>,
    pub ok: Option<bool>,
    pub ms: Option<u64>,
    pub children: Vec<ChildRef>,
    pub status: Option<RunStatusNotice>,
}

impl Delta {
    pub fn from_value(v: &Value) -> Self {
        let s = |key: &str| {
            v.get(key)
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .filter(|s| !s.is_empty())
        };
        let children = v
            .get("children")
            .and_then(Value::as_array)
            .map(|rows| rows.iter().filter_map(parse_child).collect())
            .unwrap_or_default();
        let status = v.get("status").and_then(parse_status);
        Self {
            kind: s("kind").unwrap_or_default(),
            // Durable `tool_finished` rollouts carry `preview`, not `text`.
            text: s("text").or_else(|| s("preview")).unwrap_or_default(),
            agent: s("agent"),
            tool: s("tool"),
            args: s("args"),
            ok: v.get("ok").and_then(Value::as_bool),
            ms: v.get("ms").and_then(Value::as_u64),
            children,
            status,
        }
    }
}

fn parse_child(v: &Value) -> Option<ChildRef> {
    let chat_id = v
        .get("chatId")
        .or_else(|| v.get("chat_id"))
        .and_then(Value::as_str)?
        .trim()
        .to_string();
    if chat_id.is_empty() {
        return None;
    }
    let opt = |key: &str| {
        v.get(key)
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .filter(|s| !s.is_empty())
    };
    Some(ChildRef {
        chat_id,
        job_id: opt("jobId").or_else(|| opt("job_id")),
        agent_name: opt("agentName").or_else(|| opt("agent_name")),
        label: opt("label"),
        workspace_id: opt("workspaceId").or_else(|| opt("workspace_id")),
    })
}

pub fn parse_status(v: &Value) -> Option<RunStatusNotice> {
    let kind = v.get("kind").and_then(Value::as_str)?.to_string();
    Some(RunStatusNotice {
        kind,
        reason: v
            .get("reason")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        tokens_before: v.get("tokens_before").and_then(Value::as_u64),
        tokens_after: v.get("tokens_after").and_then(Value::as_u64),
        input_tokens: v.get("input_tokens").and_then(Value::as_u64),
        output_tokens: v.get("output_tokens").and_then(Value::as_u64),
    })
}

impl StreamAccum {
    fn append_segment(&mut self, kind: SegmentKind, text: &str, agent: Option<&str>) {
        if text.is_empty() {
            return;
        }
        if let Some(last) = self.segments.last_mut() {
            match (last, kind) {
                (StreamSegment::Reasoning { text: t, agent: a }, SegmentKind::Reasoning)
                | (StreamSegment::Text { text: t, agent: a }, SegmentKind::Text) => {
                    t.push_str(text);
                    if let Some(agent) = agent {
                        *a = Some(agent.to_string());
                    }
                    return;
                }
                _ => {}
            }
        }
        let agent = agent.map(ToString::to_string);
        self.segments.push(match kind {
            SegmentKind::Reasoning => StreamSegment::Reasoning {
                text: text.to_string(),
                agent,
            },
            SegmentKind::Text => StreamSegment::Text {
                text: text.to_string(),
                agent,
            },
        });
        self.cap_segments();
    }

    fn cap_segments(&mut self) {
        if self.segments.len() > 300 {
            let excess = self.segments.len() - 300;
            self.segments.drain(..excess);
        }
    }

    fn push_tool(&mut self, ev: LiveToolEvent) {
        self.tool_events.push(ev.clone());
        if self.tool_events.len() > 50 {
            let excess = self.tool_events.len() - 50;
            self.tool_events.drain(..excess);
        }
        self.segments.push(StreamSegment::Tool { ev });
        self.cap_segments();
    }

    fn sync_tool(&mut self, id: &str) {
        let Some(ev) = self.tool_events.iter().find(|e| e.id == id).cloned() else {
            return;
        };
        for segment in &mut self.segments {
            if let StreamSegment::Tool { ev: current } = segment {
                if current.id == id {
                    *current = ev;
                    return;
                }
            }
        }
    }

    /// Clear the live turn before a durable replay rebuilds it.
    ///
    /// Mirrors the TS `reset` delta: transcript content, retry/status, and the
    /// rate history clear; usage/report/artifacts stay because `/api/state`
    /// owns those and replay skips durable usage events.
    pub fn reset(&mut self) {
        self.text.clear();
        self.reasoning.clear();
        self.agent.clear();
        self.segments.clear();
        self.tool_events.clear();
        self.retry = None;
        self.status = None;
        self.rate = RateAccum::default();
    }

    /// Append a completed durable item. Unlike a live delta it is neither a
    /// token fragment nor a retry notice, so it appends verbatim and leaves
    /// The token-rate meter alone.
    pub fn apply_snapshot(&mut self, delta: &Delta) -> bool {
        match delta.kind.as_str() {
            "text_snapshot" => {
                self.retry = None;
                if let Some(agent) = &delta.agent {
                    self.agent = agent.clone();
                }
                self.text.push_str(&delta.text);
                self.append_segment(SegmentKind::Text, &delta.text, delta.agent.as_deref());
                true
            }
            "reasoning_snapshot" => {
                self.retry = None;
                if let Some(agent) = &delta.agent {
                    self.agent = agent.clone();
                }
                self.reasoning.push_str(&delta.text);
                self.append_segment(SegmentKind::Reasoning, &delta.text, delta.agent.as_deref());
                true
            }
            _ => false,
        }
    }

    /// Fold one delta. Returns the rounded tok/s rate (matches
    /// `applyDeltaToAccum`).
    pub fn apply(&mut self, rate: &mut RateAccum, delta: &Delta, now: u64) -> f64 {
        if delta.kind.is_empty() {
            return rate.rounded();
        }
        // A finished compaction notice is one-shot; let it clear as soon as
        // The run produces its next output instead of pinning it all turn.
        if self
            .status
            .as_ref()
            .is_some_and(|status| status.kind != "compaction_started")
            && matches!(
                delta.kind.as_str(),
                "text" | "reasoning" | "tool_started" | "tool_finished"
            )
        {
            self.status = None;
        }
        match delta.kind.as_str() {
            "reasoning" => {
                self.retry = None;
                self.reasoning.push_str(&delta.text);
                self.append_segment(SegmentKind::Reasoning, &delta.text, delta.agent.as_deref());
            }
            "text" => {
                let (retry, rest) = extract_retry_notices(&delta.text, now);
                if let Some(retry) = retry {
                    self.retry = Some(retry);
                }
                if rest.trim().is_empty() {
                    if let Some(agent) = &delta.agent {
                        self.agent = agent.clone();
                    }
                    return rate.rounded();
                }
                // Real output means the stream reconnected.
                self.retry = None;
                let chars = rest.chars().count();
                if rate.last_ts > 0 {
                    let dt = (now.saturating_sub(rate.last_ts).max(16) as f64) / 1000.0;
                    let r = chars as f64 / 4.0 / dt;
                    rate.ema = if rate.ema == 0.0 {
                        r
                    } else {
                        rate.ema * 0.8 + r * 0.2
                    };
                }
                rate.last_ts = now;
                rate.last_chars = chars;
                self.text.push_str(&rest);
                self.append_segment(SegmentKind::Text, &rest, delta.agent.as_deref());
            }
            "status" => {
                if let Some(status) = &delta.status {
                    self.status = Some(status.clone());
                }
            }
            "end" => {
                self.retry = None;
                rate.ema = 0.0;
            }
            "tool_started" => {
                self.retry = None;
                let id = next_tool_id(now);
                self.push_tool(LiveToolEvent {
                    id,
                    name: delta.tool.clone().unwrap_or_else(|| "tool".to_string()),
                    args: delta.args.clone(),
                    status: ToolStatus::Running,
                    ok: None,
                    ms: None,
                    preview: None,
                    agent: delta.agent.clone(),
                    children: Vec::new(),
                });
            }
            "tool_finished" => {
                self.retry = None;
                let name = delta.tool.clone().unwrap_or_else(|| "tool".to_string());
                let open = self
                    .tool_events
                    .iter()
                    .rposition(|ev| ev.status.is_running() && ev.name == name);
                if let Some(index) = open {
                    let children = if delta.children.is_empty() {
                        legacy_child_refs(&delta.text)
                    } else {
                        delta.children.clone()
                    };
                    let ev = &mut self.tool_events[index];
                    ev.status = ToolStatus::Done;
                    ev.ok = delta.ok;
                    ev.ms = delta.ms;
                    ev.preview = Some(delta.text.clone()).filter(|s| !s.is_empty());
                    ev.children = children;
                    if ev.agent.is_none() {
                        ev.agent = delta.agent.clone();
                    }
                    let id = ev.id.clone();
                    self.sync_tool(&id);
                } else {
                    // Finish without a recorded start (mirror attach,
                    // reconnect): keep it.
                    let children = if delta.children.is_empty() {
                        legacy_child_refs(&delta.text)
                    } else {
                        delta.children.clone()
                    };
                    self.push_tool(LiveToolEvent {
                        id: next_tool_id(now),
                        name,
                        args: delta.args.clone(),
                        status: ToolStatus::Done,
                        ok: delta.ok,
                        ms: delta.ms,
                        preview: Some(delta.text.clone()).filter(|s| !s.is_empty()),
                        agent: delta.agent.clone(),
                        children,
                    });
                }
            }
            _ => {}
        }
        if let Some(agent) = &delta.agent {
            self.agent = agent.clone();
        }
        rate.rounded()
    }
}

#[derive(Clone, Copy)]
enum SegmentKind {
    Reasoning,
    Text,
}

static TOOL_SEQ: AtomicU64 = AtomicU64::new(0);

fn next_tool_id(now: u64) -> String {
    let seq = TOOL_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    format!("{now}-{seq}")
}

/// `_{0,2}\s*Reconnecting\s*\(\s*(\d+)\s*\/\s*(\d+)\s*\)\s*[.…]*\s*_{0,2}`
/// case-insensitive. Returns the last match and the cleaned text.
pub fn extract_retry_notices(text: &str, now: u64) -> (Option<RetryNotice>, String) {
    if !text.to_ascii_lowercase().contains("reconnecting") {
        return (None, text.to_string());
    }
    let lower = text.to_ascii_lowercase();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut retry = None;
    let mut search = 0;
    while let Some(rel) = lower[search..].find("reconnecting") {
        let word_start = search + rel;
        let word_end = word_start + "reconnecting".len();
        // JS `_{0,2}\s*` starts at the whitespace run directly before the
        // word, then at up to two underscores before that run — never at
        // whitespace preceding those underscores.
        let mut start = word_start;
        while start > 0 {
            let b = text.as_bytes()[start - 1];
            if b == b' ' || b == b'\t' {
                start -= 1;
            } else {
                break;
            }
        }
        let mut underscores = 0;
        while start > 0 && text.as_bytes()[start - 1] == b'_' && underscores < 2 {
            start -= 1;
            underscores += 1;
        }
        // Forward: whitespace, '(' digits '/' digits ')'.
        let bytes = text.as_bytes();
        let mut i = word_end;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'(' {
            search = word_end;
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let attempt_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == attempt_start {
            search = word_end;
            continue;
        }
        let attempt: u64 = text[attempt_start..i].parse().unwrap_or(0);
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'/' {
            search = word_end;
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let max_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == max_start {
            search = word_end;
            continue;
        }
        let max: u64 = text[max_start..i].parse().unwrap_or(1);
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b')' {
            search = word_end;
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        while i < bytes.len() && (bytes[i] == b'.' || text[i..].starts_with('…')) {
            i += if text[i..].starts_with('…') {
                '…'.len_utf8()
            } else {
                1
            };
        }
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut underscores = 0;
        while i < bytes.len() && bytes[i] == b'_' && underscores < 2 {
            i += 1;
            underscores += 1;
        }
        ranges.push((start, i));
        retry = Some(RetryNotice {
            attempt,
            max,
            at: now,
        });
        search = i.max(word_end);
    }
    if ranges.is_empty() {
        return (None, text.to_string());
    }
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for (start, end) in ranges {
        out.push_str(&text[cursor..start]);
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    // `[ \t]+\n` → `\n`, then collapse 3+ newlines, trim.
    let out = out
        .split('\n')
        .map(|line| line.trim_end_matches([' ', '\t']))
        .collect::<Vec<_>>()
        .join("\n");
    let out = collapse_newlines(&out, 2);
    (retry, out.trim().to_string())
}

/// Collapse runs of more than `max` newline characters into `max`.
pub fn collapse_newlines(text: &str, max: usize) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run = 0;
    for ch in text.chars() {
        if ch == '\n' {
            run += 1;
            if run <= max {
                out.push(ch);
            }
        } else {
            run = 0;
            out.push(ch);
        }
    }
    out
}

/// `legacyChildRefs`: child refs embedded in a finished tool's preview.
pub fn legacy_child_refs(text: &str) -> Vec<ChildRef> {
    let mut refs = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("- ") {
            let chat_at = rest.find("(chat:");
            let chat_end = chat_at.and_then(|at| rest[at..].find(')'));
            if let (Some(chat_at), Some(chat_end)) = (chat_at, chat_end) {
                let chat_id = rest[chat_at + 6..chat_at + chat_end].trim();
                if !chat_id.is_empty() {
                    let before = rest[..chat_at].trim();
                    let parts: Vec<&str> = before.split('·').map(str::trim).collect();
                    if parts.len() >= 3 && parts[0].starts_with("del-") {
                        refs.push(ChildRef {
                            chat_id: chat_id.to_string(),
                            job_id: Some(parts[0].to_string()),
                            agent_name: Some(parts[1].to_string()),
                            label: Some(parts[2].to_string()),
                            workspace_id: None,
                        });
                        continue;
                    }
                }
            }
        }
        if let Some(rest) = trimmed.strip_prefix("[SPAWNED]") {
            let mut chat = String::new();
            let mut name = String::new();
            for part in rest.split(';') {
                let part = part.trim();
                if let Some(v) = part.strip_prefix("child=") {
                    chat = v.trim().to_string();
                } else if let Some(v) = part.strip_prefix("name=") {
                    name = v.trim().to_string();
                }
            }
            if !chat.is_empty() {
                refs.push(ChildRef {
                    chat_id: chat,
                    agent_name: Some(name.clone()).filter(|s| !s.is_empty()),
                    label: Some(name).filter(|s| !s.is_empty()),
                    ..ChildRef::default()
                });
            }
        }
    }
    refs
}

/// Serialize one segment back to the persisted `liveTimeline` shape.
pub fn segment_to_json(segment: &StreamSegment) -> Value {
    match segment {
        StreamSegment::Reasoning { text, agent } => {
            let mut v = json!({"type": "reasoning", "text": text});
            if let Some(agent) = agent {
                v["agent"] = json!(agent);
            }
            v
        }
        StreamSegment::Text { text, agent } => {
            let mut v = json!({"type": "text", "text": text});
            if let Some(agent) = agent {
                v["agent"] = json!(agent);
            }
            v
        }
        StreamSegment::Tool { ev } => {
            let mut event = json!({
                "id": ev.id,
                "name": ev.name,
                "status": if ev.status.is_running() { "running" } else { "done" },
            });
            if let Some(args) = &ev.args {
                event["args"] = json!(args);
            }
            if let Some(ok) = ev.ok {
                event["ok"] = json!(ok);
            }
            if let Some(ms) = ev.ms {
                event["ms"] = json!(ms);
            }
            if let Some(preview) = &ev.preview {
                event["preview"] = json!(preview);
            }
            if let Some(agent) = &ev.agent {
                event["agent"] = json!(agent);
            }
            if !ev.children.is_empty() {
                event["children"] = Value::Array(
                    ev.children
                        .iter()
                        .map(|child| {
                            let mut c = json!({"chatId": child.chat_id});
                            if let Some(v) = &child.job_id {
                                c["jobId"] = json!(v);
                            }
                            if let Some(v) = &child.agent_name {
                                c["agentName"] = json!(v);
                            }
                            if let Some(v) = &child.label {
                                c["label"] = json!(v);
                            }
                            if let Some(v) = &child.workspace_id {
                                c["workspaceId"] = json!(v);
                            }
                            c
                        })
                        .collect(),
                );
            }
            json!({"type": "tool", "ev": event})
        }
    }
}

/// Parse a persisted `liveTimeline` array. Unknown entries are skipped.
pub fn parse_segments(v: &Value) -> Vec<StreamSegment> {
    let Some(rows) = v.as_array() else {
        return Vec::new();
    };
    let mut segments = Vec::new();
    for row in rows {
        match row.get("type").and_then(Value::as_str) {
            Some("reasoning") | Some("text") => {
                let text = row
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if text.is_empty() {
                    continue;
                }
                let agent = row
                    .get("agent")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                segments.push(
                    if row.get("type").and_then(Value::as_str) == Some("reasoning") {
                        StreamSegment::Reasoning { text, agent }
                    } else {
                        StreamSegment::Text { text, agent }
                    },
                );
            }
            Some("tool") => {
                let Some(ev) = row.get("ev") else { continue };
                let Some(name) = ev.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let status = if ev.get("status").and_then(Value::as_str) == Some("running") {
                    ToolStatus::Running
                } else {
                    ToolStatus::Done
                };
                let children = ev
                    .get("children")
                    .and_then(Value::as_array)
                    .map(|rows| rows.iter().filter_map(parse_child).collect())
                    .unwrap_or_default();
                segments.push(StreamSegment::Tool {
                    ev: LiveToolEvent {
                        id: ev
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        name: name.to_string(),
                        args: ev
                            .get("args")
                            .and_then(Value::as_str)
                            .map(ToString::to_string),
                        status,
                        ok: ev.get("ok").and_then(Value::as_bool),
                        ms: ev.get("ms").and_then(Value::as_u64),
                        preview: ev
                            .get("preview")
                            .and_then(Value::as_str)
                            .map(ToString::to_string),
                        agent: ev
                            .get("agent")
                            .and_then(Value::as_str)
                            .map(ToString::to_string),
                        children,
                    },
                });
            }
            _ => {}
        }
    }
    segments
}

/// Parse persisted `metadata.steps` (`TraceStep[]`). Only the fields the
/// chats surfaces consume.
pub fn parse_steps(v: &Value) -> Vec<TraceStep> {
    let Some(rows) = v.as_array() else {
        return Vec::new();
    };
    rows.iter()
        .map(|row| {
            let opt = |key: &str| {
                row.get(key)
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
                    .filter(|s| !s.is_empty())
            };
            TraceStep {
                id: opt("id").unwrap_or_default(),
                kind: opt("type").unwrap_or_default(),
                agent: opt("agent").unwrap_or_default(),
                title: opt("title").unwrap_or_default(),
                content: opt("content").unwrap_or_default(),
                tool_name: opt("toolName").or_else(|| opt("tool")).unwrap_or_default(),
                child_id: opt("childId"),
                label: opt("label"),
                child_name: opt("childName"),
                child_workspace: opt("childWorkspace"),
                is_error: row.get("isError").and_then(Value::as_bool).unwrap_or(false),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(kind: &str, text: &str) -> Delta {
        Delta {
            kind: kind.to_string(),
            text: text.to_string(),
            ..Delta::default()
        }
    }

    #[test]
    fn merges_adjacent_text_and_reasoning() {
        let mut acc = StreamAccum::default();
        let mut rate = RateAccum::default();
        acc.apply(&mut rate, &delta("text", "hel"), 1);
        acc.apply(&mut rate, &delta("text", "lo"), 2);
        acc.apply(&mut rate, &delta("text", " there"), 3);
        assert_eq!(acc.text, "hello there");
        assert_eq!(acc.segments.len(), 1);
        assert_eq!(acc.segments[0].text_of(), Some("hello there"));
    }

    #[test]
    fn retry_notice_is_extracted_not_rendered() {
        let mut acc = StreamAccum::default();
        let mut rate = RateAccum::default();
        // Notices arrive as their own delta; only the notice is removed.
        acc.apply(&mut rate, &delta("text", "_Reconnecting (2/5)…_"), 10);
        assert_eq!(acc.text, "");
        assert_eq!(acc.retry.as_ref().unwrap().attempt, 2);
        assert_eq!(acc.retry.as_ref().unwrap().max, 5);
        // Real output means the stream reconnected.
        acc.apply(&mut rate, &delta("text", "ok"), 20);
        assert_eq!(acc.text, "ok");
        assert!(acc.retry.is_none());
        // Text mixed with a notice still strips the notice.
        let (retry, rest) = extract_retry_notices("before _Reconnecting (2/5)…_ after", 1);
        assert!(retry.is_some());
        assert_eq!(rest, "before  after");
    }

    #[test]
    fn tool_finish_updates_the_segment_too() {
        let mut acc = StreamAccum::default();
        let mut rate = RateAccum::default();
        acc.apply(
            &mut rate,
            &Delta {
                kind: "tool_started".into(),
                tool: Some("grep".into()),
                args: Some("{\"pattern\":\"x\"}".into()),
                ..Delta::default()
            },
            1,
        );
        acc.apply(
            &mut rate,
            &Delta {
                kind: "tool_finished".into(),
                tool: Some("grep".into()),
                ok: Some(false),
                ms: Some(12),
                text: "Error: nope".into(),
                ..Delta::default()
            },
            2,
        );
        assert_eq!(acc.tool_events.len(), 1);
        let ev = &acc.tool_events[0];
        assert_eq!(ev.status, ToolStatus::Done);
        assert_eq!(ev.ok, Some(false));
        assert_eq!(ev.ms, Some(12));
        let StreamSegment::Tool { ev: segment } = &acc.segments[0] else {
            panic!("expected tool segment");
        };
        assert_eq!(segment.status, ToolStatus::Done);
        assert_eq!(segment.preview.as_deref(), Some("Error: nope"));
    }

    #[test]
    fn finish_without_start_is_kept() {
        let mut acc = StreamAccum::default();
        let mut rate = RateAccum::default();
        acc.apply(
            &mut rate,
            &Delta {
                kind: "tool_finished".into(),
                tool: Some("read".into()),
                ok: Some(true),
                ..Delta::default()
            },
            1,
        );
        assert_eq!(acc.tool_events.len(), 1);
        assert_eq!(acc.segments.len(), 1);
        assert_eq!(acc.tool_events[0].status, ToolStatus::Done);
    }

    #[test]
    fn caps_segments_and_tools() {
        let mut acc = StreamAccum::default();
        let mut rate = RateAccum::default();
        for i in 0..320 {
            acc.apply(
                &mut rate,
                &Delta {
                    kind: "tool_started".into(),
                    tool: Some(format!("tool-{i}")),
                    ..Delta::default()
                },
                i,
            );
        }
        assert_eq!(acc.tool_events.len(), 50);
        assert_eq!(acc.segments.len(), 300);
    }

    #[test]
    fn legacy_child_refs_parse_both_shapes() {
        let refs = legacy_child_refs(
            "- del-1 · Confucius · build the page (chat: c1)\n[SPAWNED] child=c2; name=Plato",
        );
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].chat_id, "c1");
        assert_eq!(refs[0].job_id.as_deref(), Some("del-1"));
        assert_eq!(refs[1].chat_id, "c2");
    }

    #[test]
    fn segments_round_trip() {
        let v = json!([
            {"type": "reasoning", "text": "think", "agent": "A"},
            {"type": "text", "text": "answer"},
            {"type": "tool", "ev": {"id": "1", "name": "grep", "status": "done", "ok": true, "ms": 3, "preview": "ok"}}
        ]);
        let segments = parse_segments(&v);
        assert_eq!(segments.len(), 3);
        let back = Value::Array(segments.iter().map(segment_to_json).collect());
        let again = parse_segments(&back);
        assert_eq!(segments, again);
    }
}
