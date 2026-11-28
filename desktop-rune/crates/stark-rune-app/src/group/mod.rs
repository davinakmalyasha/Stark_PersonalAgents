//! Group-project detail state and API. The modal/chat/rail live in this
//! module; `projects.rs` owns the board list and opens a detail here.

pub mod chat;
pub mod data;
pub mod modal;
pub mod notes;
pub mod rail;
pub mod settings;
pub mod ui;

use std::sync::mpsc::Receiver;

use data::{GroupDetail, GroupItem};

/// The three rail tabs (`GroupProjectRail.tsx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RailTab {
    #[default]
    Board,
    Team,
    Activity,
}

/// The composer's target/mention menu (`GroupProjectChat.tsx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Menu {
    #[default]
    None,
    Target,
    Mention,
}

struct GroupMsg {
    detail: Result<GroupDetail, String>,
    /// An action (approval/op/message) rather than the 3 s poll. Errors keep
    /// the visible detail and show the notice line.
    action: bool,
}

/// All group-modal state. Owned by `ProjectsState` so the rail, board and
/// modal share one poll and one detail snapshot.
pub struct GroupState {
    pub host: String,
    pub port: u16,
    pub open_id: Option<String>,
    /// Project the route wants to open once the live Group list contains it.
    /// The modal stays unmounted while this is pending.
    pub pending_open: Option<String>,
    pub detail: Option<GroupDetail>,
    pub loading: bool,
    pub error: Option<String>,
    pub action_error: Option<String>,
    pub queued: Option<String>,
    pub path_note: Option<String>,
    pub busy_item: Option<String>,
    pub op_busy: Option<String>,
    pub show_rail: bool,
    pub rail_tab: RailTab,
    /// Board item expanded in the rail (`GroupProjectBoard` local state).
    pub board_open: Option<String>,
    /// Expanded item inside the board (separate from the open milestone).
    pub item_open: Option<String>,
    /// Details activity list open.
    pub show_activity: bool,
    pub filter: String,
    pub target: String,
    pub menu: Menu,
    /// Chat bodies expanded past the 260 pt clamp (`ChatBody` local state).
    pub expanded: std::collections::HashSet<String>,
    pub draft: rune::TextEdit,
    pub sending: bool,
    pub send_error: Option<String>,
    pub settings: settings::GroupSettingsState,
    pub activity_filter: String,
    pub show_past_cycles: bool,
    pub new_milestone: rune::TextEdit,
    pub new_milestone_open: bool,
    pub new_item: rune::TextEdit,
    pub new_item_milestone: Option<String>,
    pub item_drafts: std::collections::HashMap<String, rune::TextEdit>,
    pub editing_title: Option<String>,
    pub status_busy: Option<String>,
    pub selected_run: Option<crate::group::data::GroupRun>,
    pub trash_busy: bool,
    pub trash_inbox: Option<Receiver<Result<(), String>>>,
    pub pending_settings: bool,
    pub notes: crate::notes::NotesState,
    last_ok: f64,
    last_try: f64,
    inbox: Option<Receiver<GroupMsg>>,
}

impl std::fmt::Debug for GroupState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GroupState")
            .field("open_id", &self.open_id)
            .field("detail", &self.detail.is_some())
            .field("loading", &self.loading)
            .field("show_rail", &self.show_rail)
            .finish()
    }
}

impl Clone for GroupState {
    fn clone(&self) -> Self {
        Self {
            host: self.host.clone(),
            port: self.port,
            open_id: self.open_id.clone(),
            pending_open: self.pending_open.clone(),
            detail: self.detail.clone(),
            loading: self.loading,
            error: self.error.clone(),
            action_error: self.action_error.clone(),
            queued: self.queued.clone(),
            path_note: self.path_note.clone(),
            busy_item: self.busy_item.clone(),
            op_busy: self.op_busy.clone(),
            show_rail: self.show_rail,
            rail_tab: self.rail_tab,
            board_open: self.board_open.clone(),
            item_open: self.item_open.clone(),
            show_activity: self.show_activity,
            filter: self.filter.clone(),
            target: self.target.clone(),
            menu: self.menu,
            expanded: self.expanded.clone(),
            draft: self.draft.clone(),
            sending: self.sending,
            send_error: self.send_error.clone(),
            settings: self.settings.clone(),
            activity_filter: self.activity_filter.clone(),
            show_past_cycles: self.show_past_cycles,
            new_milestone: self.new_milestone.clone(),
            new_milestone_open: self.new_milestone_open,
            new_item: self.new_item.clone(),
            new_item_milestone: self.new_item_milestone.clone(),
            item_drafts: self.item_drafts.clone(),
            editing_title: self.editing_title.clone(),
            status_busy: self.status_busy.clone(),
            selected_run: self.selected_run.clone(),
            trash_busy: self.trash_busy,
            trash_inbox: None,
            pending_settings: self.pending_settings,
            notes: self.notes.clone(),
            last_ok: self.last_ok,
            last_try: self.last_try,
            inbox: None,
        }
    }
}

impl Default for GroupState {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8124,
            open_id: None,
            pending_open: None,
            detail: None,
            loading: false,
            error: None,
            action_error: None,
            queued: None,
            path_note: None,
            busy_item: None,
            op_busy: None,
            show_rail: false,
            rail_tab: RailTab::Board,
            board_open: None,
            item_open: None,
            show_activity: true,
            filter: "everyone".to_string(),
            target: "all".to_string(),
            menu: Menu::None,
            expanded: std::collections::HashSet::new(),
            draft: rune::TextEdit::new(""),
            sending: false,
            send_error: None,
            settings: settings::GroupSettingsState::default(),
            activity_filter: "all".to_string(),
            show_past_cycles: false,
            new_milestone: rune::TextEdit::new(""),
            new_milestone_open: false,
            new_item: rune::TextEdit::new(""),
            new_item_milestone: None,
            item_drafts: std::collections::HashMap::new(),
            editing_title: None,
            status_busy: None,
            selected_run: None,
            trash_busy: false,
            trash_inbox: None,
            pending_settings: false,
            notes: crate::notes::NotesState::default(),
            last_ok: 0.0,
            last_try: 0.0,
            inbox: None,
        }
    }
}

impl GroupState {
    pub fn is_open(&self) -> bool {
        self.open_id.is_some()
    }

    /// Open a project and load its detail immediately. Reopening the same
    /// project keeps the visible snapshot while the refresh runs.
    pub fn open(&mut self, host: String, port: u16, id: String) {
        self.host = host;
        self.port = port;
        self.open_id = Some(id.clone());
        self.pending_open = None;
        self.detail = None;
        self.loading = true;
        self.error = None;
        self.action_error = None;
        self.queued = None;
        self.path_note = None;
        self.busy_item = None;
        self.op_busy = None;
        self.show_rail = false;
        self.rail_tab = RailTab::Board;
        self.filter = "everyone".to_string();
        self.board_open = None;
        self.item_open = None;
        self.show_activity = true;
        self.target = "all".to_string();
        self.menu = Menu::None;
        self.expanded.clear();
        self.draft.text.clear();
        self.send_error = None;
        self.notes.host = self.host.clone();
        self.notes.port = port;
        self.settings = settings::GroupSettingsState::default();
        self.activity_filter = "all".to_string();
        self.show_past_cycles = false;
        self.new_milestone_open = false;
        self.new_milestone.text.clear();
        self.new_item.text.clear();
        self.new_item_milestone = None;
        self.item_drafts.clear();
        self.editing_title = None;
        self.status_busy = None;
        self.selected_run = None;
        self.trash_busy = false;
        self.trash_inbox = None;
        self.pending_settings = false;
        self.spawn_detail(id, false);
    }

    /// Open a project directly on its settings sheet. Used by the sidebar's
    /// Edit action; the sheet waits for the first detail snapshot.
    pub fn open_with_settings(&mut self, host: String, port: u16, id: String) {
        self.open(host, port, id);
        self.pending_settings = true;
    }

    pub fn close(&mut self) {
        self.open_id = None;
        self.pending_open = None;
        self.detail = None;
        self.loading = false;
        self.error = None;
        self.action_error = None;
        self.queued = None;
        self.path_note = None;
        self.busy_item = None;
        self.op_busy = None;
        self.sending = false;
        self.send_error = None;
        self.inbox = None;
        self.notes.open = false;
        self.settings.open = false;
        self.selected_run = None;
    }

    fn spawn_detail(&mut self, id: String, action: bool) {
        self.last_try = crate::clock::now_millis() as f64 / 1000.0;
        let (tx, rx) = std::sync::mpsc::channel();
        self.inbox = Some(rx);
        #[cfg(not(target_arch = "wasm32"))]
        let (host, port) = (self.host.clone(), self.port);
        #[cfg(target_arch = "wasm32")]
        {
            wasm_bindgen_futures::spawn_local(async move {
                let path = format!("/api/groups/{}?events=200", id);
                let detail = match crate::transport::fetch_json(&path).await {
                    Ok(v) => Ok(GroupDetail::from_json(&v)),
                    Err(e) => Err(e),
                };
                let _ = tx.send(GroupMsg { detail, action });
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::spawn(move || {
            let detail = fetch_detail(&host, port, &id);
            let _ = tx.send(GroupMsg { detail, action });
        });
    }

    /// Drain one snapshotted request and keep the 3 s detail poll alive.
    pub fn poll(&mut self, clock: f64) -> bool {
        if let Some(rx) = self.trash_inbox.take() {
            match rx.try_recv() {
                Ok(Ok(())) => {
                    self.trash_busy = false;
                    self.close();
                    return true;
                }
                Ok(Err(error)) => {
                    self.trash_busy = false;
                    self.action_error = Some(error);
                    return true;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    self.trash_inbox = Some(rx);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.trash_busy = false;
                    self.action_error = Some("Could not move the project to Trash.".to_string());
                }
            }
        }
        let Some(rx) = self.inbox.take() else {
            if self.open_id.is_some()
                && clock - self.last_ok >= 3.0
                && clock - self.last_try >= 3.0
                && !self.loading
            {
                let id = self.open_id.clone().unwrap_or_default();
                self.spawn_detail(id, false);
            }
            return false;
        };
        let mut changed = false;
        match rx.try_recv() {
            Ok(msg) => {
                let finished_op = self.op_busy.take();
                let was_sending = self.sending;
                self.loading = false;
                self.sending = false;
                self.busy_item = None;
                match msg.detail {
                    Ok(detail) => {
                        self.detail = Some(detail);
                        if self.pending_settings {
                            self.pending_settings = false;
                            crate::group::settings::open(self);
                        }
                        self.error = None;
                        self.action_error = None;
                        self.last_ok = clock;
                        if let Some(op) = finished_op {
                            let verb = match op.as_str() {
                                "close" => "Close",
                                "pause" => "Pause",
                                "resume" => "Resume",
                                other => other,
                            };
                            self.queued = Some(format!("{verb} queued — applies within ~15s."));
                        }
                    }
                    Err(error) => {
                        if was_sending {
                            self.send_error = Some(error);
                        } else if msg.action && self.detail.is_some() {
                            self.action_error = Some(error);
                        } else {
                            self.error = Some(error);
                        }
                    }
                }
                changed = true;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.inbox = Some(rx);
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.loading = false;
                changed = true;
            }
        }
        if self.open_id.is_some()
            && clock - self.last_ok >= 3.0
            && clock - self.last_try >= 3.0
            && !self.loading
            && self.inbox.is_none()
        {
            let id = self.open_id.clone().unwrap_or_default();
            self.spawn_detail(id, false);
        }
        changed
    }

    pub fn resolve(&mut self, item: &GroupItem, grant: bool) {
        let Some(id) = self.open_id.clone() else {
            return;
        };
        self.busy_item = Some(item.id.clone());
        self.action_error = None;
        self.queued = None;
        #[cfg(not(target_arch = "wasm32"))]
        let (host, port) = (self.host.clone(), self.port);
        let item_id = item.id.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.inbox = Some(rx);
        let body = serde_json::json!({ "grant": grant, "note": "" });
        let path = format!("/api/groups/{id}/items/{item_id}/approval");
        let id_for_fetch = id.clone();
        #[cfg(target_arch = "wasm32")]
        {
            wasm_bindgen_futures::spawn_local(async move {
                let result = match crate::transport::fetch_send("POST", &path, Some(&body)).await {
                    Ok(_) => fetch_detail_wasm(&id_for_fetch).await,
                    Err(e) => Err(e),
                };
                let _ = tx.send(GroupMsg {
                    detail: result,
                    action: true,
                });
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::spawn(move || {
            let result = crate::transport::send_json(&host, port, "POST", &path, &body)
                .map(|_| ())
                .and_then(|()| fetch_detail(&host, port, &id_for_fetch));
            let _ = tx.send(GroupMsg {
                detail: result,
                action: true,
            });
        });
    }

    pub fn op(&mut self, op: &str) {
        self.op_with(op, serde_json::json!({}));
    }

    pub fn op_with(&mut self, op: &str, params: serde_json::Value) {
        let Some(id) = self.open_id.clone() else {
            return;
        };
        self.op_busy = Some(op.to_string());
        self.action_error = None;
        self.queued = None;
        #[cfg(not(target_arch = "wasm32"))]
        let (host, port) = (self.host.clone(), self.port);
        let body = serde_json::json!({ "op": op, "params": params });
        let path = format!("/api/groups/{id}/update");
        let id_for_fetch = id.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.inbox = Some(rx);
        #[cfg(target_arch = "wasm32")]
        {
            wasm_bindgen_futures::spawn_local(async move {
                let result = match crate::transport::fetch_send("POST", &path, Some(&body)).await {
                    Ok(_) => fetch_detail_wasm(&id_for_fetch).await,
                    Err(e) => Err(e),
                };
                let _ = tx.send(GroupMsg {
                    detail: result,
                    action: true,
                });
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::spawn(move || {
            let result = crate::transport::send_json(&host, port, "POST", &path, &body)
                .map(|_| ())
                .and_then(|()| fetch_detail(&host, port, &id_for_fetch));
            let _ = tx.send(GroupMsg {
                detail: result,
                action: true,
            });
        });
    }

    pub fn send(&mut self) {
        let Some(id) = self.open_id.clone() else {
            return;
        };
        let body = self.draft.text.trim().to_string();
        if body.is_empty() || self.sending {
            return;
        }
        self.sending = true;
        self.send_error = None;
        self.action_error = None;
        self.queued = None;
        self.menu = Menu::None;
        self.draft.text.clear();
        let target = self.target.clone();
        #[cfg(not(target_arch = "wasm32"))]
        let (host, port) = (self.host.clone(), self.port);
        let payload = serde_json::json!({
            "body": body,
            "content": body,
            "to": target,
            "target": target,
            "from": "owner",
            "kind": "note",
        });
        let path = format!("/api/groups/{id}/messages");
        let fallback = format!("/api/groups/{id}/update");
        let id_for_fetch = id.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.inbox = Some(rx);
        #[cfg(target_arch = "wasm32")]
        {
            wasm_bindgen_futures::spawn_local(async move {
                let result = match crate::transport::fetch_send("POST", &path, Some(&payload)).await
                {
                    Ok(_) => fetch_detail_wasm(&id_for_fetch).await,
                    Err(_) => {
                        let fallback_body =
                            serde_json::json!({ "op": "message", "params": payload });
                        match crate::transport::fetch_send("POST", &fallback, Some(&fallback_body))
                            .await
                        {
                            Ok(_) => fetch_detail_wasm(&id_for_fetch).await,
                            Err(e) => Err(e),
                        }
                    }
                };
                let _ = tx.send(GroupMsg {
                    detail: result,
                    action: true,
                });
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::spawn(move || {
            let posted = crate::transport::send_json(&host, port, "POST", &path, &payload);
            let result = match posted {
                Ok(_) => fetch_detail(&host, port, &id_for_fetch),
                Err(_) => {
                    let fallback_body = serde_json::json!({ "op": "message", "params": payload });
                    crate::transport::send_json(&host, port, "POST", &fallback, &fallback_body)
                        .map(|_| ())
                        .and_then(|()| fetch_detail(&host, port, &id_for_fetch))
                }
            };
            let _ = tx.send(GroupMsg {
                detail: result,
                action: true,
            });
        });
    }

    pub fn stop(&mut self) {
        self.op("stop");
    }

    pub fn approve_plan(&mut self) {
        let Some(id) = self.open_id.clone() else {
            return;
        };
        self.op_busy = Some("approve_plan".to_string());
        self.action_error = None;
        let (tx, rx) = std::sync::mpsc::channel();
        self.inbox = Some(rx);
        #[cfg(not(target_arch = "wasm32"))]
        let (host, port) = (self.host.clone(), self.port);
        let path = format!("/api/groups/{id}/approve-plan");
        #[cfg(target_arch = "wasm32")]
        {
            wasm_bindgen_futures::spawn_local(async move {
                let result =
                    match crate::transport::fetch_send("POST", &path, Some(&serde_json::json!({})))
                        .await
                    {
                        Ok(_) => fetch_detail_wasm(&id).await,
                        Err(e) => Err(e),
                    };
                let _ = tx.send(GroupMsg {
                    detail: result,
                    action: true,
                });
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::spawn(move || {
            let result =
                crate::transport::send_json(&host, port, "POST", &path, &serde_json::json!({}))
                    .map(|_| ())
                    .and_then(|()| fetch_detail(&host, port, &id));
            let _ = tx.send(GroupMsg {
                detail: result,
                action: true,
            });
        });
    }

    pub fn trash_project(&mut self) {
        let Some(id) = self.open_id.clone() else {
            return;
        };
        if self.trash_busy {
            return;
        }
        self.trash_busy = true;
        self.action_error = None;
        let (tx, rx) = std::sync::mpsc::channel();
        self.trash_inbox = Some(rx);
        #[cfg(not(target_arch = "wasm32"))]
        let (host, port) = (self.host.clone(), self.port);
        let path = format!("/api/groups/{id}");
        #[cfg(target_arch = "wasm32")]
        {
            wasm_bindgen_futures::spawn_local(async move {
                let result = match crate::transport::fetch_send("DELETE", &path, None).await {
                    Ok(_) => Ok(()),
                    Err(e) => Err(e),
                };
                let _ = tx.send(result);
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::spawn(move || {
            let result =
                crate::transport::send_json(&host, port, "DELETE", &path, &serde_json::json!({}))
                    .map(|_| ());
            let _ = tx.send(result);
        });
    }

    pub fn set_path_note(&mut self, note: String) {
        self.path_note = Some(note);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn fetch_detail(host: &str, port: u16, id: &str) -> Result<GroupDetail, String> {
    let path = format!("/api/groups/{id}?events=200");
    crate::transport::get_json(host, port, &path).map(|v| GroupDetail::from_json(&v))
}

#[cfg(target_arch = "wasm32")]
async fn fetch_detail_wasm(id: &str) -> Result<GroupDetail, String> {
    let path = format!("/api/groups/{id}?events=200");
    match crate::transport::fetch_json(&path).await {
        Ok(v) => Ok(GroupDetail::from_json(&v)),
        Err(e) => Err(e),
    }
}
