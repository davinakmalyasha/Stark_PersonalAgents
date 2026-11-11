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

/// One transcript row from a chat payload.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChatMsgRow {
    pub key: String,
    pub role: String,
    pub body: String,
    pub at: String,
    /// `metadata.duration` (`8s`) — trace header label.
    pub duration: String,
    /// `metadata.isTrace` and the run has an activity segment/steps: React
    /// renders the collapsed "Worked for …" divider above the answer.
    pub show_trace: bool,
    /// Timeline ends in a tool/reasoning segment: React keeps the flattened
    /// content out of the answer lane (trace-only row).
    pub hide_body: bool,
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

fn parse_messages(v: &serde_json::Value) -> Vec<ChatMsgRow> {
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
                    let timeline = meta
                        .and_then(|x| x.get("liveTimeline"))
                        .and_then(|x| x.as_array());
                    let timeline_len = timeline.map(|a| a.len()).unwrap_or(0);
                    let last_is_text = timeline
                        .and_then(|a| a.last())
                        .and_then(|x| x.get("type"))
                        .and_then(|x| x.as_str())
                        .map(|t| t == "text")
                        .unwrap_or(false);
                    let activity = if timeline_len == 0 {
                        0
                    } else if last_is_text {
                        timeline_len.saturating_sub(1)
                    } else {
                        timeline_len
                    };
                    let step_activity = meta
                        .and_then(|x| x.get("steps"))
                        .and_then(|x| x.as_array())
                        .map(|steps| {
                            steps.iter().any(|s| {
                                s.get("childId").and_then(|x| x.as_str()).is_some()
                                    || matches!(
                                        s.get("type").and_then(|x| x.as_str()),
                                        Some("tool") | Some("command")
                                    )
                                    || s.get("content")
                                        .and_then(|x| x.as_str())
                                        .is_some_and(|c| !c.trim().is_empty())
                            })
                        })
                        .unwrap_or(false);
                    ChatMsgRow {
                        key: opt_str_of(m, "id").unwrap_or_else(|| format!("msg-{i}")),
                        role: match str_of(m, "role").as_str() {
                            "assistant" => "agent".to_string(),
                            other => other.to_string(),
                        },
                        body: str_of(m, "content"),
                        at: str_of(m, "timestamp"),
                        duration,
                        show_trace: is_trace && (activity > 0 || step_activity),
                        hide_body: timeline_len > 0 && !last_is_text,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
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
        let port = crate::rune_transcript::DEFAULT_API_PORT;
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
        let rows = match crate::transport::fetch_json(&path).await {
            Ok(v) => parse_messages(&v),
            Err(_) => Vec::new(),
        };
        let _ = tx.send(ChatsMsg::Messages { key, rows });
    });
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let host = crate::rune_transcript::DEFAULT_API_HOST;
        let port = crate::rune_transcript::DEFAULT_API_PORT;
        let rows =
            crate::transport::get_json(host, port, &format!("/api/workspaces/{ws}/chats/{chat}"))
                .map(|v| parse_messages(&v))
                .unwrap_or_default();
        let _ = tx.send(ChatsMsg::Messages { key, rows });
    });
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
