//! Cloud runs — port of `CloudRuns.tsx` + `services/api/cloud.ts` (plus the
//! workspace/chat pickers from `workspace.ts` / `chat.ts`).
//!
//! Contract:
//! - `GET /api/runs/active` → `{running, workspace_path, chat_id,
//!   started_at}` (the live dot; pre-existing Rune behavior, kept).
//! - `GET /api/cloud/status` → `{origin, cap, configured, cloud_host,
//!   runs:[{running, chat_id, workspace_path, started_at}]}`.
//! - `GET /api/cloud/handoffs` → `{handoffs:[...]}`.
//! - `GET /api/workspaces` → `{workspaces:[{id, name}]}`; then per workspace
//!   `GET /api/workspaces/{id}/chats` → `{chats:[{id, title}]}` (failures
//!   resolve `[]`, like `fetchChats`' catch).
//! - `POST /api/cloud/send {workspace_id, chat_id?, task?}` → `{handoff}`.
//!
//! The send button is an explicit press (user go-ahead at press time, like
//! composer send). Everything else is read-only; failures keep last state
//! (`catch {}` in `refresh`). Auto-refresh every 10s (`CloudRuns` interval).

use crate::keys::EnterPress;
use rune::{col, row, text, text_input, El, TextEdit};

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

/// One active run, mirroring `ActiveRun` (`GET /api/runs/active`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CloudActive {
    pub running: bool,
    pub workspace_path: String,
    pub chat_id: String,
    pub started_at: String,
}

impl CloudActive {
    pub fn from_json(v: &serde_json::Value) -> Self {
        let str_of = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        Self {
            running: v.get("running").and_then(|x| x.as_bool()).unwrap_or(false),
            workspace_path: str_of("workspace_path"),
            chat_id: str_of("chat_id"),
            started_at: str_of("started_at"),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn fetch(host: &str, port: u16) -> Result<Self, String> {
        let v = crate::transport::get_json(host, port, "/api/runs/active")?;
        Ok(Self::from_json(&v))
    }
}

/// One run inside `GET /api/cloud/status` (`CloudRun`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CloudRun {
    pub running: bool,
    pub chat_id: String,
    pub workspace_path: String,
    pub started_at: String,
}

impl CloudRun {
    pub fn from_json(v: &serde_json::Value) -> Self {
        let str_of = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        Self {
            running: v.get("running").and_then(|x| x.as_bool()).unwrap_or(false),
            chat_id: str_of("chat_id"),
            workspace_path: str_of("workspace_path"),
            started_at: str_of("started_at"),
        }
    }
}

/// `GET /api/cloud/status` (`CloudStatus`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CloudStatus {
    pub origin: String,
    pub cap: i64,
    pub configured: bool,
    pub cloud_host: String,
    pub runs: Vec<CloudRun>,
}

impl CloudStatus {
    pub fn from_json(v: &serde_json::Value) -> Self {
        Self {
            origin: v
                .get("origin")
                .and_then(|x| x.as_str())
                .unwrap_or("local")
                .to_string(),
            cap: v.get("cap").and_then(|x| x.as_i64()).unwrap_or(10),
            configured: v
                .get("configured")
                .and_then(|x| x.as_bool())
                .unwrap_or(false),
            cloud_host: v
                .get("cloud_host")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            runs: v
                .get("runs")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().map(CloudRun::from_json).collect())
                .unwrap_or_default(),
        }
    }
}

/// One handoff (`CloudHandoff`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CloudHandoff {
    pub id: String,
    pub chat_id: String,
    pub workspace_id: String,
    pub branch: String,
    pub task: String,
    pub status: String,
    pub origin: String,
    pub package: String,
    pub error: String,
    pub updated: String,
}

impl CloudHandoff {
    pub fn from_json(v: &serde_json::Value) -> Self {
        let str_of = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        Self {
            id: str_of("id"),
            chat_id: str_of("chat_id"),
            workspace_id: str_of("workspace_id"),
            branch: str_of("branch"),
            task: str_of("task"),
            status: str_of("status"),
            origin: str_of("origin"),
            package: str_of("package"),
            error: str_of("error"),
            updated: str_of("updated_at"),
        }
    }

    /// Chip colors (`statusColor`): sent/received emerald, staged sky,
    /// failed red, everything else zinc.
    pub fn chip(&self) -> (&'static str, &'static str) {
        match self.status.as_str() {
            "received" | "sent" => ("#064e3b55", "#6ee7b7"),
            "staged" => ("#a1a1aa55", "#e4e4e7"),
            "failed" => ("#7f1d1d55", "#fca5a5"),
            _ => ("#27272a", "#d4d4d8"),
        }
    }
}

/// Workspace picker entry (`WorkspaceInfo` id+name subset).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WorkspaceEntry {
    pub id: String,
    pub name: String,
}

/// Chat picker entry (`ChatSummary` id+title subset).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChatEntry {
    pub id: String,
    pub title: String,
}

/// One board snapshot: status + handoffs + picker data. Chats belong to
/// `chats_for` (the selected workspace at fetch time); a late selection
/// change discards them instead of showing another workspace's chats.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CloudBoard {
    pub status: Option<CloudStatus>,
    pub handoffs: Vec<CloudHandoff>,
    pub workspaces: Vec<WorkspaceEntry>,
    pub chats: Vec<ChatEntry>,
    pub chats_for: String,
}

/// Worker messages share one inbox.
#[derive(Debug)]
pub enum CloudMsg {
    Active(Result<CloudActive, String>),
    Board(Result<CloudBoard, String>),
    Sent(Result<CloudHandoff, String>),
}

pub struct CloudState {
    pub host: String,
    pub port: u16,
    pub active: Option<CloudActive>,
    pub status: Option<CloudStatus>,
    pub handoffs: Vec<CloudHandoff>,
    pub workspaces: Vec<WorkspaceEntry>,
    pub chats: Vec<ChatEntry>,
    pub chats_for: String,
    pub workspace_id: String,
    pub chat_id: String,
    pub task_edit: TextEdit,
    pub notice: Option<String>,
    pub loading: bool,
    pub board_loading: bool,
    pub sending: bool,
    pub error: Option<String>,
    pub last_ok: f64,
    pub auto: bool,
    pub inbox: Option<std::sync::mpsc::Receiver<CloudMsg>>,
    agent_tx: Option<std::sync::mpsc::Sender<CloudMsg>>,
}

impl std::fmt::Debug for CloudState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudState")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("active", &self.active)
            .field("handoffs", &self.handoffs.len())
            .field("workspaces", &self.workspaces.len())
            .field("loading", &self.loading)
            .field("error", &self.error)
            .finish()
    }
}

impl Clone for CloudState {
    fn clone(&self) -> Self {
        Self {
            host: self.host.clone(),
            port: self.port,
            active: self.active.clone(),
            status: self.status.clone(),
            handoffs: self.handoffs.clone(),
            workspaces: self.workspaces.clone(),
            chats: self.chats.clone(),
            chats_for: self.chats_for.clone(),
            workspace_id: self.workspace_id.clone(),
            chat_id: self.chat_id.clone(),
            task_edit: self.task_edit.clone(),
            notice: self.notice.clone(),
            loading: self.loading,
            board_loading: self.board_loading,
            sending: self.sending,
            error: self.error.clone(),
            last_ok: self.last_ok,
            auto: self.auto,
            inbox: None,
            agent_tx: None,
        }
    }
}

impl Default for CloudState {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8124,
            active: None,
            status: None,
            handoffs: Vec::new(),
            workspaces: Vec::new(),
            chats: Vec::new(),
            chats_for: String::new(),
            workspace_id: String::new(),
            chat_id: String::new(),
            task_edit: TextEdit::new(""),
            notice: None,
            loading: false,
            board_loading: false,
            sending: false,
            error: None,
            last_ok: 0.0,
            auto: true,
            inbox: None,
            agent_tx: None,
        }
    }
}

/// Host access for the board.
pub trait CloudHost {
    fn cloud_mut(&mut self) -> &mut CloudState;
}

fn sender(s: &mut CloudState) -> std::sync::mpsc::Sender<CloudMsg> {
    if s.inbox.is_none() {
        let (tx, rx) = std::sync::mpsc::channel();
        s.inbox = Some(rx);
        s.agent_tx = Some(tx);
    }
    s.agent_tx.clone().expect("inbox installs a sender")
}

/// Refresh the live `/api/runs/active` dot. No-op while one is in flight;
/// failures surface as an error row, never a stuck spinner.
pub fn refresh_cloud(s: &mut CloudState) {
    if s.loading {
        return;
    }
    s.loading = true;
    s.error = None;
    let tx = sender(s);
    let (host, port) = (s.host.clone(), s.port);
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let result = match crate::transport::fetch_json("/api/runs/active").await {
            Ok(v) => Ok(CloudActive::from_json(&v)),
            Err(e) => Err(e),
        };
        let _ = tx.send(CloudMsg::Active(result));
        let _ = (host, port);
    });
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let _ = tx.send(CloudMsg::Active(CloudActive::fetch(&host, port)));
    });
}

/// Refresh status + handoffs + workspaces + the selected workspace's chats.
/// One worker, sequential reads; any failure keeps last state (the reference UI's
/// `catch {}`), except the status read which must succeed to count.
pub fn refresh_board(s: &mut CloudState) {
    if s.board_loading {
        return;
    }
    s.board_loading = true;
    let tx = sender(s);
    let (host, port) = (s.host.clone(), s.port);
    let ws = s.workspace_id.clone();
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let _ = tx.send(CloudMsg::Board(fetch_board_wasm(&ws).await));
        let _ = (host, port);
    });
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let _ = tx.send(CloudMsg::Board(fetch_board_native(&host, port, &ws)));
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn fetch_board_native(host: &str, port: u16, workspace_id: &str) -> Result<CloudBoard, String> {
    let status = Some(CloudStatus::from_json(&crate::transport::get_json(
        host,
        port,
        "/api/cloud/status",
    )?));
    // Handoffs + pickers are best-effort (`catch → []` on the reference UI side).
    let handoffs = crate::transport::get_json(host, port, "/api/cloud/handoffs")
        .ok()
        .and_then(|v| {
            v.get("handoffs")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().map(CloudHandoff::from_json).collect())
        })
        .unwrap_or_default();
    let workspaces: Vec<WorkspaceEntry> = crate::transport::get_json(host, port, "/api/workspaces")
        .ok()
        .and_then(|v| {
            v.get("workspaces").and_then(|x| x.as_array()).map(|a| {
                a.iter()
                    .map(|w| WorkspaceEntry {
                        id: w
                            .get("id")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                        name: w
                            .get("name")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                    })
                    .collect()
            })
        })
        .unwrap_or_default();
    let pick = if workspace_id.trim().is_empty() {
        workspaces.first().map(|w| w.id.clone()).unwrap_or_default()
    } else {
        workspace_id.to_string()
    };
    let chats: Vec<ChatEntry> = if pick.is_empty() {
        Vec::new()
    } else {
        crate::transport::get_json(host, port, &format!("/api/workspaces/{pick}/chats"))
            .ok()
            .and_then(|v| {
                v.get("chats").and_then(|x| x.as_array()).map(|a| {
                    a.iter()
                        .map(|c| ChatEntry {
                            id: c
                                .get("id")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string(),
                            title: c
                                .get("title")
                                .and_then(|x| x.as_str())
                                .filter(|t| !t.is_empty())
                                .unwrap_or("Untitled")
                                .to_string(),
                        })
                        .collect()
                })
            })
            .unwrap_or_default()
    };
    Ok(CloudBoard {
        status,
        handoffs,
        workspaces,
        chats,
        chats_for: pick,
    })
}

#[cfg(target_arch = "wasm32")]
async fn fetch_board_wasm(workspace_id: &str) -> Result<CloudBoard, String> {
    let status = match crate::transport::fetch_json("/api/cloud/status").await {
        Ok(v) => Some(CloudStatus::from_json(&v)),
        Err(e) => return Err(e),
    };
    let handoffs = crate::transport::fetch_json("/api/cloud/handoffs")
        .await
        .ok()
        .and_then(|v| {
            v.get("handoffs")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().map(CloudHandoff::from_json).collect())
        })
        .unwrap_or_default();
    let workspaces: Vec<WorkspaceEntry> = crate::transport::fetch_json("/api/workspaces")
        .await
        .ok()
        .and_then(|v| {
            v.get("workspaces").and_then(|x| x.as_array()).map(|a| {
                a.iter()
                    .map(|w| WorkspaceEntry {
                        id: w
                            .get("id")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                        name: w
                            .get("name")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                    })
                    .collect()
            })
        })
        .unwrap_or_default();
    let pick = if workspace_id.trim().is_empty() {
        workspaces.first().map(|w| w.id.clone()).unwrap_or_default()
    } else {
        workspace_id.to_string()
    };
    let chats: Vec<ChatEntry> = if pick.is_empty() {
        Vec::new()
    } else {
        crate::transport::fetch_json(&format!("/api/workspaces/{pick}/chats"))
            .await
            .ok()
            .and_then(|v| {
                v.get("chats").and_then(|x| x.as_array()).map(|a| {
                    a.iter()
                        .map(|c| ChatEntry {
                            id: c
                                .get("id")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string(),
                            title: c
                                .get("title")
                                .and_then(|x| x.as_str())
                                .filter(|t| !t.is_empty())
                                .unwrap_or("Untitled")
                                .to_string(),
                        })
                        .collect()
                })
            })
            .unwrap_or_default()
    };
    Ok(CloudBoard {
        status,
        handoffs,
        workspaces,
        chats,
        chats_for: pick,
    })
}

/// Send the selected workspace/chat + task to the cloud (explicit press).
/// The notice mirrors `handleSend`: `sent` names the branch, `staged` names
/// The carry-over package.
pub fn send_cloud(s: &mut CloudState) {
    if s.sending || s.workspace_id.trim().is_empty() {
        return;
    }
    s.sending = true;
    s.notice = None;
    s.error = None;
    let tx = sender(s);
    let (host, port) = (s.host.clone(), s.port);
    let (ws, chat, task) = (
        s.workspace_id.clone(),
        s.chat_id.clone(),
        s.task_edit.text.trim().to_string(),
    );
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let mut body = serde_json::json!({ "workspace_id": ws });
        if !chat.trim().is_empty() {
            body["chat_id"] = serde_json::Value::String(chat);
        }
        if !task.is_empty() {
            body["task"] = serde_json::Value::String(task);
        }
        let result =
            match crate::transport::fetch_send("POST", "/api/cloud/send", Some(&body)).await {
                Err(e) => Err(e),
                Ok(v) => v
                    .get("handoff")
                    .map(CloudHandoff::from_json)
                    .ok_or_else(|| "Cloud send returned no handoff.".to_string()),
            };
        let _ = tx.send(CloudMsg::Sent(result));
        let _ = (host, port);
    });
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let mut body = serde_json::json!({ "workspace_id": ws });
        if !chat.trim().is_empty() {
            body["chat_id"] = serde_json::Value::String(chat);
        }
        if !task.is_empty() {
            body["task"] = serde_json::Value::String(task);
        }
        let result =
            match crate::transport::send_json(&host, port, "POST", "/api/cloud/send", &body) {
                Err(e) => Err(e),
                Ok(v) => v
                    .get("handoff")
                    .map(CloudHandoff::from_json)
                    .ok_or_else(|| "Cloud send returned no handoff.".to_string()),
            };
        let _ = tx.send(CloudMsg::Sent(result));
    });
}

/// Drain the inbox; auto-refresh both feeds at most every 10s.
pub fn poll_cloud_state(s: &mut CloudState, clock: f64) -> bool {
    let mut changed = false;
    while let Some(msg) = s.inbox.as_ref().and_then(|rx| rx.try_recv().ok()) {
        match msg {
            CloudMsg::Active(Ok(active)) => {
                s.active = Some(active);
                s.error = None;
                s.loading = false;
                changed = true;
            }
            CloudMsg::Active(Err(message)) => {
                s.error = Some(message);
                s.loading = false;
                changed = true;
            }
            CloudMsg::Board(Ok(board)) => {
                s.status = board.status;
                s.handoffs = board.handoffs;
                s.workspaces = board.workspaces;
                // Adopt a default workspace like `setWorkspaceId(wsList[0])`,
                // and only take chats that belong to the current selection.
                if s.workspace_id.trim().is_empty() {
                    if let Some(first) = s.workspaces.first() {
                        s.workspace_id = first.id.clone();
                    }
                }
                if board.chats_for == s.workspace_id {
                    s.chats = board.chats;
                    s.chats_for = board.chats_for;
                }
                s.board_loading = false;
                s.last_ok = clock;
                changed = true;
            }
            CloudMsg::Board(Err(message)) => {
                s.error = Some(message);
                s.board_loading = false;
                changed = true;
            }
            CloudMsg::Sent(result) => {
                s.sending = false;
                match result {
                    Ok(h) => {
                        s.notice = Some(if h.status == "sent" {
                            format!(
                                "Sent to VPS on {}. Continue it there — same chat id.",
                                h.branch
                            )
                        } else {
                            format!(
                                "Staged as {}. VPS not configured: import {} on the VPS via POST /api/cloud/receive.",
                                h.branch, h.package
                            )
                        });
                        s.task_edit.text.clear();
                    }
                    Err(message) => {
                        s.notice = Some(message);
                    }
                }
                refresh_board(s);
                changed = true;
            }
        }
    }
    let dead = s.inbox.as_ref().is_some_and(|rx| {
        matches!(
            rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        )
    });
    if dead {
        s.inbox = None;
        s.agent_tx = None;
    }
    if s.auto && !s.loading && !s.board_loading && clock - s.last_ok > 10.0 {
        s.loading = true;
        s.board_loading = true;
        refresh_cloud(s);
        // `refresh_cloud` returns early on `loading`; call the board worker
        // directly through the shared sender path.
        refresh_board(s);
        changed = true;
    }
    changed
}

/// `Nov 9, 14:30` (`formatTime`): absolute, locale-free. Unparseable input
/// passes through (the reference UI returns the raw value).
pub fn format_time(value: &str) -> String {
    if value.trim().is_empty() {
        return "—".to_string();
    }
    let digits: String = value.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() < 12 {
        return value.to_string();
    }
    let part = |from: usize, len: usize| digits[from..from + len].parse::<i64>().unwrap_or(0);
    let (y, mo, d, h, mi) = (part(0, 4), part(4, 2), part(6, 2), part(8, 2), part(10, 2));
    if !(1..=12).contains(&mo) || d == 0 || y == 0 {
        return value.to_string();
    }
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!("{} {d}, {h:02}:{mi:02}", MONTHS[(mo - 1) as usize])
}

fn stat_cell(label: &str, value: String, key: String) -> El {
    row()
        .key(key)
        .w_full()
        .items_center()
        .gap(8.0)
        .child(
            text(label.to_string())
                .grow(1.0)
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .color(ink("#71717a")),
        )
        .child(
            text(value)
                .font(rune::tok::TextStyle::new(11.0, 400.0).mono())
                .color(ink("#e4e4e7")),
        )
}

/// Full CloudRuns board: live dot, status grid, send form, handoffs.
/// Stateful pickers use tap rows (Rune has no `<select>`): workspaces, then
/// The selected workspace's chats with a "Whole workspace" head row.
pub fn cloud_view<S: CloudHost + 'static>(s: &CloudState) -> El {
    let running = s.active.as_ref().is_some_and(|a| a.running);
    let status_label = match &s.active {
        Some(a) if a.running => "Running",
        Some(_) => "Idle",
        None => "Unknown",
    };
    let detail = match &s.active {
        Some(a) if !a.workspace_path.is_empty() => a.workspace_path.clone(),
        Some(a) if !a.chat_id.is_empty() => format!("chat {}", a.chat_id),
        _ => "no run reported yet".to_string(),
    };
    let mut body = col()
        .key("cloud-board")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(ink("#0a0a0a"))
        .pt(4.0)
        .gap(10.0)
        .child(
            row()
                .key("cloud-head")
                .w_full()
                .items_center()
                .child(
                    text("Cloud runs".to_string())
                        .key("cloud-title")
                        .grow(1.0)
                        .font(rune::tok::TextStyle::new(15.0, 650.0))
                        .color(ink("#f2f2f5")),
                )
                .child(
                    text(
                        if s.loading || s.board_loading {
                            "…"
                        } else {
                            "Refresh"
                        }
                        .to_string(),
                    )
                    .key("cloud-refresh")
                    .role("button")
                    .focusable(true)
                    .label("Refresh cloud status")
                    .font(rune::tok::TextStyle::new(11.0, 600.0))
                    .px(12.0)
                    .py(8.0)
                    .radius(8.0)
                    .fill(ink("#ffffff0d"))
                    .color(ink("#d4d4d8"))
                    .on_press_enter(|s: &mut S| {
                        let c = s.cloud_mut();
                        refresh_cloud(c);
                        refresh_board(c);
                    }),
                ),
        )
        .child(
            row()
                .key("cloud-status")
                .w_full()
                .items_center()
                .gap(10.0)
                .radius(12.0)
                .fill(ink("#141417"))
                .border(1.0, ink("#ffffff14"))
                .px(14.0)
                .py(12.0)
                .child(
                    rune::el()
                        .key("cloud-dot")
                        .size(8.0, 8.0)
                        .radius(4.0)
                        .fill(ink(if running { "#7ee0a8" } else { "#52525b" })),
                )
                .child(
                    col()
                        .grow(1.0)
                        .min_h(0.0)
                        .child(
                            text(status_label.to_string())
                                .key("cloud-state")
                                .font(rune::tok::TextStyle::new(13.0, 600.0))
                                .color(ink("#e4e4e7")),
                        )
                        .child(
                            text(detail)
                                .key("cloud-detail")
                                .font(rune::tok::TextStyle::new(11.0, 400.0))
                                .color(ink("#8a8a93")),
                        ),
                ),
        );
    // Status grid ("This machine").
    {
        let (origin, cap, vps) = match &s.status {
            Some(st) => (
                st.origin.clone(),
                format!("{} / {}", st.runs.len(), st.cap),
                if st.configured {
                    if st.cloud_host.is_empty() {
                        "set".to_string()
                    } else {
                        st.cloud_host.clone()
                    }
                } else {
                    "not set".to_string()
                },
            ),
            None => ("connecting…".to_string(), "—".to_string(), "—".to_string()),
        };
        body = body.child(
            col()
                .key("cloud-grid")
                .w_full()
                .gap(8.0)
                .radius(12.0)
                .fill(ink("#141417"))
                .border(1.0, ink("#ffffff14"))
                .px(14.0)
                .py(12.0)
                .child(
                    text("This machine".to_string())
                        .key("cloud-grid-title")
                        .font(rune::tok::TextStyle::new(12.0, 700.0))
                        .color(ink("#e4e4e7")),
                )
                .child(stat_cell("Origin", origin, "cloud-origin".to_string()))
                .child(stat_cell("Running", cap, "cloud-running".to_string()))
                .child(stat_cell("VPS target", vps, "cloud-vps".to_string()))
                .child(stat_cell(
                    "Handoffs",
                    s.handoffs.len().to_string(),
                    "cloud-handoffs-count".to_string(),
                )),
        );
        if !s.status.as_ref().is_some_and(|st| st.configured) {
            body = body.child(
                text("Set STARK_CLOUD_URL + STARK_CLOUD_TOKEN (a VPS pairing-device token) to push directly; otherwise handoffs stage a package you carry over.".to_string())
                    .key("cloud-unconfigured")
                    .font(rune::tok::TextStyle::new(11.0, 400.0))
                    .wrap()
                    .color(ink("#71717a")),
            );
        }
    }
    // Send form.
    {
        let mut send = col()
            .key("cloud-send")
            .w_full()
            .gap(8.0)
            .radius(12.0)
            .fill(ink("#141417"))
            .border(1.0, ink("#ffffff14"))
            .px(14.0)
            .py(12.0)
            .child(
                text("Send to Cloud".to_string())
                    .key("cloud-send-title")
                    .font(rune::tok::TextStyle::new(12.0, 700.0))
                    .color(ink("#e4e4e7")),
            );
        if s.workspaces.is_empty() {
            send = send.child(
                text("No workspaces yet.".to_string())
                    .key("cloud-send-nows")
                    .font(rune::tok::TextStyle::new(11.0, 400.0))
                    .color(ink("#52525b")),
            );
        }
        for ws in &s.workspaces {
            let id = ws.id.clone();
            let selected = ws.id == s.workspace_id;
            send = send.child(
                row()
                    .key(format!("cloud-ws-{id}"))
                    .w_full()
                    .items_center()
                    .gap(8.0)
                    .radius(8.0)
                    .px(10.0)
                    .py(6.0)
                    .fill(ink(if selected { "#1b1b20" } else { "#00000000" }))
                    .border(1.0, ink(if selected { "#ffffff30" } else { "#ffffff14" }))
                    .role("button")
                    .focusable(true)
                    .label(format!("Send from {}", ws.name))
                    .on_press_enter(move |s: &mut S| {
                        let c = s.cloud_mut();
                        c.workspace_id = id.clone();
                        c.chat_id.clear();
                        refresh_board(c);
                    })
                    .child(
                        text(ws.name.clone())
                            .font(rune::tok::TextStyle::new(12.0, 600.0))
                            .color(ink(if selected { "#e4e4e7" } else { "#a1a1aa" })),
                    ),
            );
        }
        // Chat picker: whole-workspace head row + chats of the selection.
        let whole = s.chat_id.trim().is_empty();
        send = send.child(
            row()
                .key("cloud-chat-whole")
                .w_full()
                .items_center()
                .gap(8.0)
                .radius(8.0)
                .px(10.0)
                .py(6.0)
                .fill(ink(if whole { "#1b1b20" } else { "#00000000" }))
                .border(1.0, ink(if whole { "#ffffff30" } else { "#ffffff14" }))
                .role("button")
                .focusable(true)
                .label("Send whole workspace")
                .on_press_enter(move |s: &mut S| {
                    s.cloud_mut().chat_id.clear();
                })
                .child(
                    text("Whole workspace".to_string())
                        .font(rune::tok::TextStyle::new(11.0, 400.0))
                        .color(ink(if whole { "#e4e4e7" } else { "#71717a" })),
                ),
        );
        for chat in &s.chats {
            let id = chat.id.clone();
            let selected = chat.id == s.chat_id;
            send = send.child(
                row()
                    .key(format!("cloud-chat-{id}"))
                    .w_full()
                    .items_center()
                    .gap(8.0)
                    .radius(8.0)
                    .px(10.0)
                    .py(6.0)
                    .fill(ink(if selected { "#1b1b20" } else { "#00000000" }))
                    .border(1.0, ink(if selected { "#ffffff30" } else { "#ffffff14" }))
                    .role("button")
                    .focusable(true)
                    .label(format!("Send {}", chat.title))
                    .on_press_enter(move |s: &mut S| {
                        s.cloud_mut().chat_id = id.clone();
                    })
                    .child(
                        text(chat.title.clone())
                            .font(rune::tok::TextStyle::new(11.0, 400.0))
                            .color(ink(if selected { "#e4e4e7" } else { "#a1a1aa" })),
                    ),
            );
        }
        send = send.child(
            text_input(&s.task_edit, rune::tok::TextStyle::new(12.0, 400.0))
                .key("cloud-task")
                .w_full()
                .placeholder(
                    "e.g. finish the failing tests on cloud/fix-login-…",
                    ink("#52525b"),
                )
                .caret(ink("#f2f2f4"), crate::motion::TEXT_SELECTION)
                .on_edit(move |s: &mut S| &mut s.cloud_mut().task_edit),
        );
        send = send.child(
            row()
                .key("cloud-sendrow")
                .w_full()
                .items_center()
                .gap(8.0)
                .child(
                    text(
                        if s.sending {
                            "Sending…"
                        } else {
                            "Send to Cloud"
                        }
                        .to_string(),
                    )
                    .key("cloud-send-button")
                    .role("button")
                    .focusable(true)
                    .label("Send to cloud")
                    .font(rune::tok::TextStyle::new(12.0, 600.0))
                    .px(12.0)
                    .py(8.0)
                    .radius(8.0)
                    .fill(ink(if s.workspace_id.trim().is_empty() {
                        "#a1a1aa33"
                    } else {
                        "#a1a1aa55"
                    }))
                    .border(1.0, ink("#a1a1aa55"))
                    .color(ink(if s.workspace_id.trim().is_empty() {
                        "#a1a1aa88"
                    } else {
                        "#e4e4e7"
                    }))
                    .on_press_enter(move |s: &mut S| {
                        send_cloud(s.cloud_mut());
                    }),
                ),
        );
        if let Some(notice) = &s.notice {
            send = send.child(
                text(notice.clone())
                    .key("cloud-notice")
                    .font(rune::tok::TextStyle::new(11.0, 400.0))
                    .wrap()
                    .color(ink("#a1a1aa")),
            );
        }
        body = body.child(send);
    }
    // Handoffs.
    {
        let mut section = col()
            .key("cloud-handoffs")
            .w_full()
            .gap(8.0)
            .radius(12.0)
            .fill(ink("#141417"))
            .border(1.0, ink("#ffffff14"))
            .px(14.0)
            .py(12.0)
            .child(
                text("Handoffs".to_string())
                    .key("cloud-handoffs-title")
                    .font(rune::tok::TextStyle::new(12.0, 700.0))
                    .color(ink("#e4e4e7")),
            );
        if s.handoffs.is_empty() {
            section = section.child(
                text(
                    "No handoffs yet. Send a chat above and it lands here with its branch."
                        .to_string(),
                )
                .key("cloud-handoffs-empty")
                .font(rune::tok::TextStyle::new(12.0, 400.0))
                .color(ink("#52525b")),
            );
        }
        for h in &s.handoffs {
            let (chip_bg, chip_ink) = h.chip();
            let mut card = col()
                .key(format!("cloud-handoff-{id}", id = h.id))
                .w_full()
                .gap(4.0)
                .radius(8.0)
                .fill(rune::Color {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.02,
                })
                .border(1.0, ink("#ffffff0f"))
                .px(12.0)
                .py(8.0)
                .child(
                    row()
                        .key(format!("cloud-handoff-{id}-head", id = h.id))
                        .w_full()
                        .items_center()
                        .gap(8.0)
                        .child(
                            text(h.status.clone())
                                .key(format!("cloud-handoff-{id}-status", id = h.id))
                                .font(rune::tok::TextStyle::new(10.0, 600.0))
                                .px(6.0)
                                .py(2.0)
                                .radius(4.0)
                                .fill(ink(chip_bg))
                                .border(1.0, ink(chip_ink))
                                .color(ink(chip_ink)),
                        )
                        .child(
                            text(h.branch.clone())
                                .key(format!("cloud-handoff-{id}-branch", id = h.id))
                                .grow(1.0)
                                .font(rune::tok::TextStyle::new(11.0, 400.0).mono())
                                .color(ink("#e4e4e7")),
                        )
                        .child(
                            text(format_time(&h.updated))
                                .key(format!("cloud-handoff-{id}-time", id = h.id))
                                .font(rune::tok::TextStyle::new(10.0, 400.0))
                                .color(ink("#52525b")),
                        ),
                )
                .child(
                    text(format!(
                        "chat: {}  ws: {}  from: {}",
                        h.chat_id.chars().take(12).collect::<String>(),
                        h.workspace_id.chars().take(12).collect::<String>(),
                        h.origin
                    ))
                    .key(format!("cloud-handoff-{id}-meta", id = h.id))
                    .font(rune::tok::TextStyle::new(11.0, 400.0).mono())
                    .color(ink("#71717a")),
                );
            if !h.task.is_empty() {
                card = card.child(
                    text(h.task.clone())
                        .key(format!("cloud-handoff-{id}-task", id = h.id))
                        .font(rune::tok::TextStyle::new(11.0, 400.0))
                        .wrap()
                        .color(ink("#a1a1aa")),
                );
            }
            if !h.error.is_empty() {
                card = card.child(
                    text(h.error.clone())
                        .key(format!("cloud-handoff-{id}-error", id = h.id))
                        .role("alert")
                        .live_region(rune::LiveRegion::Assertive)
                        .font(rune::tok::TextStyle::new(11.0, 400.0))
                        .wrap()
                        .color(ink("#fda4af")),
                );
            }
            section = section.child(card);
        }
        body = body.child(section);
    }
    if let Some(err) = &s.error {
        body = body.child(
            text(err.clone())
                .key("cloud-error")
                .role("alert")
                .live_region(rune::LiveRegion::Assertive)
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .color(ink("#fda4af")),
        );
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handoff_card_mounts() {
        struct T {
            cloud: CloudState,
        }
        impl CloudHost for T {
            fn cloud_mut(&mut self) -> &mut CloudState {
                &mut self.cloud
            }
        }
        let env = rune::Env::new(true, 1.0);
        let cloud = CloudState {
            handoffs: vec![CloudHandoff {
                id: "h1".to_string(),
                status: "staged".to_string(),
                branch: "cloud/x".to_string(),
                ..CloudHandoff::default()
            }],
            ..CloudState::default()
        };
        let mut rt = rune::Runtime::new(
            T { cloud },
            |s: &T, _: &mut rune::Cx<T>| cloud_view::<T>(&s.cloud),
            (1280.0, 900.0),
            env,
        );
        rt.frame(0.0);
        for key in [
            "cloud-handoffs",
            "cloud-handoff-h1",
            "cloud-handoff-h1-status",
        ] {
            assert!(rt.find(key).is_some(), "{key} mounted");
        }
    }

    #[test]
    fn status_parses_with_defaults() {
        let v = serde_json::json!({
            "origin": "desk", "cap": 4, "configured": true, "cloud_host": "vps",
            "runs": [{"running": true, "chat_id": "c1"}],
        });
        let s = CloudStatus::from_json(&v);
        assert_eq!(s.origin, "desk");
        assert_eq!(s.runs.len(), 1);
        let empty = CloudStatus::from_json(&serde_json::json!({}));
        assert_eq!(empty.origin, "local");
        assert_eq!(empty.cap, 10);
        assert!(!empty.configured);
    }

    #[test]
    fn handoff_chip_colors_match_status_color() {
        let sent = CloudHandoff {
            status: "sent".to_string(),
            ..CloudHandoff::default()
        };
        assert_eq!(sent.chip().1, "#6ee7b7");
        let staged = CloudHandoff {
            status: "staged".to_string(),
            ..CloudHandoff::default()
        };
        assert_eq!(staged.chip().1, "#e4e4e7");
        let failed = CloudHandoff {
            status: "failed".to_string(),
            ..CloudHandoff::default()
        };
        assert_eq!(failed.chip().1, "#fca5a5");
        let other = CloudHandoff {
            status: "weird".to_string(),
            ..CloudHandoff::default()
        };
        assert_eq!(other.chip().1, "#d4d4d8");
    }

    #[test]
    fn format_time_shapes() {
        assert_eq!(format_time("2026-11-09T14:30:00Z"), "Nov 9, 14:30");
        assert_eq!(format_time(""), "—");
        assert_eq!(format_time("garbage"), "garbage");
    }
}
