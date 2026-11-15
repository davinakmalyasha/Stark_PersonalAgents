//! Real chat data for the Rune shell.
//!
//! Ports the boot half of `hooks/useSessions.ts` (open tabs + every chat
//! summary) and the tidy-tree layout of `canvas/Graph.ts` so the map shows
//! the same cards React does, at the same coordinates.
//!
//! Contract:
//! - `GET /api/ui/state` → `{open_tabs:["ws:chat"], active_tab}`
//! - `GET /api/workspaces` → `{workspaces:[{id,name,chatCount,…}]}`
//! - `GET /api/workspaces/{id}/chats` → `{chats:[…]}` (per workspace)
//! - `GET /api/workspaces/{id}/chats/{chat}` → `{messages:[…]}` (open tabs)
//!
//! Everything is read-only. Failures fall back to the seeded scene so the
//! shell always renders.

use std::collections::HashMap;

/// Display title (`cleanChatTitle`): strip stacked ` · Agent` suffixes.
pub fn display_title(title: &str) -> String {
    let mut out = title.trim().to_string();
    if out.is_empty() {
        return "Untitled".to_string();
    }
    loop {
        let lower = out.to_ascii_lowercase();
        let mut stripped = false;
        for suffix in [" · general", " · dev", " · ui", " · devops"] {
            if lower.ends_with(suffix) {
                out.truncate(out.len() - suffix.len());
                out = out.trim().to_string();
                stripped = true;
                break;
            }
        }
        if !stripped {
            break;
        }
    }
    if out.is_empty() {
        title.trim().to_string()
    } else {
        out
    }
}

/// Sidebar stamp (`HouseSidebar.relativeTime`): `now`, `23m`, `5h`, `9d`.
/// No `ago` suffix and no seconds tier — the compact list format.
pub fn relative_time_short(value: &str) -> String {
    if value.trim().is_empty() {
        return String::new();
    }
    let Some(then) = crate::agents::parse_timestamp(value) else {
        return String::new();
    };
    let now = (crate::notes::now_millis() / 1000) as i64;
    let mins = (now - then).max(0) / 60;
    if mins < 1 {
        "now".to_string()
    } else if mins < 60 {
        format!("{mins}m")
    } else {
        let hrs = mins / 60;
        if hrs < 24 {
            format!("{hrs}h")
        } else {
            format!("{}d", hrs / 24)
        }
    }
}

/// One chat page in `GET /api/workspaces/{id}/chats`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChatSummary {
    pub id: String,
    pub workspace_id: String,
    pub workspace_name: String,
    pub workspace_kind: String,
    pub title: String,
    pub updated_at: String,
    pub message_count: i64,
    pub parent_chat_id: Option<String>,
    pub parent_workspace_id: Option<String>,
    pub last_message: String,
    pub last_role: String,
}

impl ChatSummary {
    /// Stable page key (`workspaceId:chatId`, React `chatKey`).
    pub fn key(&self) -> String {
        format!("{}:{}", self.workspace_id, self.id)
    }

    /// Canvas card id (`chat:workspaceId:chatId`, React `itemId`).
    pub fn card_id(&self) -> String {
        format!("chat:{}", self.key())
    }

    /// Actor label shown as the card subtitle (`resolveChatActor`).
    pub fn actor(&self) -> &'static str {
        let title = self.title.to_ascii_lowercase();
        if title.ends_with("· general") {
            return "General";
        }
        for role in ["dev", "ui", "devops"] {
            if title.ends_with(&format!("· {role}")) {
                return "Developer";
            }
        }
        match self.workspace_kind.as_str() {
            "general" => "Orchestrator",
            "project" => "Developer",
            _ => "General",
        }
    }
}

/// One uploaded (or restored) file (`ChatAttachment` in `types/index.ts`).
/// `id` is the server's stored filename; a local draft file has no id until
/// the composer uploads it. `path` is the on-disk path the backend returns,
/// which is what the native image cache decodes.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChatAttachment {
    pub id: String,
    pub name: String,
    /// MIME type (`file.type` in React).
    pub file_type: String,
    pub size: u64,
    pub path: String,
    pub is_image: bool,
    pub url: Option<String>,
}

impl ChatAttachment {
    /// Parse one `metadata.attachments` entry; unknown fields are ignored,
    /// exactly like React reading the typed object.
    pub fn from_json(v: &serde_json::Value) -> Self {
        Self {
            id: str_of(v, "id"),
            name: str_of(v, "name"),
            file_type: str_of(v, "type"),
            size: u64_of(v, "size"),
            path: str_of(v, "path"),
            is_image: v
                .get("isImage")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            url: opt_str_of(v, "url"),
        }
    }

    /// Wire shape sent inside the user message metadata and `/api/run`
    /// (`RunRequest.attachments`). Field order follows the React object.
    pub fn to_json(&self) -> serde_json::Value {
        let mut value = serde_json::json!({
            "id": self.id,
            "name": self.name,
            "type": self.file_type,
            "size": self.size,
            "path": self.path,
            "isImage": self.is_image,
        });
        if let Some(url) = &self.url {
            value["url"] = serde_json::Value::String(url.clone());
        }
        value
    }

    pub fn is_audio(&self) -> bool {
        self.file_type.starts_with("audio/")
    }

    /// `Image` / `Audio` / the MIME subtype / `File`.
    pub fn kind_label(&self) -> String {
        if self.is_image {
            "Image".to_string()
        } else if self.is_audio() {
            "Audio".to_string()
        } else {
            self.file_type
                .split_once('/')
                .map(|(_, sub)| sub.to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "File".to_string())
        }
    }

    /// `formatSize`: 1 KB minimum, `x.x MB` past a megabyte.
    pub fn size_label(&self) -> String {
        if self.size < 1024 * 1024 {
            format!("{} KB", (self.size as f64 / 1024.0).round().max(1.0) as u64)
        } else {
            format!("{:.1} MB", self.size as f64 / (1024.0 * 1024.0))
        }
    }
}

/// Browser `File.type` stand-in for a picked path; the backend stores this
/// MIME with the upload and React's card kind reads from it.
pub fn mime_for_path(path: &std::path::Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "ogg" | "oga" => "audio/ogg",
        "webm" => "audio/webm",
        "flac" => "audio/flac",
        "pdf" => "application/pdf",
        "txt" | "md" | "log" => "text/plain; charset=utf-8",
        "json" => "application/json",
        "csv" => "text/csv; charset=utf-8",
        "mp4" => "video/mp4",
        _ => "application/octet-stream",
    }
}

/// One transcript row from a chat payload.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChatMsgRow {
    pub key: String,
    pub role: String,
    pub body: String,
    /// The server's original `content` (before presentation cleanup).
    pub raw_body: String,
    pub at: String,
    /// `metadata.duration` (`8s`) — trace header label.
    pub duration: String,
    /// `metadata.isTrace` and the run has an activity segment/steps: React
    /// renders the collapsed "Worked for …" divider above the answer.
    pub show_trace: bool,
    /// Timeline ends in a tool/reasoning segment: React keeps the flattened
    /// content out of the answer lane (trace-only row).
    pub hide_body: bool,
    /// `metadata.kind`: `"aborted"` or `"compaction"` renders
    /// `SessionEventDivider` instead of a bubble.
    pub event: String,
    /// Compaction token metadata for `SessionEventDivider`.
    pub event_tokens_before: u64,
    pub event_input: u64,
    pub event_output: u64,
    /// `metadata.usage` is present: React renders the per-message token/time
    /// footer only then.
    pub has_usage: bool,
    /// `metadata.usage.total_input_tokens` / `total_output_tokens`.
    pub tokens_in: u64,
    pub tokens_out: u64,
    /// Live timeline (`metadata.liveTimeline`, or `thinking` fallback).
    pub timeline: Vec<crate::stream_accum::StreamSegment>,
    /// `presentStream(...).activitySegments`.
    pub activity: Vec<crate::stream_accum::StreamSegment>,
    /// `presentStream(...).finalText` (delegation narration stripped).
    pub final_text: String,
    /// Live agent label (`agentName` on legacy payloads).
    pub agent_name: String,
    /// `metadata.stopped`.
    pub stopped: bool,
    /// `metadata.thinking`.
    pub thinking: String,
    /// Legacy `metadata.steps`.
    pub steps: Vec<crate::stream_accum::TraceStep>,
    /// Persisted retry notice (legacy live snapshots).
    pub retry: Option<crate::stream_accum::RetryNotice>,
    /// Persisted `metadata.liveStatus`.
    pub live_status: Option<crate::stream_accum::RunStatusNotice>,
    /// Per-message usage for the footer/persistence.
    pub run_usage: Option<crate::stream_accum::RunUsage>,
    /// `metadata.attachments` (`MessageBubble`'s user file cards).
    pub attachments: Vec<ChatAttachment>,
}

/// One message ready for `POST …/messages` (assistant completion, partial
/// stop, or inline error). Draining happens on the shell poll thread.
#[derive(Debug, Clone)]
pub struct ChatSave {
    pub workspace_id: String,
    pub chat_id: String,
    pub role: String,
    pub content: String,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChatSnapshot {
    pub open_tabs: Vec<String>,
    pub active_tab: String,
    /// Open-tab payloads keyed by `workspace:chat` (insertion order = tabs).
    pub messages: HashMap<String, Vec<ChatMsgRow>>,
    pub chats: Vec<ChatSummary>,
}

/// Async result carried from the fetch thread into the UI poll.
pub enum ChatsMsg {
    Snapshot(Result<ChatSnapshot, String>),
    Messages { key: String, rows: Vec<ChatMsgRow> },
}

#[derive(Debug, Default)]
pub struct ChatsState {
    pub inbox: Option<std::sync::mpsc::Receiver<ChatsMsg>>,
    /// Kept so per-chat history fetches share one stream with the snapshot.
    pub tx: Option<std::sync::mpsc::Sender<ChatsMsg>>,
    pub loading: bool,
    pub loaded: bool,
    pub error: Option<String>,
    /// Latest snapshot (sidebar rows + open tabs).
    pub snapshot: ChatSnapshot,
    /// Boot auto-load is enabled by the real app entry, never by unit tests.
    pub auto: bool,
    pub last_try: f64,
}

/// One shared result stream for snapshots and per-chat history.
fn ensure_channel(chats: &mut ChatsState) -> std::sync::mpsc::Sender<ChatsMsg> {
    if chats.tx.is_none() {
        let (tx, rx) = std::sync::mpsc::channel();
        chats.tx = Some(tx);
        chats.inbox = Some(rx);
    }
    chats.tx.clone().expect("channel just ensured")
}

impl Clone for ChatsState {
    fn clone(&self) -> Self {
        Self {
            inbox: None,
            tx: None,
            loading: false,
            loaded: self.loaded,
            error: self.error.clone(),
            snapshot: self.snapshot.clone(),
            auto: self.auto,
            last_try: self.last_try,
        }
    }
}

fn str_of(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

fn opt_str_of(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn u64_of(v: &serde_json::Value, key: &str) -> u64 {
    v.get(key)
        .and_then(|x| x.as_u64().or_else(|| x.as_f64().map(|n| n.max(0.0) as u64)))
        .unwrap_or(0)
}

/// `workspaceKind`/booleans → the actor family used by `canvasActors.ts`.
fn workspace_kind(w: &serde_json::Value) -> String {
    if let Some(kind) = w.get("workspaceKind").and_then(|x| x.as_str()) {
        if !kind.is_empty() {
            return kind.to_string();
        }
    }
    let flag = |k: &str| w.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
    if flag("isGeneral") {
        "general"
    } else if flag("isHouse") {
        "house"
    } else if flag("isPersonal") {
        "personal"
    } else if flag("isDfd") {
        "dfd"
    } else {
        "project"
    }
    .to_string()
}

fn parse_workspace(v: &serde_json::Value) -> (String, String, String, i64) {
    (
        str_of(v, "id"),
        str_of(v, "name"),
        workspace_kind(v),
        v.get("chatCount").and_then(|x| x.as_i64()).unwrap_or(0),
    )
}

fn parse_chat(
    v: &serde_json::Value,
    workspace_id: &str,
    workspace_name: &str,
    workspace_kind: &str,
) -> ChatSummary {
    ChatSummary {
        id: str_of(v, "id"),
        workspace_id: workspace_id.to_string(),
        workspace_name: workspace_name.to_string(),
        workspace_kind: workspace_kind.to_string(),
        title: str_of(v, "title"),
        updated_at: str_of(v, "updatedAt"),
        message_count: v.get("messageCount").and_then(|x| x.as_i64()).unwrap_or(0),
        parent_chat_id: opt_str_of(v, "parentChatId"),
        parent_workspace_id: opt_str_of(v, "parentWorkspaceId"),
        last_message: str_of(v, "lastMessage"),
        last_role: str_of(v, "lastRole"),
    }
}

pub(crate) fn parse_messages(v: &serde_json::Value) -> Vec<ChatMsgRow> {
    v.get("messages")
        .and_then(|x| x.as_array())
        .map(|rows| {
            rows.iter()
                .enumerate()
                .map(|(i, m)| {
                    let meta = m.get("metadata");
                    let duration = meta
                        .and_then(|x| x.get("duration"))
                        .and_then(|x| x.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let is_trace = meta
                        .and_then(|x| x.get("isTrace"))
                        .and_then(|x| x.as_bool())
                        .unwrap_or(false);
                    let thinking = meta
                        .and_then(|x| x.get("thinking"))
                        .and_then(|x| x.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let mut timeline = meta
                        .and_then(|x| x.get("liveTimeline"))
                        .map(crate::stream_accum::parse_segments)
                        .unwrap_or_default();
                    // Old messages only carry the flattened thinking string.
                    if timeline.is_empty() && !thinking.trim().is_empty() {
                        timeline.push(crate::stream_accum::StreamSegment::Reasoning {
                            text: thinking.clone(),
                            agent: None,
                        });
                    }
                    let steps = meta
                        .and_then(|x| x.get("steps"))
                        .map(crate::stream_accum::parse_steps)
                        .unwrap_or_default();
                    let role = match str_of(m, "role").as_str() {
                        "assistant" => "agent".to_string(),
                        other => other.to_string(),
                    };
                    let raw_body = str_of(m, "content");
                    // Only agent rows own a timeline; user/error/system text
                    // is rendered as written (MessageBubble never calls
                    // `presentStream` for them).
                    let (final_text, activity, show_trace) = if role == "agent" {
                        let presentation = crate::stream_presentation::present_stream(
                            &timeline, &raw_body, false, &steps,
                        );
                        let show_trace = is_trace
                            && (!presentation.activity.is_empty()
                                || steps.iter().any(|step| step.renderable()));
                        (presentation.final_text, presentation.activity, show_trace)
                    } else if role == "user" {
                        (
                            crate::stream_presentation::strip_mailbox_envelope(&raw_body),
                            Vec::new(),
                            false,
                        )
                    } else {
                        (raw_body.clone(), Vec::new(), false)
                    };
                    let hide_body = role == "agent" && final_text.trim().is_empty();
                    let event = meta
                        .and_then(|x| x.get("kind"))
                        .and_then(|x| x.as_str())
                        .filter(|k| *k == "aborted" || *k == "compaction")
                        .unwrap_or_default()
                        .to_string();
                    let has_usage = meta.and_then(|x| x.get("usage")).is_some();
                    let usage = meta.and_then(|x| x.get("usage"));
                    let tokens_in = usage.map(|u| u64_of(u, "total_input_tokens")).unwrap_or(0);
                    let tokens_out = usage.map(|u| u64_of(u, "total_output_tokens")).unwrap_or(0);
                    let retry = meta.and_then(|x| x.get("retry")).and_then(parse_retry);
                    let live_status = meta
                        .and_then(|x| x.get("liveStatus"))
                        .and_then(crate::stream_accum::parse_status);
                    let agent_name = m
                        .get("agentName")
                        .and_then(|x| x.as_str())
                        .or_else(|| {
                            meta.and_then(|x| x.get("agentName"))
                                .and_then(|x| x.as_str())
                        })
                        .unwrap_or_default()
                        .to_string();
                    ChatMsgRow {
                        key: opt_str_of(m, "id").unwrap_or_else(|| format!("msg-{i}")),
                        role,
                        body: final_text.clone(),
                        raw_body,
                        at: str_of(m, "timestamp"),
                        duration,
                        show_trace,
                        hide_body,
                        event,
                        event_tokens_before: meta
                            .and_then(|x| x.get("tokens_before"))
                            .and_then(|x| x.as_u64())
                            .unwrap_or(0),
                        event_input: meta
                            .and_then(|x| x.get("input_tokens"))
                            .and_then(|x| x.as_u64())
                            .unwrap_or(0),
                        event_output: meta
                            .and_then(|x| x.get("output_tokens"))
                            .and_then(|x| x.as_u64())
                            .unwrap_or(0),
                        has_usage,
                        tokens_in,
                        tokens_out,
                        timeline,
                        activity,
                        final_text,
                        agent_name,
                        stopped: meta
                            .and_then(|x| x.get("stopped"))
                            .and_then(|x| x.as_bool())
                            .unwrap_or(false),
                        thinking,
                        steps,
                        retry,
                        live_status,
                        run_usage: usage.map(crate::stream_accum::RunUsage::from_json),
                        attachments: meta
                            .and_then(|x| x.get("attachments"))
                            .and_then(|x| x.as_array())
                            .map(|files| files.iter().map(ChatAttachment::from_json).collect())
                            .unwrap_or_default(),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_retry(v: &serde_json::Value) -> Option<crate::stream_accum::RetryNotice> {
    let attempt = v.get("attempt").and_then(|x| x.as_u64()).unwrap_or(0);
    let max = v.get("max").and_then(|x| x.as_u64()).unwrap_or(0);
    if attempt == 0 && max == 0 {
        return None;
    }
    Some(crate::stream_accum::RetryNotice {
        attempt,
        max,
        at: v.get("at").and_then(|x| x.as_u64()).unwrap_or(0),
    })
}

/// Shared assembly: UI state + workspace list + per-workspace chat pages +
/// open-tab message payloads.
fn build_snapshot(
    ui: &serde_json::Value,
    workspaces: &serde_json::Value,
    pages: &[(String, serde_json::Value)],
    messages: &[(String, serde_json::Value)],
) -> ChatSnapshot {
    let open_tabs: Vec<String> = ui
        .get("open_tabs")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let active_tab = str_of(ui, "active_tab");
    let mut meta: HashMap<String, (String, String, String)> = HashMap::new();
    if let Some(list) = workspaces.get("workspaces").and_then(|x| x.as_array()) {
        for w in list {
            let (id, name, kind, _) = parse_workspace(w);
            meta.insert(id, (name, kind, String::new()));
        }
    }
    let mut chats = Vec::new();
    for (workspace_id, page) in pages {
        let (name, kind) = meta
            .get(workspace_id)
            .map(|(n, k, _)| (n.clone(), k.clone()))
            .unwrap_or_default();
        if let Some(list) = page.get("chats").and_then(|x| x.as_array()) {
            for c in list {
                chats.push(parse_chat(c, workspace_id, &name, &kind));
            }
        }
    }
    // React's sidebar orders by recency across workspaces.
    chats.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    let mut message_map = HashMap::new();
    for (key, payload) in messages {
        message_map.insert(key.clone(), parse_messages(payload));
    }
    ChatSnapshot {
        open_tabs,
        active_tab,
        messages: message_map,
        chats,
    }
}

// ── Native fetch ──────────────────────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
fn fetch_snapshot(host: &str, port: u16) -> Result<ChatSnapshot, String> {
    let empty = serde_json::json!({});
    let ui = crate::transport::get_json(host, port, "/api/ui/state").unwrap_or(empty.clone());
    let workspaces = crate::transport::get_json(host, port, "/api/workspaces")?;
    let mut entries = Vec::new();
    if let Some(list) = workspaces.get("workspaces").and_then(|x| x.as_array()) {
        for w in list {
            let (id, _, _, chat_count) = parse_workspace(w);
            // Missing project workspaces return no chats; React still polls
            // them, but a cold boot with 100 stale entries is all round-trip.
            let exists = w.get("exists").and_then(|x| x.as_bool()).unwrap_or(true);
            if id.is_empty() || chat_count <= 0 || !exists {
                continue;
            }
            entries.push(id);
        }
    }
    let mut pages = Vec::new();
    for id in &entries {
        if let Ok(page) =
            crate::transport::get_json(host, port, &format!("/api/workspaces/{id}/chats"))
        {
            pages.push((id.clone(), page));
        }
    }
    let tabs: Vec<String> = ui
        .get("open_tabs")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let mut messages = Vec::new();
    for key in &tabs {
        let Some((ws, chat)) = key.split_once(':') else {
            continue;
        };
        if let Ok(page) =
            crate::transport::get_json(host, port, &format!("/api/workspaces/{ws}/chats/{chat}"))
        {
            messages.push((key.clone(), page));
        }
    }
    Ok(build_snapshot(&ui, &workspaces, &pages, &messages))
}

/// Kick off one full snapshot fetch (no-op while a fetch is in flight).
pub fn refresh(chats: &mut ChatsState) {
    if chats.loading {
        return;
    }
    chats.loading = true;
    chats.error = None;
    let tx = ensure_channel(chats);
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let result = fetch_snapshot_wasm().await;
        let _ = tx.send(ChatsMsg::Snapshot(result));
    });
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let host = crate::rune_transcript::DEFAULT_API_HOST;
        let port = crate::rune_transcript::api_port();
        let _ = tx.send(ChatsMsg::Snapshot(fetch_snapshot(host, port)));
    });
}

/// Fetch one chat's history (sidebar/tab open on a chat that was not an open
/// tab at boot).
pub fn refresh_messages(chats: &mut ChatsState, workspace_id: &str, chat_id: &str) {
    let key = format!("{workspace_id}:{chat_id}");
    let tx = ensure_channel(chats);
    let ws = workspace_id.to_string();
    let chat = chat_id.to_string();
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let path = format!("/api/workspaces/{ws}/chats/{chat}");
        let Ok(value) = crate::transport::fetch_json(&path).await else {
            return;
        };
        let rows = parse_messages(&value);
        let _ = tx.send(ChatsMsg::Messages { key, rows });
    });
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let host = crate::rune_transcript::DEFAULT_API_HOST;
        let port = crate::rune_transcript::api_port();
        let rows =
            crate::transport::get_json(host, port, &format!("/api/workspaces/{ws}/chats/{chat}"))
                .map(|v| parse_messages(&v))
                .unwrap_or_default();
        let _ = tx.send(ChatsMsg::Messages { key, rows });
    });
}

/// Write a batch of finished messages, then fetch the chat once. React does
/// the same (`finishHouseMessage` posts, then `fetchChat`), so the transcript
/// settles on the server's canonical ids/timestamps.
pub fn persist_messages(chats: &mut ChatsState, saves: Vec<ChatSave>) {
    if saves.is_empty() {
        return;
    }
    let key = format!("{}:{}", saves[0].workspace_id, saves[0].chat_id);
    let ws = saves[0].workspace_id.clone();
    let chat = saves[0].chat_id.clone();
    let tx = ensure_channel(chats);
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        for save in &saves {
            let path = format!(
                "/api/workspaces/{}/chats/{}/messages",
                save.workspace_id, save.chat_id
            );
            if crate::transport::fetch_send("POST", &path, Some(&message_body(save)))
                .await
                .is_err()
            {
                return;
            }
        }
        let path = format!("/api/workspaces/{ws}/chats/{chat}");
        let Ok(value) = crate::transport::fetch_json(&path).await else {
            return;
        };
        let rows = parse_messages(&value);
        let _ = tx.send(ChatsMsg::Messages { key, rows });
    });
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let host = crate::rune_transcript::DEFAULT_API_HOST;
        let port = crate::rune_transcript::api_port();
        for save in &saves {
            let path = format!(
                "/api/workspaces/{}/chats/{}/messages",
                save.workspace_id, save.chat_id
            );
            if crate::transport::send_json(host, port, "POST", &path, &message_body(save)).is_err()
            {
                // Keep the finalized row visible; a transient write failure
                // must not blank the transcript.
                return;
            }
        }
        let path = format!("/api/workspaces/{ws}/chats/{chat}");
        let Ok(value) = crate::transport::get_json(host, port, &path) else {
            return;
        };
        let rows = parse_messages(&value);
        let _ = tx.send(ChatsMsg::Messages { key, rows });
    });
}

fn message_body(save: &ChatSave) -> serde_json::Value {
    let mut body = serde_json::json!({"role": save.role, "content": save.content});
    if !save.metadata.is_null() {
        body["metadata"] = save.metadata.clone();
    }
    body
}

/// Drain a transcript's queued writes into one POST+GET batch.
pub fn drain_pending_saves(
    chats: &mut ChatsState,
    thread: &mut crate::rune_transcript::RuneTranscriptState,
) -> bool {
    if thread.pending_saves.is_empty() {
        return false;
    }
    let saves = std::mem::take(&mut thread.pending_saves);
    persist_messages(chats, saves);
    true
}

/// Refresh a chat after an aborted turn's terminal frame settles.
pub fn drain_refresh(
    chats: &mut ChatsState,
    thread: &mut crate::rune_transcript::RuneTranscriptState,
) -> bool {
    let Some((workspace_id, chat_id)) = thread.refresh_chat.take() else {
        return false;
    };
    if workspace_id.trim().is_empty() || chat_id.trim().is_empty() {
        return false;
    }
    refresh_messages(chats, &workspace_id, &chat_id);
    true
}

// ── Browser fetch ─────────────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
async fn fetch_snapshot_wasm() -> Result<ChatSnapshot, String> {
    let empty = serde_json::json!({});
    let ui = crate::transport::fetch_json("/api/ui/state")
        .await
        .unwrap_or(empty.clone());
    let workspaces = crate::transport::fetch_json("/api/workspaces").await?;
    let mut entries = Vec::new();
    if let Some(list) = workspaces.get("workspaces").and_then(|x| x.as_array()) {
        for w in list {
            let (id, _, _, chat_count) = parse_workspace(w);
            // Missing project workspaces return no chats; React still polls
            // them, but a cold boot with 100 stale entries is all round-trip.
            let exists = w.get("exists").and_then(|x| x.as_bool()).unwrap_or(true);
            if id.is_empty() || chat_count <= 0 || !exists {
                continue;
            }
            entries.push(id);
        }
    }
    let mut pages = Vec::new();
    for id in &entries {
        if let Ok(page) = crate::transport::fetch_json(&format!("/api/workspaces/{id}/chats")).await
        {
            pages.push((id.clone(), page));
        }
    }
    let tabs: Vec<String> = ui
        .get("open_tabs")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let mut messages = Vec::new();
    for key in &tabs {
        let Some((ws, chat)) = key.split_once(':') else {
            continue;
        };
        if let Ok(page) =
            crate::transport::fetch_json(&format!("/api/workspaces/{ws}/chats/{chat}")).await
        {
            messages.push((key.clone(), page));
        }
    }
    Ok(build_snapshot(&ui, &workspaces, &pages, &messages))
}

// ── Canvas layout (`canvasGraph.ts`) ──────────────────────────────────────

const ROOT_WIDTH: f32 = 468.0;
const CHILD_WIDTH: f32 = 384.0;
const ROOT_GAP: f32 = 120.0;
const CHILD_GAP: f32 = 64.0;
const LEVEL_GAP: f32 = 470.0;
const MAX_DEPTH: usize = 3;
const MAX_NODES: usize = 60;
const MAX_CHILDREN: usize = 12;

#[derive(Debug, Clone)]
struct Node {
    key: String,
    chat: ChatSummary,
    depth: usize,
    parent: Option<String>,
    children: Vec<String>,
    hidden_children: usize,
    is_tab: bool,
}

/// One laid-out card: id + position + tree metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphCard {
    pub id: String,
    pub workspace_id: String,
    pub chat_id: String,
    pub title: String,
    pub subtitle: String,
    pub x: f32,
    pub y: f32,
    pub compact: bool,
    pub is_tab: bool,
    pub depth: usize,
    /// Parent card id when this card nests under another (`CanvasItem.parentItemId`).
    pub parent_item_id: Option<String>,
}

/// Tidy-tree placement, line for line from `buildCanvasGraph`.
pub fn layout(snapshot: &ChatSnapshot) -> (Vec<GraphCard>, Vec<(String, String)>) {
    fn node_width(depth: usize) -> f32 {
        if depth == 0 {
            ROOT_WIDTH
        } else {
            CHILD_WIDTH
        }
    }

    fn width_of(
        key: &str,
        memo: &mut HashMap<String, f32>,
        nodes: &[Node],
        index: &HashMap<String, usize>,
    ) -> f32 {
        if let Some(w) = memo.get(key) {
            return *w;
        }
        let Some(&ix) = index.get(key) else {
            return 0.0;
        };
        let node = &nodes[ix];
        let own = node_width(node.depth);
        let width = if node.children.is_empty() {
            own
        } else {
            let mut sum = 0.0;
            for (i, child) in node.children.iter().enumerate() {
                sum += width_of(child, memo, nodes, index) + if i > 0 { CHILD_GAP } else { 0.0 };
            }
            own.max(sum)
        };
        memo.insert(key.to_string(), width);
        width
    }

    fn place(
        key: &str,
        left: f32,
        nodes: &[Node],
        index: &HashMap<String, usize>,
        memo: &mut HashMap<String, f32>,
        positions: &mut HashMap<String, (f32, f32)>,
    ) -> f32 {
        let Some(&ix) = index.get(key) else {
            return left;
        };
        let node = &nodes[ix];
        let width = width_of(key, memo, nodes, index);
        let own = node_width(node.depth);
        let y = node.depth as f32 * LEVEL_GAP;
        if node.children.is_empty() {
            positions.insert(key.to_string(), (left + (width - own) / 2.0, y));
            return left + width;
        }
        let mut child_widths = 0.0;
        for (i, child) in node.children.iter().enumerate() {
            child_widths +=
                width_of(child, memo, nodes, index) + if i > 0 { CHILD_GAP } else { 0.0 };
        }
        let mut cursor = left + (width - child_widths).max(0.0) / 2.0;
        for child in &node.children {
            cursor = place(child, cursor, nodes, index, memo, positions) + CHILD_GAP;
        }
        positions.insert(key.to_string(), (left + (width - own) / 2.0, y));
        left + width
    }

    #[allow(clippy::too_many_arguments)]
    fn visit(
        parent_key: &str,
        depth: usize,
        nodes: &mut Vec<Node>,
        index: &mut HashMap<String, usize>,
        by_key: &HashMap<String, ChatSummary>,
        visited: &mut std::collections::HashSet<String>,
        total: &mut usize,
    ) {
        if depth >= MAX_DEPTH {
            return;
        }
        let Some(&parent_ix) = index.get(parent_key) else {
            return;
        };
        let parent_ws = nodes[parent_ix].chat.workspace_id.clone();
        let parent_chat_id = nodes[parent_ix].chat.id.clone();
        let mut candidates: Vec<(String, ChatSummary)> = by_key
            .iter()
            .filter(|(key, chat)| {
                key.as_str() != parent_key
                    && chat.parent_chat_id.as_deref() == Some(parent_chat_id.as_str())
                    && chat
                        .parent_workspace_id
                        .as_deref()
                        .unwrap_or(chat.workspace_id.as_str())
                        == parent_ws
            })
            .map(|(k, c)| (k.clone(), c.clone()))
            .collect();
        candidates.sort_by(|a, b| a.1.updated_at.cmp(&b.1.updated_at));
        let child_depth = depth + 1;
        let candidate_count = candidates.len();
        let mut added = 0usize;
        for (key, chat) in candidates {
            if *total >= MAX_NODES || added >= MAX_CHILDREN {
                break;
            }
            if visited.contains(&key) {
                continue;
            }
            if let Some(&ix) = index.get(&key) {
                nodes[ix].depth = child_depth;
                nodes[ix].parent = Some(parent_key.to_string());
            } else {
                let ix = nodes.len();
                index.insert(key.clone(), ix);
                nodes.push(Node {
                    key: key.clone(),
                    chat,
                    depth: child_depth,
                    parent: Some(parent_key.to_string()),
                    children: Vec::new(),
                    hidden_children: 0,
                    is_tab: false,
                });
                *total += 1;
            }
            visited.insert(key.clone());
            nodes[parent_ix].children.push(key.clone());
            added += 1;
            visit(&key, child_depth, nodes, index, by_key, visited, total);
        }
        nodes[parent_ix].hidden_children = candidate_count.saturating_sub(added);
    }

    let mut by_key: HashMap<String, ChatSummary> = HashMap::new();
    for chat in &snapshot.chats {
        by_key.insert(chat.key(), chat.clone());
    }
    let mut nodes: Vec<Node> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut tab_keys: Vec<String> = Vec::new();
    for key in &snapshot.open_tabs {
        if index.contains_key(key) {
            continue;
        }
        let Some((ws, chat_id)) = key.split_once(':') else {
            continue;
        };
        let chat = by_key.get(key).cloned().unwrap_or_else(|| ChatSummary {
            id: chat_id.to_string(),
            workspace_id: ws.to_string(),
            title: "Untitled".to_string(),
            ..ChatSummary::default()
        });
        index.insert(key.clone(), nodes.len());
        nodes.push(Node {
            key: key.clone(),
            chat,
            depth: 0,
            parent: None,
            children: Vec::new(),
            hidden_children: 0,
            is_tab: true,
        });
        tab_keys.push(key.clone());
    }
    // A tab whose parent tab is also open nests under it.
    let mut root_keys: Vec<String> = Vec::new();
    for key in &tab_keys {
        let node_index = index[key];
        let parent = nodes[node_index]
            .chat
            .parent_chat_id
            .as_ref()
            .map(|pid| {
                let pws = nodes[node_index]
                    .chat
                    .parent_workspace_id
                    .clone()
                    .unwrap_or_else(|| nodes[node_index].chat.workspace_id.clone());
                format!("{pws}:{pid}")
            })
            .filter(|pk| pk != key && index.contains_key(pk));
        if let Some(parent) = parent {
            nodes[node_index].parent = Some(parent);
        } else {
            root_keys.push(key.clone());
        }
    }
    let mut visited: std::collections::HashSet<String> = root_keys.iter().cloned().collect();
    let mut total = nodes.len();
    for root in &root_keys {
        visit(
            root,
            0,
            &mut nodes,
            &mut index,
            &by_key,
            &mut visited,
            &mut total,
        );
    }
    // Unvisited tabs are still roots (React promotes them before layout).
    for key in &tab_keys {
        if let Some(&ix) = index.get(key) {
            if !visited.contains(key) {
                nodes[ix].parent = None;
                nodes[ix].depth = 0;
                root_keys.push(key.clone());
                visited.insert(key.clone());
            }
        }
    }
    let mut memo = HashMap::new();
    let mut positions: HashMap<String, (f32, f32)> = HashMap::new();
    let mut cursor = 0.0;
    for root in &root_keys {
        cursor = place(root, cursor, &nodes, &index, &mut memo, &mut positions) + ROOT_GAP;
    }
    let forest_width = (cursor - ROOT_GAP).max(0.0);
    let shift_x = 72.0 - forest_width / 2.0;
    let shift_y = 64.0;
    let mut cards = Vec::new();
    let mut edges = Vec::new();
    for node in &nodes {
        let Some(&(x, y)) = positions.get(&node.key) else {
            continue;
        };
        let id = format!("chat:{}", node.key);
        if let Some(parent) = &node.parent {
            edges.push((format!("chat:{parent}"), id.clone()));
        }
        let parent_item_id = node.parent.as_ref().map(|parent| format!("chat:{parent}"));
        cards.push(GraphCard {
            id,
            workspace_id: node.chat.workspace_id.clone(),
            chat_id: node.chat.id.clone(),
            title: display_title(&node.chat.title),
            subtitle: node.chat.actor().to_string(),
            x: (x + shift_x).round(),
            y: (y + shift_y).round(),
            compact: node.depth > 0,
            is_tab: node.is_tab,
            depth: node.depth,
            parent_item_id,
        });
    }
    (cards, edges)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(ws: &str, id: &str, title: &str, updated: &str) -> ChatSummary {
        ChatSummary {
            id: id.to_string(),
            workspace_id: ws.to_string(),
            title: title.to_string(),
            updated_at: updated.to_string(),
            ..ChatSummary::default()
        }
    }

    #[test]
    fn short_time_tiers_match_sidebar() {
        assert_eq!(relative_time_short(""), "");
        assert_eq!(relative_time_short("garbage"), "");
        // Just-minted stamps land in the `now` tier.
        assert_eq!(
            relative_time_short(&crate::rune_transcript::now_iso()),
            "now"
        );
    }

    #[test]
    fn layout_matches_react_root_position_for_one_tab() {
        let chat = chat("_general", "a", "hello", "2026-01-01T00:00:00Z");
        let snapshot = ChatSnapshot {
            open_tabs: vec!["_general:a".to_string()],
            active_tab: "_general:a".to_string(),
            messages: HashMap::new(),
            chats: vec![chat],
        };
        let (cards, edges) = layout(&snapshot);
        assert_eq!(cards.len(), 1);
        assert!(edges.is_empty());
        // One 468-wide root: forest 468 → shiftX = 72 - 234 = -162 → x = -162.
        assert_eq!(cards[0].id, "chat:_general:a");
        assert_eq!(cards[0].x, -162.0);
        assert_eq!(cards[0].y, 64.0);
        assert!(!cards[0].compact);
    }

    #[test]
    fn persisted_timeline_drives_trace_and_answer() {
        let payload = serde_json::json!({"messages": [{
            "id": "m1",
            "role": "assistant",
            "content": "The answer.",
            "timestamp": "2026-11-12T12:00:00Z",
            "metadata": {
                "isTrace": true,
                "duration": "8s",
                "liveTimeline": [
                    {"type": "reasoning", "text": "thinking"},
                    {"type": "tool", "ev": {"id": "1", "name": "grep", "status": "done", "ok": false, "ms": 9, "preview": "Error: no target"}},
                    {"type": "text", "text": "The answer."}
                ],
                "usage": {"total_input_tokens": 12, "total_output_tokens": 3}
            }
        }]});
        let rows = parse_messages(&payload);
        let row = &rows[0];
        assert_eq!(row.role, "agent");
        assert_eq!(row.body, "The answer.");
        assert_eq!(row.final_text, "The answer.");
        assert_eq!(row.activity.len(), 2);
        assert!(row.show_trace);
        assert!(!row.hide_body);
        assert_eq!(row.tokens_in, 12);
        assert_eq!(row.tokens_out, 3);
        assert_eq!(row.timeline.len(), 3);
    }

    #[test]
    fn thinking_fallback_and_stopped_parse() {
        let payload = serde_json::json!({"messages": [{
            "id": "m2",
            "role": "agent",
            "content": "partial",
            "metadata": {
                "isTrace": true,
                "thinking": "why",
                "stopped": true,
                "kind": "aborted"
            }
        }]});
        let rows = parse_messages(&payload);
        let row = &rows[0];
        assert_eq!(row.thinking, "why");
        assert!(row.stopped);
        assert_eq!(row.event, "aborted");
        assert_eq!(row.timeline.len(), 1);
        assert_eq!(row.activity.len(), 1);
        // React's timeline is authoritative: a thinking-only timeline has no
        // trailing text run, so the flattened content is not shown.
        assert_eq!(row.final_text, "");
        assert!(row.hide_body);
        assert!(row.show_trace);
    }

    #[test]
    fn user_content_is_not_run_through_the_stream_cleanups() {
        let payload = serde_json::json!({"messages": [{
            "id": "u1",
            "role": "user",
            "content": "Asked.✓ Done — keep my prompt verbatim"
        }]});
        let rows = parse_messages(&payload);
        assert_eq!(rows[0].body, "Asked.✓ Done — keep my prompt verbatim");
        assert!(!rows[0].hide_body);
        assert!(!rows[0].show_trace);
        assert!(rows[0].activity.is_empty());
    }

    #[test]
    fn internal_transport_rows_are_filtered_by_the_home_view() {
        let payload = serde_json::json!({"messages": [{
            "id": "m3",
            "role": "user",
            "content": "[DELEGATION RESULT] hidden"
        }]});
        let rows = parse_messages(&payload);
        assert_eq!(rows[0].body, "[DELEGATION RESULT] hidden");
    }

    #[test]
    fn layout_nests_child_under_parent() {
        let mut parent = chat("_general", "p", "parent", "2026-01-01T00:00:00Z");
        parent.message_count = 2;
        let mut child = chat("_general", "c", "parent · General", "2026-01-02T00:00:00Z");
        child.parent_chat_id = Some("p".to_string());
        let snapshot = ChatSnapshot {
            open_tabs: vec!["_general:p".to_string()],
            active_tab: "_general:p".to_string(),
            messages: HashMap::new(),
            chats: vec![parent, child],
        };
        let (cards, edges) = layout(&snapshot);
        assert_eq!(cards.len(), 2);
        assert_eq!(edges.len(), 1);
        assert_eq!(
            edges[0],
            ("chat:_general:p".into(), "chat:_general:c".into())
        );
        let child = cards.iter().find(|c| c.chat_id == "c").unwrap();
        assert!(child.compact);
        assert_eq!(child.depth, 1);
    }
}
