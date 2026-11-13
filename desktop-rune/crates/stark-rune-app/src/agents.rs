//! Agent tree — port of `AgentTreePanel.tsx` + `services/api/agents.ts`.
//!
//! Contract:
//! - `GET /api/agents` → `{agents:[AgentNode]}` (empty array on error, like
//!   `fetchAgents`'s catch).
//! - `GET /api/agents/{id}` → `AgentDetail {agent, messages, history,
//!   children}` (null on error).
//! - `GET /api/agents/{id}/diff` → `{agent?, diff?{stat, files[]}, error?}`.
//! - `POST /api/agents/{id}/message {message}` → `{ok, steered?, error?}`.
//! - `POST /api/agents/{id}/cancel` (explicit press, like composer Stop).
//! - `POST /api/agents/{id}/merge|discard` → `{ok, error?}` — both move real
//!   files, so both sit behind React's own two-tap `confirmAction` gate: the
//!   first tap arms, the second fires. That confirm tap IS the user go-ahead.
//!
//! Mount: a section in the Home sidebar (delegations surface per-workspace,
//! like the React coding sidebar) with the detail drawer as a modal sheet.

use std::collections::{HashMap, HashSet};

use rune::{col, row, text, text_input, El, TextEdit};

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

/// Status dot colors (`STATUS_COLOR`).
fn status_ink(status: &str) -> &'static str {
    match status {
        "running" => "#34d399",
        "queued" => "#fbbf24",
        "idle" => "#71717a",
        "interrupted" => "#fb923c",
        "completed" => "#38bdf8",
        "failed" => "#f87171",
        "cancelled" => "#52525b",
        _ => "#71717a",
    }
}

/// Access badge (`accessBadge`): read-only / isolated / direct.
pub fn access_badge(access: &str, worktree_path: &str) -> Option<&'static str> {
    match access.to_ascii_lowercase().as_str() {
        "read" => Some("read-only"),
        "worktree" => Some("isolated"),
        "write" if !worktree_path.is_empty() => Some("isolated"),
        "write" => Some("direct"),
        _ if !worktree_path.is_empty() => Some("isolated"),
        _ => None,
    }
}

/// One agent node (`AgentNode`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AgentNode {
    pub id: String,
    pub kind: String,
    pub path: String,
    pub parent_id: Option<String>,
    pub scope: String,
    pub role: String,
    pub nickname: String,
    pub status: String,
    pub model: String,
    pub depth: i64,
    pub result: String,
    pub access: String,
    pub worktree_path: String,
    pub pending: i64,
    pub active: String,
}

impl AgentNode {
    pub fn from_json(v: &serde_json::Value) -> Self {
        let str_of = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let worktree = v.get("worktree").cloned().unwrap_or_default();
        Self {
            id: str_of("id"),
            kind: str_of("kind"),
            path: str_of("path"),
            parent_id: v
                .get("parentId")
                .and_then(|x| x.as_str())
                .map(str::to_string),
            scope: str_of("scope"),
            role: str_of("role"),
            nickname: str_of("nickname"),
            status: str_of("status"),
            model: str_of("model"),
            depth: v.get("depth").and_then(|x| x.as_i64()).unwrap_or(0),
            result: str_of("result"),
            access: str_of("access"),
            worktree_path: worktree
                .get("path")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            pending: v
                .get("pendingMessages")
                .and_then(|x| x.as_i64())
                .unwrap_or(0),
            active: v
                .get("lastActive")
                .and_then(|x| x.as_str())
                .map(str::to_string)
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    v.get("updatedAt")
                        .and_then(|x| x.as_str())
                        .map(str::to_string)
                })
                .unwrap_or_default(),
        }
    }

    pub fn display(&self) -> String {
        if !self.nickname.is_empty() {
            self.nickname.clone()
        } else if !self.role.is_empty() {
            self.role.clone()
        } else {
            self.scope.clone()
        }
    }
}

/// One mailbox message (`AgentMessage`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AgentMessage {
    pub id: String,
    pub kind: String,
    pub body: String,
    pub delivery: String,
    pub created: String,
    pub consumed: bool,
}

impl AgentMessage {
    pub fn from_json(v: &serde_json::Value) -> Self {
        let str_of = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        Self {
            id: str_of("id"),
            kind: str_of("kind"),
            body: str_of("body"),
            delivery: str_of("delivery"),
            created: str_of("createdAt"),
            consumed: v
                .get("consumedAt")
                .and_then(|x| x.as_str())
                .is_some_and(|s| !s.is_empty()),
        }
    }
}

/// Agent detail (`AgentDetail` + worktree/diff halves).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AgentDetail {
    pub agent: AgentNode,
    pub pending: Vec<AgentMessage>,
    pub history: Vec<AgentMessage>,
    pub worktree_branch: String,
    pub worktree_state: String,
    pub worktree_base: String,
    pub worktree_error: String,
    pub diff_stat: String,
    pub diff_files: Vec<(String, String)>,
}

impl AgentDetail {
    pub fn from_json(v: &serde_json::Value) -> Option<Self> {
        let agent = v.get("agent")?;
        let wt = agent.get("worktree").cloned().unwrap_or_default();
        let diff = wt.get("diff").cloned().unwrap_or_default();
        let str_of = |v: &serde_json::Value, k: &str| {
            v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
        };
        Some(Self {
            agent: AgentNode::from_json(agent),
            pending: v
                .get("messages")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().map(AgentMessage::from_json).collect())
                .unwrap_or_default(),
            history: v
                .get("history")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().map(AgentMessage::from_json).collect())
                .unwrap_or_default(),
            worktree_branch: str_of(&wt, "branch"),
            worktree_state: str_of(&wt, "state"),
            worktree_base: str_of(&wt, "base"),
            worktree_error: str_of(&wt, "error"),
            diff_stat: str_of(&diff, "stat"),
            diff_files: diff
                .get("files")
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .map(|f| (str_of(f, "status"), str_of(f, "path")))
                        .collect()
                })
                .unwrap_or_default(),
        })
    }
}

/// Detail drawer tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentTab {
    #[default]
    Overview,
    Mailbox,
    Worktree,
}

/// A confirmed write result: refresh list + detail, surface the notice.
#[derive(Debug, Clone)]
pub enum AgentOp {
    RefreshList,
    RefreshDetail { agent_id: String },
    SendMessage { agent_id: String, message: String },
    Cancel { agent_id: String },
    Merge { agent_id: String },
    Discard { agent_id: String },
    RefreshDiff { agent_id: String },
}

#[derive(Debug, Clone)]
pub struct AgentOpResult {
    pub notice: Option<String>,
    pub error: Option<String>,
}

pub struct AgentsState {
    pub host: String,
    pub port: u16,
    pub agents: Vec<AgentNode>,
    pub open: bool,
    pub expanded: HashSet<String>,
    pub selected: Option<String>,
    pub detail: Option<AgentDetail>,
    pub tab: AgentTab,
    pub message_edit: TextEdit,
    pub busy: bool,
    pub diff_busy: bool,
    pub error: Option<String>,
    pub notice: Option<String>,
    /// Merge/discard two-tap gate (`confirmAction`).
    pub confirm: Option<String>,
    pub last_ok: f64,
    pub auto: bool,
    pub loading: bool,
    pub inbox: Option<std::sync::mpsc::Receiver<AgentThreadMsg>>,
    /// Persistent worker sender: created with the inbox, cloned per worker,
    /// so in-flight sends never strand on a channel swap.
    agent_tx: Option<std::sync::mpsc::Sender<AgentThreadMsg>>,
}

impl std::fmt::Debug for AgentsState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentsState")
            .field("agents", &self.agents.len())
            .field("selected", &self.selected)
            .field("tab", &self.tab)
            .field("busy", &self.busy)
            .finish()
    }
}

impl Clone for AgentsState {
    fn clone(&self) -> Self {
        Self {
            host: self.host.clone(),
            port: self.port,
            agents: self.agents.clone(),
            open: self.open,
            expanded: self.expanded.clone(),
            selected: self.selected.clone(),
            detail: self.detail.clone(),
            tab: self.tab,
            message_edit: self.message_edit.clone(),
            busy: self.busy,
            diff_busy: self.diff_busy,
            error: self.error.clone(),
            notice: self.notice.clone(),
            confirm: self.confirm.clone(),
            last_ok: self.last_ok,
            auto: self.auto,
            loading: self.loading,
            inbox: None,
            agent_tx: None,
        }
    }
}

impl Default for AgentsState {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8124,
            agents: Vec::new(),
            open: true,
            expanded: HashSet::new(),
            selected: None,
            detail: None,
            tab: AgentTab::Overview,
            message_edit: TextEdit::new(""),
            busy: false,
            diff_busy: false,
            error: None,
            notice: None,
            confirm: None,
            last_ok: 0.0,
            auto: true,
            loading: false,
            inbox: None,
            agent_tx: None,
        }
    }
}

/// Worker messages: list snapshots and op outcomes share one inbox. The
/// detail rides boxed (520B vs 48B siblings — `large_enum_variant`).
#[derive(Debug)]
pub enum AgentThreadMsg {
    List(Result<Vec<AgentNode>, String>),
    Detail(Result<Option<Box<AgentDetail>>, String>),
    Op(AgentOpResult),
}

/// Group agent ids by parent (`children` memo).
pub fn children_of(agents: &[AgentNode]) -> HashMap<Option<String>, Vec<usize>> {
    let mut map: HashMap<Option<String>, Vec<usize>> = HashMap::new();
    for (i, a) in agents.iter().enumerate() {
        map.entry(a.parent_id.clone()).or_default().push(i);
    }
    map
}

/// Spawn a worker for `op`. List/detail refreshes run even while another op
/// is in flight; writes serialize on `busy` (React disables while
/// `detailBusy`).
pub fn dispatch_agent(s: &mut AgentsState, op: AgentOp) {
    let is_write = matches!(
        op,
        AgentOp::SendMessage { .. }
            | AgentOp::Cancel { .. }
            | AgentOp::Merge { .. }
            | AgentOp::Discard { .. }
    );
    if is_write && s.busy {
        return;
    }
    if is_write {
        s.busy = true;
        s.error = None;
        s.notice = None;
    }
    if s.inbox.is_none() {
        let (tx, rx) = std::sync::mpsc::channel();
        s.inbox = Some(rx);
        s.agent_tx = Some(tx);
    }
    let tx = s.agent_tx.clone().expect("inbox installs a sender");
    let (host, port) = (s.host.clone(), s.port);
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let _ = tx.send(run_agent_op_wasm(op).await);
        let _ = (host, port);
    });
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let _ = tx.send(run_agent_op_native(&host, port, op));
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn run_agent_op_native(host: &str, port: u16, op: AgentOp) -> AgentThreadMsg {
    match op {
        AgentOp::RefreshList => AgentThreadMsg::List(fetch_agents_native(host, port)),
        AgentOp::RefreshDetail { agent_id } => AgentThreadMsg::Detail(
            fetch_detail_native(host, port, &agent_id).map(|o| o.map(Box::new)),
        ),
        AgentOp::SendMessage { agent_id, message } => {
            let r = crate::transport::send_json(
                host,
                port,
                "POST",
                &format!("/agents/{agent_id}/message"),
                &serde_json::json!({ "message": message }),
            );
            AgentThreadMsg::Op(match r {
                Err(e) => AgentOpResult {
                    notice: None,
                    error: Some(e),
                },
                Ok(v) if v.get("error").and_then(|x| x.as_str()).is_some() => AgentOpResult {
                    notice: None,
                    error: Some(
                        v.get("error")
                            .and_then(|x| x.as_str())
                            .unwrap_or("Send failed")
                            .to_string(),
                    ),
                },
                Ok(v) => AgentOpResult {
                    notice: Some(
                        if v.get("steered").and_then(|x| x.as_bool()).unwrap_or(false) {
                            "Steered the running agent.".to_string()
                        } else {
                            "Queued for the next run.".to_string()
                        },
                    ),
                    error: None,
                },
            })
        }
        AgentOp::Cancel { agent_id } => {
            let r = crate::transport::send_json(
                host,
                port,
                "POST",
                &format!("/agents/{agent_id}/cancel"),
                &serde_json::Value::Null,
            );
            AgentThreadMsg::Op(AgentOpResult {
                notice: None,
                error: r.err(),
            })
        }
        AgentOp::Merge { agent_id } => {
            // Latest diff first, like `runMerge` (best-effort; merge decides).
            let _ = crate::transport::get_json(host, port, &format!("/agents/{agent_id}/diff"));
            let r = crate::transport::send_json(
                host,
                port,
                "POST",
                &format!("/agents/{agent_id}/merge"),
                &serde_json::Value::Null,
            );
            AgentThreadMsg::Op(match r {
                Err(e) => AgentOpResult {
                    notice: None,
                    error: Some(e),
                },
                Ok(v) if v.get("error").and_then(|x| x.as_str()).is_some() => AgentOpResult {
                    notice: None,
                    error: Some(
                        v.get("error")
                            .and_then(|x| x.as_str())
                            .unwrap_or("Merge failed")
                            .to_string(),
                    ),
                },
                Ok(_) => AgentOpResult {
                    notice: Some("Worktree merged.".to_string()),
                    error: None,
                },
            })
        }
        AgentOp::Discard { agent_id } => {
            let r = crate::transport::send_json(
                host,
                port,
                "POST",
                &format!("/agents/{agent_id}/discard"),
                &serde_json::Value::Null,
            );
            AgentThreadMsg::Op(match r {
                Err(e) => AgentOpResult {
                    notice: None,
                    error: Some(e),
                },
                Ok(v) if v.get("error").and_then(|x| x.as_str()).is_some() => AgentOpResult {
                    notice: None,
                    error: Some(
                        v.get("error")
                            .and_then(|x| x.as_str())
                            .unwrap_or("Discard failed")
                            .to_string(),
                    ),
                },
                Ok(_) => AgentOpResult {
                    notice: Some("Worktree discarded.".to_string()),
                    error: None,
                },
            })
        }
        AgentOp::RefreshDiff { agent_id } => AgentThreadMsg::Detail(
            fetch_detail_native(host, port, &agent_id).map(|o| o.map(Box::new)),
        ),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn fetch_agents_native(host: &str, port: u16) -> Result<Vec<AgentNode>, String> {
    match crate::transport::get_json(host, port, "/agents") {
        // `fetchAgents` resolves `[]` on error — an empty tree, never a red
        // board for a transient failure.
        Err(_) => Ok(Vec::new()),
        Ok(v) => Ok(v
            .get("agents")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().map(AgentNode::from_json).collect())
            .unwrap_or_default()),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn fetch_detail_native(
    host: &str,
    port: u16,
    agent_id: &str,
) -> Result<Option<AgentDetail>, String> {
    match crate::transport::get_json(host, port, &format!("/agents/{agent_id}")) {
        Err(e) => Err(e),
        Ok(v) => Ok(AgentDetail::from_json(&v)),
    }
}

#[cfg(target_arch = "wasm32")]
async fn run_agent_op_wasm(op: AgentOp) -> AgentThreadMsg {
    match op {
        AgentOp::RefreshList => {
            AgentThreadMsg::List(match crate::transport::fetch_json("/agents").await {
                Err(_) => Ok(Vec::new()),
                Ok(v) => Ok(v
                    .get("agents")
                    .and_then(|x| x.as_array())
                    .map(|a| a.iter().map(AgentNode::from_json).collect())
                    .unwrap_or_default()),
            })
        }
        AgentOp::RefreshDetail { agent_id } | AgentOp::RefreshDiff { agent_id } => {
            AgentThreadMsg::Detail(
                match crate::transport::fetch_json(&format!("/agents/{agent_id}")).await {
                    Err(e) => Err(e),
                    Ok(v) => Ok(AgentDetail::from_json(&v).map(Box::new)),
                },
            )
        }
        AgentOp::SendMessage { agent_id, message } => {
            let r = crate::transport::fetch_send(
                "POST",
                &format!("/agents/{agent_id}/message"),
                Some(&serde_json::json!({ "message": message })),
            )
            .await;
            AgentThreadMsg::Op(match r {
                Err(e) => AgentOpResult {
                    notice: None,
                    error: Some(e),
                },
                Ok(v) if v.get("error").and_then(|x| x.as_str()).is_some() => AgentOpResult {
                    notice: None,
                    error: Some(
                        v.get("error")
                            .and_then(|x| x.as_str())
                            .unwrap_or("Send failed")
                            .to_string(),
                    ),
                },
                Ok(v) => AgentOpResult {
                    notice: Some(
                        if v.get("steered").and_then(|x| x.as_bool()).unwrap_or(false) {
                            "Steered the running agent.".to_string()
                        } else {
                            "Queued for the next run.".to_string()
                        },
                    ),
                    error: None,
                },
            })
        }
        AgentOp::Cancel { agent_id } => {
            let r =
                crate::transport::fetch_send("POST", &format!("/agents/{agent_id}/cancel"), None)
                    .await;
            AgentThreadMsg::Op(AgentOpResult {
                notice: None,
                error: r.err(),
            })
        }
        AgentOp::Merge { agent_id } => {
            let _ = crate::transport::fetch_json(&format!("/agents/{agent_id}/diff")).await;
            let r =
                crate::transport::fetch_send("POST", &format!("/agents/{agent_id}/merge"), None)
                    .await;
            AgentThreadMsg::Op(match r {
                Err(e) => AgentOpResult {
                    notice: None,
                    error: Some(e),
                },
                Ok(v) if v.get("error").and_then(|x| x.as_str()).is_some() => AgentOpResult {
                    notice: None,
                    error: Some(
                        v.get("error")
                            .and_then(|x| x.as_str())
                            .unwrap_or("Merge failed")
                            .to_string(),
                    ),
                },
                Ok(_) => AgentOpResult {
                    notice: Some("Worktree merged.".to_string()),
                    error: None,
                },
            })
        }
        AgentOp::Discard { agent_id } => {
            let r =
                crate::transport::fetch_send("POST", &format!("/agents/{agent_id}/discard"), None)
                    .await;
            AgentThreadMsg::Op(match r {
                Err(e) => AgentOpResult {
                    notice: None,
                    error: Some(e),
                },
                Ok(v) if v.get("error").and_then(|x| x.as_str()).is_some() => AgentOpResult {
                    notice: None,
                    error: Some(
                        v.get("error")
                            .and_then(|x| x.as_str())
                            .unwrap_or("Discard failed")
                            .to_string(),
                    ),
                },
                Ok(_) => AgentOpResult {
                    notice: Some("Worktree discarded.".to_string()),
                    error: None,
                },
            })
        }
    }
}

/// Drain the inbox; auto-refresh the list at most every 5s (React's panel
/// interval). After a write op lands, refresh list + detail like
/// `refreshAll`.
pub fn poll_agents_state(s: &mut AgentsState, clock: f64) -> bool {
    let mut changed = false;
    let mut refresh_detail: Option<String> = None;
    while let Some(msg) = s.inbox.as_ref().and_then(|rx| rx.try_recv().ok()) {
        match msg {
            AgentThreadMsg::List(Ok(agents)) => {
                s.agents = agents;
                s.loading = false;
                s.last_ok = clock;
                changed = true;
            }
            AgentThreadMsg::List(Err(_)) => {
                // `fetchAgents` swallows to `[]`; the native fetch already
                // does, so this arm is unreachable — keep the board calm.
                s.loading = false;
                changed = true;
            }
            AgentThreadMsg::Detail(Ok(detail)) => {
                s.detail = detail.map(|d| *d);
                s.diff_busy = false;
                changed = true;
            }
            AgentThreadMsg::Detail(Err(message)) => {
                s.error = Some(message);
                s.diff_busy = false;
                changed = true;
            }
            AgentThreadMsg::Op(res) => {
                s.busy = false;
                s.confirm = None;
                s.error = res.error;
                s.notice = res.notice;
                refresh_detail = s.selected.clone();
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
    // `refreshAll` after writes: re-read list + detail. Queued as fresh
    // dispatches on the same open inbox; workers only send.
    if refresh_detail.is_some() {
        let id = refresh_detail.clone().unwrap_or_default();
        if !id.is_empty() {
            dispatch_agent(s, AgentOp::RefreshDetail { agent_id: id });
        }
        dispatch_agent(s, AgentOp::RefreshList);
    }
    if s.auto && !s.loading && clock - s.last_ok > 5.0 {
        s.loading = true;
        dispatch_agent(s, AgentOp::RefreshList);
        changed = true;
    }
    changed
}

/// Host access for the tree + drawer.
pub trait AgentsHost {
    fn agents_mut(&mut self) -> &mut AgentsState;
}

/// Parse a chat/agent timestamp: bare epoch seconds or ISO-8601
/// `YYYY-MM-DDTHH:MM:SSZ` (the server's wire shape). Naive stamps are local
/// wall time, matching `new Date()` in the React sidebar.
pub fn parse_timestamp(value: &str) -> Option<i64> {
    if let Ok(secs) = value.trim().parse::<i64>() {
        return Some(secs);
    }
    crate::rune_transcript::iso_epoch_secs(value)
}

/// `just now / 42s ago / 3m ago / 5h ago / 2d ago` (`relativeTime`).
pub fn relative_time(value: &str) -> String {
    if value.trim().is_empty() {
        return String::new();
    }
    let now = (crate::notes::now_millis() / 1000) as i64;
    let then = match parse_timestamp(value) {
        Some(t) => t,
        None => return String::new(),
    };
    let secs = (now - then).max(0);
    if secs < 10 {
        "just now".to_string()
    } else if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

fn agent_rows<S: AgentsHost + 'static>(
    agents: &[AgentNode],
    kids: &std::collections::HashMap<Option<String>, Vec<usize>>,
    expanded: &std::collections::HashSet<String>,
    parent: Option<String>,
    depth: usize,
) -> El {
    let mut out = col()
        .key(format!(
            "agents-kids-{}-{depth}",
            parent.clone().unwrap_or_default()
        ))
        .w_full()
        .gap(2.0);
    let rows = kids.get(&parent).cloned().unwrap_or_default();
    for i in rows {
        let agent = &agents[i];
        let grandchildren = kids
            .get(&Some(agent.id.clone()))
            .cloned()
            .unwrap_or_default();
        let is_open = expanded.contains(&agent.id);
        let toggle_id = agent.id.clone();
        let select_id = agent.id.clone();
        let cancel_id = agent.id.clone();
        let mut head = row()
            .key(format!("agent-{id}", id = agent.id))
            .w_full()
            .items_center()
            .gap(6.0)
            .radius(6.0)
            .pl(10.0 + depth as f32 * 12.0)
            .py(4.0)
            .child(
                text(if grandchildren.is_empty() {
                    " ".to_string()
                } else if is_open {
                    "▾".to_string()
                } else {
                    "▸".to_string()
                })
                .key(format!("agent-{id}-chev", id = agent.id))
                .role("button")
                .label("Expand agent")
                .font(rune::tok::TextStyle::new(11.0, 600.0))
                .color(ink("#71717a"))
                .on_press(move |s: &mut S| {
                    let st = s.agents_mut();
                    if !st.expanded.remove(&toggle_id) {
                        st.expanded.insert(toggle_id.clone());
                    }
                }),
            )
            .child(
                rune::el()
                    .key(format!("agent-{id}-dot", id = agent.id))
                    .size(6.0, 6.0)
                    .radius(3.0)
                    .fill(ink(status_ink(&agent.status))),
            )
            .child(
                text(agent.display())
                    .key(format!("agent-{id}-name", id = agent.id))
                    .grow(1.0)
                    .font(rune::tok::TextStyle::new(11.0, 400.0))
                    .color(ink("#d4d4d8"))
                    .role("button")
                    .label(format!("Open {name}", name = agent.display()))
                    .on_press(move |s: &mut S| {
                        open_agent(s.agents_mut(), &select_id);
                    }),
            )
            .child(
                text(agent.kind.clone())
                    .key(format!("agent-{id}-kind", id = agent.id))
                    .font(rune::tok::TextStyle::new(9.0, 600.0))
                    .color(ink("#52525b")),
            );
        if let Some(badge) = access_badge(&agent.access, &agent.worktree_path) {
            head = head.child(
                text(badge.to_string())
                    .key(format!("agent-{id}-badge", id = agent.id))
                    .font(rune::tok::TextStyle::new(8.5, 600.0))
                    .px(4.0)
                    .py(1.0)
                    .radius(4.0)
                    .fill(ink("#00000000"))
                    .border(1.0, ink("#ffffff1a"))
                    .color(ink("#71717a")),
            );
        }
        if agent.pending > 0 {
            head = head.child(
                text(agent.pending.to_string())
                    .key(format!("agent-{id}-pending", id = agent.id))
                    .font(rune::tok::TextStyle::new(8.5, 600.0))
                    .px(5.0)
                    .py(1.0)
                    .radius(7.0)
                    .fill(ink("#fbbf2426"))
                    .color(ink("#fcd34d")),
            );
        }
        if agent.status == "running" || agent.status == "queued" {
            head = head.child(
                rune::el()
                    .key(format!("agent-{id}-cancel", id = agent.id))
                    .size(12.0, 12.0)
                    .role("button")
                    .label(format!("Cancel {}", agent.display()))
                    .shape(crate::lucide_x(12.0, 2.0, ink("#71717a")))
                    .on_press(move |s: &mut S| {
                        dispatch_agent(
                            s.agents_mut(),
                            AgentOp::Cancel {
                                agent_id: cancel_id.clone(),
                            },
                        );
                    }),
            );
        }
        let mut item = col()
            .key(format!("agent-{id}-item", id = agent.id))
            .w_full()
            .child(head);
        let mut sub = agent.status.clone();
        let rel = relative_time(&agent.active);
        if !rel.is_empty() {
            sub.push_str(&format!(" · {rel}"));
        }
        if !agent.model.is_empty() {
            sub.push_str(&format!(" · {}", agent.model));
        }
        item = item.child(
            text(sub)
                .key(format!("agent-{id}-sub", id = agent.id))
                .pl(28.0 + depth as f32 * 12.0)
                .font(rune::tok::TextStyle::new(9.0, 400.0))
                .color(ink("#52525b")),
        );
        if is_open && !grandchildren.is_empty() {
            item = item.child(agent_rows::<S>(
                agents,
                kids,
                expanded,
                Some(agent.id.clone()),
                depth + 1,
            ));
        }
        out = out.child(item);
    }
    out
}

/// Select an agent and load its detail (the drawer's `openDetail`).
pub fn open_agent(s: &mut AgentsState, agent_id: &str) {
    s.selected = Some(agent_id.to_string());
    s.tab = AgentTab::Overview;
    s.error = None;
    s.notice = None;
    s.confirm = None;
    s.detail = None;
    dispatch_agent(
        s,
        AgentOp::RefreshDetail {
            agent_id: agent_id.to_string(),
        },
    );
}

/// Agents sidebar section: collapsible header with the count, then the
/// parent/child tree. Mounted in the Home sidebar under the projects rail.
pub fn agents_section<S: AgentsHost + 'static>(s: &AgentsState) -> El {
    let mut body = col().key("side-agents").w_full().gap(4.0).pt(10.0).child(
        row()
            .key("side-agents-head")
            .w_full()
            .items_center()
            .gap(6.0)
            .role("button")
            .label("Toggle agents")
            .on_press(move |s: &mut S| {
                s.agents_mut().open = !s.agents_mut().open;
            })
            .child(
                text("Agents".to_string())
                    .key("side-agents-title")
                    .grow(1.0)
                    .font(rune::tok::TextStyle::new(11.0, 700.0))
                    .color(ink("#71717a")),
            )
            .child(
                text(s.agents.len().to_string())
                    .key("side-agents-count")
                    .font(rune::tok::TextStyle::new(9.0, 400.0))
                    .color(ink("#52525b")),
            )
            .child(
                text(if s.open { "▾" } else { "▸" }.to_string())
                    .key("side-agents-chev")
                    .font(rune::tok::TextStyle::new(11.0, 600.0))
                    .color(ink("#71717a")),
            ),
    );
    if s.open {
        if s.agents.is_empty() {
            body = body.child(
                text("No agents yet. Delegations appear here.".to_string())
                    .key("side-agents-empty")
                    .font(rune::tok::TextStyle::new(10.0, 400.0))
                    .color(ink("#52525b")),
            );
        } else {
            let kids = children_of(&s.agents);
            body = body.child(agent_rows::<S>(&s.agents, &kids, &s.expanded, None, 0));
        }
        if let Some(err) = &s.error {
            body = body.child(
                text(err.clone())
                    .key("side-agents-error")
                    .font(rune::tok::TextStyle::new(10.0, 400.0))
                    .color(ink("#fda4af")),
            );
        }
    }
    body
}

/// Agent detail drawer: status head, overview/mailbox/worktree tabs, and the
/// action row. Merge/discard sit behind the two-tap `confirmAction` gate
/// (that confirm tap is the user go-ahead — both move real files). React's
/// "Open chat" has no Rune target (no per-agent chat view) and is dropped.
pub fn agent_sheet<S: AgentsHost + 'static>(s: &AgentsState) -> El {
    let selected: Option<AgentNode> = s
        .selected
        .as_ref()
        .and_then(|id| s.agents.iter().find(|a| &a.id == id).cloned())
        .or_else(|| s.detail.as_ref().map(|d| d.agent.clone()));
    let Some(agent) = selected else {
        return col().key("agent-sheet-empty").w_full();
    };
    let detail = s.detail.clone().unwrap_or_default();
    let mut sheet = col()
        .key("agent-card")
        .w(560.0)
        .radius(16.0)
        .fill(ink("#0c0c10"))
        .border(1.0, ink("#ffffff26"))
        .px(16.0)
        .pt(12.0)
        .pb(14.0)
        .gap(10.0)
        .child(
            row()
                .key("agent-sheet-head")
                .w_full()
                .items_center()
                .gap(8.0)
                .child(
                    rune::el()
                        .key("agent-sheet-dot")
                        .size(8.0, 8.0)
                        .radius(4.0)
                        .fill(ink(status_ink(&agent.status))),
                )
                .child(
                    col()
                        .grow(1.0)
                        .min_h(0.0)
                        .child(
                            text(agent.display())
                                .key("agent-sheet-title")
                                .font(rune::tok::TextStyle::new(13.0, 600.0))
                                .color(ink("#f4f4f5")),
                        )
                        .child({
                            let mut sub = agent.status.clone();
                            let rel = relative_time(&agent.active);
                            if !rel.is_empty() {
                                sub.push_str(&format!(" · {rel}"));
                            }
                            if !agent.model.is_empty() {
                                sub.push_str(&format!(" · {}", agent.model));
                            }
                            text(sub)
                                .key("agent-sheet-sub")
                                .font(rune::tok::TextStyle::new(10.0, 400.0))
                                .color(ink("#71717a"))
                        }),
                )
                .child(
                    rune::el()
                        .key("agent-sheet-close")
                        .size(14.0, 14.0)
                        .role("button")
                        .label("Close agent details")
                        .shape(crate::lucide_x(14.0, 2.0, ink("#8a8a93")))
                        .on_press(move |s: &mut S| {
                            let st = s.agents_mut();
                            st.selected = None;
                            st.detail = None;
                            st.confirm = None;
                        }),
                ),
        );
    // Tabs.
    let mut tabs = row().key("agent-sheet-tabs").w_full().gap(4.0);
    for (tab, label) in [
        (AgentTab::Overview, "Overview"),
        (AgentTab::Mailbox, "Mailbox"),
        (AgentTab::Worktree, "Worktree"),
    ] {
        let active = s.tab == tab;
        let mut btn = text(label.to_string())
            .key(format!("agent-tab-{label}"))
            .role("button")
            .label(format!("{label} tab"))
            .font(rune::tok::TextStyle::new(11.0, 400.0))
            .px(10.0)
            .py(4.0)
            .radius(6.0)
            .color(ink(if active { "#ffffff" } else { "#71717a" }))
            .on_press(move |s: &mut S| {
                s.agents_mut().tab = tab;
            });
        if active {
            btn = btn.fill(rune::Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.09,
            });
        }
        tabs = tabs.child(btn);
        if tab == AgentTab::Mailbox && !detail.pending.is_empty() {
            tabs = tabs.child(
                text(detail.pending.len().to_string())
                    .key("agent-tab-mailbox-count")
                    .font(rune::tok::TextStyle::new(9.0, 600.0))
                    .px(5.0)
                    .py(1.0)
                    .radius(7.0)
                    .fill(ink("#fbbf2433"))
                    .color(ink("#fcd34d")),
            );
        }
    }
    sheet = sheet.child(tabs);
    if let Some(err) = &s.error {
        sheet = sheet.child(
            text(err.clone())
                .key("agent-sheet-error")
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .color(ink("#fda4af")),
        );
    }
    if let Some(notice) = &s.notice {
        sheet = sheet.child(
            text(notice.clone())
                .key("agent-sheet-notice")
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .color(ink("#6ee7b7")),
        );
    }
    match s.tab {
        AgentTab::Overview => {
            sheet = sheet.child(agent_overview::<S>(&agent));
        }
        AgentTab::Mailbox => {
            sheet = sheet.child(agent_mailbox::<S>(&agent, &detail, &s.message_edit, s.busy));
        }
        AgentTab::Worktree => {
            sheet = sheet.child(agent_worktree::<S>(&agent, &detail, s));
        }
    }
    rune::el()
        .key("agent-overlay")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(rune::Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.7,
        })
        .center()
        .modal(true, move |s: &mut S| {
            let st = s.agents_mut();
            st.selected = None;
            st.detail = None;
            st.confirm = None;
        })
        .child(sheet)
}

fn fact(label: &str, value: String, key: String) -> El {
    col()
        .key(key)
        .grow(1.0)
        .min_h(0.0)
        .radius(8.0)
        .fill(rune::Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.03,
        })
        .border(1.0, ink("#ffffff1a"))
        .px(10.0)
        .py(8.0)
        .gap(2.0)
        .child(
            text(label.to_string())
                .font(rune::tok::TextStyle::new(9.0, 600.0))
                .color(ink("#52525b")),
        )
        .child(
            text(value)
                .font(rune::tok::TextStyle::new(12.0, 400.0))
                .color(ink("#d4d4d8")),
        )
}

fn agent_overview<S: AgentsHost + 'static>(agent: &AgentNode) -> El {
    let id = &agent.id;
    let badge = access_badge(&agent.access, &agent.worktree_path)
        .unwrap_or("direct")
        .to_string();
    col()
        .key(format!("agent-overview-{id}"))
        .w_full()
        .gap(8.0)
        .child(
            row()
                .key(format!("agent-facts-{id}"))
                .w_full()
                .gap(8.0)
                .child(fact(
                    "Kind",
                    format!("{} · depth {}", agent.kind, agent.depth),
                    format!("agent-fact-kind-{id}"),
                ))
                .child(fact("Access", badge, format!("agent-fact-access-{id}"))),
        )
        .child(
            row()
                .key(format!("agent-facts2-{id}"))
                .w_full()
                .gap(8.0)
                .child(fact(
                    "Scope",
                    agent.scope.clone(),
                    format!("agent-fact-scope-{id}"),
                ))
                .child(fact(
                    "Path",
                    agent.path.clone(),
                    format!("agent-fact-path-{id}"),
                )),
        )
        .child(
            row()
                .key(format!("agent-actions-{id}"))
                .w_full()
                .gap(8.0)
                .child(
                    text("Refresh".to_string())
                        .key(format!("agent-refresh-{id}"))
                        .role("button")
                        .label("Refresh agent")
                        .font(rune::tok::TextStyle::new(11.0, 400.0))
                        .px(12.0)
                        .py(6.0)
                        .radius(8.0)
                        .fill(ink("#00000000"))
                        .border(1.0, ink("#ffffff1a"))
                        .color(ink("#d4d4d8"))
                        .on_press({
                            let id = id.clone();
                            move |s: &mut S| {
                                dispatch_agent(
                                    s.agents_mut(),
                                    AgentOp::RefreshDetail {
                                        agent_id: id.clone(),
                                    },
                                );
                            }
                        }),
                ),
        )
}

fn agent_mailbox<S: AgentsHost + 'static>(
    agent: &AgentNode,
    detail: &AgentDetail,
    draft: &TextEdit,
    busy: bool,
) -> El {
    let id = agent.id.clone();
    let send_id = id.clone();
    let mut body = col()
        .key(format!("agent-mailbox-{id}"))
        .w_full()
        .gap(10.0)
        .child(
            row()
                .key(format!("agent-sendrow-{id}"))
                .w_full()
                .items_center()
                .gap(8.0)
                .child(
                    text_input(draft, rune::tok::TextStyle::new(12.0, 400.0))
                        .key(format!("agent-message-{id}"))
                        .grow(1.0)
                        .min_h(0.0)
                        .placeholder(
                            if agent.status == "running" {
                                "Steer the running agent…"
                            } else {
                                "Queue a message for the next run…"
                            },
                            ink("#52525b"),
                        )
                        .caret(ink("#f2f2f4"), ink("#3b82f6"))
                        .on_edit(move |s: &mut S| &mut s.agents_mut().message_edit),
                )
                .child(
                    text(if busy { "…" } else { "Send" }.to_string())
                        .key(format!("agent-send-{id}"))
                        .role("button")
                        .label("Send message")
                        .font(rune::tok::TextStyle::new(11.0, 600.0))
                        .px(12.0)
                        .py(8.0)
                        .radius(8.0)
                        .fill(ink("#ffffff"))
                        .color(ink("#0a0a0d"))
                        .on_press(move |s: &mut S| {
                            let st = s.agents_mut();
                            let message = st.message_edit.text.trim().to_string();
                            if message.is_empty() || st.busy {
                                return;
                            }
                            st.message_edit.text.clear();
                            dispatch_agent(
                                st,
                                AgentOp::SendMessage {
                                    agent_id: send_id.clone(),
                                    message,
                                },
                            );
                        }),
                ),
        );
    body = body.child(
        text(format!("Pending ({})", detail.pending.len()))
            .key(format!("agent-pending-head-{id}"))
            .font(rune::tok::TextStyle::new(10.0, 700.0))
            .color(ink("#71717a")),
    );
    if detail.pending.is_empty() {
        body = body.child(
            text("No queued messages.".to_string())
                .key(format!("agent-pending-empty-{id}"))
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .color(ink("#52525b")),
        );
    }
    for m in &detail.pending {
        body = body.child(
            col()
                .key(format!("agent-msg-{mid}", mid = m.id))
                .w_full()
                .gap(2.0)
                .radius(8.0)
                .fill(ink("#fbbf240d"))
                .border(1.0, ink("#fbbf2433"))
                .px(12.0)
                .py(8.0)
                .child(
                    text(format!(
                        "{} · {} · {}",
                        m.kind,
                        m.delivery,
                        relative_time(&m.created)
                    ))
                    .font(rune::tok::TextStyle::new(9.0, 600.0))
                    .color(ink("#fcd34d")),
                )
                .child(
                    text(m.body.clone())
                        .font(rune::tok::TextStyle::new(11.5, 400.0))
                        .wrap()
                        .color(ink("#e4e4e7")),
                ),
        );
    }
    body = body.child(
        text(format!("History ({})", detail.history.len()))
            .key(format!("agent-history-head-{id}"))
            .font(rune::tok::TextStyle::new(10.0, 700.0))
            .color(ink("#71717a")),
    );
    if detail.history.is_empty() {
        body = body.child(
            text("No mailbox history.".to_string())
                .key(format!("agent-history-empty-{id}"))
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .color(ink("#52525b")),
        );
    }
    for m in detail.history.iter().rev().take(30) {
        body = body.child(
            col()
                .key(format!("agent-hist-{mid}", mid = m.id))
                .w_full()
                .gap(2.0)
                .radius(8.0)
                .fill(ink("#00000000"))
                .border(1.0, ink("#ffffff1a"))
                .px(12.0)
                .py(8.0)
                .child(
                    text(format!(
                        "{} · {} · {} · {}",
                        m.kind,
                        m.delivery,
                        if m.consumed { "consumed" } else { "pending" },
                        relative_time(&m.created)
                    ))
                    .font(rune::tok::TextStyle::new(9.0, 400.0))
                    .color(ink("#52525b")),
                )
                .child(
                    text(m.body.clone())
                        .font(rune::tok::TextStyle::new(11.0, 400.0))
                        .wrap()
                        .color(ink("#a1a1aa")),
                ),
        );
    }
    body
}

fn agent_worktree<S: AgentsHost + 'static>(
    agent: &AgentNode,
    detail: &AgentDetail,
    s: &AgentsState,
) -> El {
    let id = agent.id.clone();
    let mut body = col().key(format!("agent-worktree-{id}")).w_full().gap(8.0);
    if agent.worktree_path.is_empty() {
        return body.child(
            text("No isolated worktree. This agent edits the live workspace directly.".to_string())
                .key(format!("agent-nowt-{id}"))
                .font(rune::tok::TextStyle::new(11.5, 400.0))
                .color(ink("#71717a")),
        );
    }
    body = body.child(
        col()
            .key(format!("agent-wt-{id}"))
            .w_full()
            .gap(4.0)
            .radius(8.0)
            .fill(rune::Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.03,
            })
            .border(1.0, ink("#ffffff1a"))
            .px(10.0)
            .py(8.0)
            .child(
                row()
                    .key(format!("agent-wt-head-{id}"))
                    .w_full()
                    .items_center()
                    .gap(6.0)
                    .child(
                        text(detail.worktree_branch.clone())
                            .font(rune::tok::TextStyle::new(11.0, 400.0))
                            .color(ink("#a1a1aa")),
                    )
                    .child(
                        text(if detail.worktree_state.is_empty() {
                            "active".to_string()
                        } else {
                            detail.worktree_state.clone()
                        })
                        .key(format!("agent-wt-state-{id}"))
                        .font(rune::tok::TextStyle::new(9.0, 600.0))
                        .px(6.0)
                        .py(2.0)
                        .radius(4.0)
                        .fill(ink("#00000000"))
                        .border(1.0, ink("#ffffff1a"))
                        .color(ink("#71717a")),
                    ),
            )
            .child(
                text(agent.worktree_path.clone())
                    .key(format!("agent-wt-path-{id}"))
                    .font(rune::tok::TextStyle::new(10.5, 400.0).mono())
                    .color(ink("#71717a")),
            )
            .child(
                text(format!(
                    "base {}",
                    if detail.worktree_base.is_empty() {
                        "unknown"
                    } else {
                        &detail.worktree_base
                    }
                ))
                .key(format!("agent-wt-base-{id}"))
                .font(rune::tok::TextStyle::new(10.0, 400.0).mono())
                .color(ink("#52525b")),
            ),
    );
    if !detail.worktree_error.is_empty() {
        body = body.child(
            text(detail.worktree_error.clone())
                .key(format!("agent-wt-error-{id}"))
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .color(ink("#fda4af")),
        );
    }
    if !detail.diff_stat.is_empty() {
        body = body.child(
            text(detail.diff_stat.clone())
                .key(format!("agent-diff-{id}"))
                .font(rune::tok::TextStyle::new(10.5, 400.0).mono())
                .wrap()
                .color(ink("#d4d4d8")),
        );
    }
    if !detail.diff_files.is_empty() {
        let mut files = col()
            .key(format!("agent-files-{id}"))
            .w_full()
            .gap(2.0)
            .radius(8.0)
            .fill(rune::Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.03,
            })
            .border(1.0, ink("#ffffff1a"))
            .px(10.0)
            .py(8.0)
            .child(
                text("Files".to_string())
                    .font(rune::tok::TextStyle::new(9.0, 600.0))
                    .color(ink("#52525b")),
            );
        for (status, path) in detail.diff_files.iter().take(40) {
            files = files.child(
                row()
                    .key(format!("agent-file-{id}-{path}"))
                    .w_full()
                    .gap(8.0)
                    .child(
                        text(status.clone())
                            .font(rune::tok::TextStyle::new(10.5, 400.0).mono())
                            .color(ink("#71717a")),
                    )
                    .child(
                        text(path.clone())
                            .font(rune::tok::TextStyle::new(10.5, 400.0).mono())
                            .color(ink("#d4d4d8")),
                    ),
            );
        }
        body = body.child(files);
    }
    let diff_id = id.clone();
    let merge_id = id.clone();
    let discard_id = id.clone();
    body = body.child(
        row()
            .key(format!("agent-wt-actions-{id}"))
            .w_full()
            .gap(8.0)
            .child(
                text(
                    if s.diff_busy {
                        "Loading…"
                    } else {
                        "Refresh diff"
                    }
                    .to_string(),
                )
                .key(format!("agent-diff-refresh-{id}"))
                .role("button")
                .label("Refresh diff")
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .px(12.0)
                .py(6.0)
                .radius(8.0)
                .fill(ink("#00000000"))
                .border(1.0, ink("#ffffff1a"))
                .color(ink("#d4d4d8"))
                .on_press(move |s: &mut S| {
                    s.agents_mut().diff_busy = true;
                    dispatch_agent(
                        s.agents_mut(),
                        AgentOp::RefreshDiff {
                            agent_id: diff_id.clone(),
                        },
                    );
                }),
            )
            .child(
                text("Merge".to_string())
                    .key(format!("agent-merge-{id}"))
                    .role("button")
                    .label("Merge worktree")
                    .font(rune::tok::TextStyle::new(11.0, 600.0))
                    .px(12.0)
                    .py(6.0)
                    .radius(8.0)
                    .fill(ink("#ffffff"))
                    .color(ink("#0a0a0d"))
                    .on_press(move |s: &mut S| {
                        // First tap arms the confirm gate; the second fires.
                        s.agents_mut().confirm = Some(format!("merge:{merge_id}"));
                    }),
            )
            .child(
                text("Discard".to_string())
                    .key(format!("agent-discard-{id}"))
                    .role("button")
                    .label("Discard worktree")
                    .font(rune::tok::TextStyle::new(11.0, 400.0))
                    .px(12.0)
                    .py(6.0)
                    .radius(8.0)
                    .fill(ink("#00000000"))
                    .border(1.0, ink("#f871714d"))
                    .color(ink("#fda4af"))
                    .on_press(move |s: &mut S| {
                        s.agents_mut().confirm = Some(format!("discard:{discard_id}"));
                    }),
            ),
    );
    if let Some(confirm) = s.confirm.clone() {
        let (action, target) = confirm.split_once(':').unwrap_or(("", ""));
        if target == id {
            let prompt = if action == "merge" {
                "Merge this worktree into the target tree? Review the diff first."
            } else {
                "Discard this worktree? Unmerged changes are lost."
            };
            let fire_id = id.clone();
            let is_merge = action == "merge";
            body = body.child(
                col()
                    .key(format!("agent-confirm-{id}"))
                    .w_full()
                    .gap(8.0)
                    .radius(8.0)
                    .fill(ink("#fbbf240f"))
                    .border(1.0, ink("#fbbf2440"))
                    .px(12.0)
                    .py(10.0)
                    .child(
                        text(prompt.to_string())
                            .font(rune::tok::TextStyle::new(11.5, 400.0))
                            .color(ink("#fde68a")),
                    )
                    .child(
                        row()
                            .w_full()
                            .justify_end()
                            .gap(8.0)
                            .child(
                                text("Cancel".to_string())
                                    .key(format!("agent-confirm-cancel-{id}"))
                                    .role("button")
                                    .label("Cancel")
                                    .font(rune::tok::TextStyle::new(11.0, 400.0))
                                    .px(12.0)
                                    .py(6.0)
                                    .radius(8.0)
                                    .fill(ink("#00000000"))
                                    .border(1.0, ink("#ffffff1a"))
                                    .color(ink("#a1a1aa"))
                                    .on_press(move |s: &mut S| {
                                        s.agents_mut().confirm = None;
                                    }),
                            )
                            .child(
                                text(
                                    if is_merge {
                                        "Confirm merge"
                                    } else {
                                        "Confirm discard"
                                    }
                                    .to_string(),
                                )
                                .key(format!("agent-confirm-fire-{id}"))
                                .role("button")
                                .label("Confirm")
                                .font(rune::tok::TextStyle::new(11.0, 600.0))
                                .px(12.0)
                                .py(6.0)
                                .radius(8.0)
                                .fill(ink(if is_merge { "#ffffff" } else { "#ef4444" }))
                                .color(ink(if is_merge { "#0a0a0d" } else { "#ffffff" }))
                                .on_press(move |s: &mut S| {
                                    let op = if is_merge {
                                        AgentOp::Merge {
                                            agent_id: fire_id.clone(),
                                        }
                                    } else {
                                        AgentOp::Discard {
                                            agent_id: fire_id.clone(),
                                        }
                                    };
                                    dispatch_agent(s.agents_mut(), op);
                                }),
                            ),
                    ),
            );
        }
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    fn node(id: &str, parent: Option<&str>, status: &str) -> AgentNode {
        AgentNode {
            id: id.to_string(),
            kind: "subagent".to_string(),
            path: format!("/ws/{id}"),
            parent_id: parent.map(str::to_string),
            scope: "ws".to_string(),
            role: "coder".to_string(),
            nickname: format!("nick-{id}"),
            status: status.to_string(),
            model: "m".to_string(),
            depth: 1,
            result: String::new(),
            access: "worktree".to_string(),
            worktree_path: "/tmp/wt".to_string(),
            pending: 0,
            active: String::new(),
        }
    }

    #[test]
    fn node_parses_live_shape() {
        let v = serde_json::json!({
            "id": "a1", "kind": "workspace", "path": "/w", "parentId": null,
            "scope": "s", "role": "r", "nickname": "n", "status": "running",
            "model": "m", "depth": 0, "access": "write",
            "worktree": {"path": "/tmp/x", "branch": "b", "base": "main", "state": "active"},
            "pendingMessages": 2, "lastActive": "2026-11-09T10:00:00Z",
        });
        let n = AgentNode::from_json(&v);
        assert_eq!(n.display(), "n");
        assert_eq!(n.pending, 2);
        assert_eq!(n.worktree_path, "/tmp/x");
        assert!(n.parent_id.is_none());
        assert_eq!(access_badge("write", "/tmp/x"), Some("isolated"));
        assert_eq!(access_badge("write", ""), Some("direct"));
        assert_eq!(access_badge("read", ""), Some("read-only"));
        assert_eq!(access_badge("", ""), None);
    }

    #[test]
    fn detail_parses_messages_and_diff() {
        let v = serde_json::json!({
            "agent": {"id": "a1", "status": "idle",
                      "worktree": {"branch": "b", "diff": {"stat": "3 files", "files": [{"status": "M", "path": "f.rs"}]}}},
            "messages": [{"id": "m1", "kind": "steer", "body": "go", "delivery": "queued", "createdAt": "2026-11-09T10:00:00Z"}],
            "history": [],
        });
        let d = AgentDetail::from_json(&v).expect("detail");
        assert_eq!(d.pending.len(), 1);
        assert!(!d.pending[0].consumed);
        assert_eq!(d.diff_stat, "3 files");
        assert_eq!(d.diff_files, vec![("M".to_string(), "f.rs".to_string())]);
        assert!(AgentDetail::from_json(&serde_json::json!({})).is_none());
    }

    #[test]
    fn children_group_by_parent() {
        let agents = vec![
            node("root", None, "running"),
            node("kid", Some("root"), "idle"),
        ];
        let kids = children_of(&agents);
        assert_eq!(kids.get(&None).unwrap().len(), 1);
        assert_eq!(kids.get(&Some("root".to_string())).unwrap().len(), 1);
    }

    #[test]
    fn relative_time_buckets() {
        let now = (crate::notes::now_millis() / 1000) as i64;

        assert_eq!(relative_time(&now.to_string()), "just now");
        assert_eq!(relative_time(&(now - 70).to_string()), "1m ago");
        assert_eq!(relative_time(&(now - 7200).to_string()), "2h ago");
        assert_eq!(relative_time(""), "");
        assert_eq!(relative_time("not-a-date"), "");
    }

    /// Section + sheet mounted shell-style (the tree lives in the side
    /// panel now, not the canvas body).
    fn section_view(
        s: &crate::rune_canvas::CanvasState,
        _: &mut rune::Cx<crate::rune_canvas::CanvasState>,
    ) -> rune::El {
        let mut root = rune::col()
            .key("agents-test-root")
            .w_full()
            .h_full()
            .child(crate::rune_canvas::canvas_body::<
                crate::rune_canvas::CanvasState,
            >(s))
            .child(agents_section::<crate::rune_canvas::CanvasState>(&s.agents));
        if s.agents.selected.is_some() {
            root = root.child(agent_sheet::<crate::rune_canvas::CanvasState>(&s.agents));
        }
        root
    }

    #[test]
    fn tree_mounts_and_select_opens_sheet() {
        let env = Env::new(true, 1.0);
        let mut init = crate::rune_canvas::CanvasState::map();
        init.agents.agents = vec![
            node("root", None, "running"),
            node("kid", Some("root"), "idle"),
        ];
        init.agents.auto = false;
        let mut rt = Runtime::new(
            init,
            section_view,
            (crate::rune_canvas::MAP_W, crate::rune_canvas::MAP_H),
            env,
        );
        rt.frame(0.0);
        for key in ["side-agents", "agent-root-name"] {
            assert!(rt.find(key).is_some(), "{key} mounted");
        }
        // Expand root to show the child row (collapsed by default).
        let (x, y) = rt.find("agent-root-chev").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.find("agent-kid-name").is_some(), "child row expands");
        // Name press selects + opens the drawer (detail fetch fails with no
        // server — the sheet still mounts off the list row).
        let (x, y) = rt.find("agent-root-name").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(rt.state.agents.selected.as_deref(), Some("root"));
        assert!(rt.find("agent-overlay").is_some(), "drawer mounts");
        assert!(rt.find("agent-tab-Mailbox").is_some());
        // Mailbox tab switches.
        let (x, y) = rt.find("agent-tab-Mailbox").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.find("agent-mailbox-root").is_some(), "mailbox mounts");
    }

    #[test]
    fn merge_discard_gate_arms_before_firing() {
        let env = Env::new(true, 1.0);
        let mut init = crate::rune_canvas::CanvasState::map();
        let mut a = node("wt1", None, "completed");
        a.worktree_path = "/tmp/wt1".to_string();
        init.agents.agents = vec![a];
        init.agents.auto = false;
        init.agents.selected = Some("wt1".to_string());
        init.agents.detail = Some(AgentDetail {
            agent: node("wt1", None, "completed"),
            ..AgentDetail::default()
        });
        init.agents.tab = AgentTab::Worktree;
        let mut rt = Runtime::new(
            init,
            section_view,
            (crate::rune_canvas::MAP_W, crate::rune_canvas::MAP_H),
            env,
        );
        rt.frame(0.0);
        assert!(rt.find("agent-merge-wt1").is_some(), "merge mounts");
        // First tap arms only — no worker, no close.
        let (x, y) = rt.find("agent-merge-wt1").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(
            rt.state.agents.confirm.as_deref(),
            Some("merge:wt1"),
            "first tap arms"
        );
        assert!(
            rt.find("agent-confirm-fire-wt1").is_some(),
            "confirm mounts"
        );
        assert!(rt.state.agents.selected.is_some(), "drawer stays open");
    }
}
