//! House + Memory rooms — ports of `HouseWorkspace` (read-only half) and
//! `MemoryWorkspace` (list half).
//!
//! Data contracts (verified live against `stark-server`):
//! - `GET /api/house/state` → `{rooms:[{id,name}], devices:[{id,name,roomId,
//!   type,state,capabilities,isOnline}], scenes, energy, security}`.
//! - `GET /api/memory` → `{enabled, workspaces:[{id,kind,name,
//!   status:{entryCount,…}}]}`.
//!
//! Device actuation (`PATCH /api/house/devices/{id}`) is deliberately NOT
//! wired: it moves blinds/cameras in the user's home. Refresh/display only.
//! Views are generic over host traits like the transcript/canvas families.

use rune::{col, row, text, El};

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

/// Host access for the house board.
pub trait HouseHost {
    fn house_mut(&mut self) -> &mut HouseState;
}

/// Host access for the memory board.
pub trait MemoryHost {
    fn memory_mut(&mut self) -> &mut MemoryState;
}

/// One house device row: id, display name, room, compact state line.
#[derive(Debug, Clone, PartialEq)]
pub struct HouseDevice {
    pub id: String,
    pub name: String,
    pub room_id: String,
    pub dtype: String,
    pub state_line: String,
    pub online: bool,
}

impl HouseDevice {
    pub fn from_json(v: &serde_json::Value) -> Self {
        let str_of = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let state = v.get("state").cloned().unwrap_or_default();
        // Compact one-line summary covering `DeviceState` (`house.types.ts`):
        // power/position first, then locks, climate, sensors, key count.
        let state_line = state
            .get("on")
            .and_then(|x| x.as_bool())
            .map(|on| {
                if on {
                    "on".to_string()
                } else {
                    "off".to_string()
                }
            })
            .or_else(|| {
                state.get("power").and_then(|x| x.as_bool()).map(|on| {
                    if on {
                        "on".to_string()
                    } else {
                        "off".to_string()
                    }
                })
            })
            .or_else(|| {
                state
                    .get("brightness")
                    .and_then(|x| x.as_f64())
                    .map(|b| format!("{}%", b.round() as i64))
            })
            .or_else(|| {
                state
                    .get("position")
                    .and_then(|x| x.as_f64())
                    .map(|p| format!("{}%", (p * 100.0).round() as i64))
            })
            .or_else(|| {
                state.get("locked").and_then(|x| x.as_bool()).map(|l| {
                    if l {
                        "locked".to_string()
                    } else {
                        "unlocked".to_string()
                    }
                })
            })
            .or_else(|| {
                state
                    .get("targetTemp")
                    .and_then(|x| x.as_f64())
                    .map(|t| format!("{t}°"))
            })
            .or_else(|| {
                state
                    .get("temperature")
                    .and_then(|x| x.as_f64())
                    .map(|t| format!("{t}°"))
            })
            .or_else(|| {
                state.get("recording").and_then(|x| x.as_bool()).map(|r| {
                    if r {
                        "rec".to_string()
                    } else {
                        "idle".to_string()
                    }
                })
            })
            .or_else(|| {
                state.get("motion").and_then(|x| x.as_bool()).map(|m| {
                    if m {
                        "motion".to_string()
                    } else {
                        "clear".to_string()
                    }
                })
            })
            .or_else(|| {
                state.as_object().map(|o| {
                    if o.is_empty() {
                        "—".to_string()
                    } else {
                        format!("{} keys", o.len())
                    }
                })
            })
            .unwrap_or_else(|| "—".to_string());
        Self {
            id: str_of("id"),
            name: str_of("name"),
            room_id: str_of("roomId"),
            dtype: str_of("type"),
            state_line,
            online: v.get("isOnline").and_then(|x| x.as_bool()).unwrap_or(false),
        }
    }
}

/// Parsed `/api/house/state` subset the board renders.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HouseSnapshot {
    pub rooms: Vec<(String, String)>,
    pub devices: Vec<HouseDevice>,
}

impl HouseSnapshot {
    pub fn from_json(v: &serde_json::Value) -> Self {
        let rooms = v
            .get("rooms")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .map(|r| {
                        (
                            r.get("id")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string(),
                            r.get("name")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let devices = v
            .get("devices")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().map(HouseDevice::from_json).collect())
            .unwrap_or_default();
        Self { rooms, devices }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn fetch(host: &str, port: u16) -> Result<Self, String> {
        let v = crate::transport::get_json(host, port, "/api/house/state")?;
        Ok(Self::from_json(&v))
    }
}

pub struct HouseState {
    pub host: String,
    pub port: u16,
    pub rooms: Vec<(String, String)>,
    pub devices: Vec<HouseDevice>,
    pub loading: bool,
    pub error: Option<String>,
    pub last_ok: f64,
    pub auto: bool,
    pub inbox: Option<std::sync::mpsc::Receiver<Result<HouseSnapshot, String>>>,
}

impl std::fmt::Debug for HouseState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HouseState")
            .field("rooms", &self.rooms.len())
            .field("devices", &self.devices.len())
            .field("loading", &self.loading)
            .field("error", &self.error)
            .finish()
    }
}

impl Clone for HouseState {
    fn clone(&self) -> Self {
        Self {
            host: self.host.clone(),
            port: self.port,
            rooms: self.rooms.clone(),
            devices: self.devices.clone(),
            loading: self.loading,
            error: self.error.clone(),
            last_ok: self.last_ok,
            auto: self.auto,
            inbox: None,
        }
    }
}

impl Default for HouseState {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8124,
            rooms: Vec::new(),
            devices: Vec::new(),
            loading: false,
            error: None,
            last_ok: 0.0,
            auto: true,
            inbox: None,
        }
    }
}

/// One memory workspace row.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MemoryWs {
    pub id: String,
    pub name: String,
    pub entries: i64,
}

impl MemoryWs {
    pub fn from_json(v: &serde_json::Value) -> Self {
        let status = v.get("status").cloned().unwrap_or_default();
        Self {
            id: v
                .get("id")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            name: v
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            entries: status
                .get("entryCount")
                .and_then(|x| x.as_i64())
                .unwrap_or(0),
        }
    }
}

/// One memory record: the request plus its consolidated summary.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MemoryRecord {
    pub id: String,
    pub request: String,
    pub summary: String,
    pub created: String,
}

impl MemoryRecord {
    pub fn from_json(v: &serde_json::Value) -> Self {
        let str_of = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        Self {
            id: str_of("id"),
            request: str_of("request"),
            summary: str_of("summary"),
            created: str_of("createdAt"),
        }
    }
}

/// Workspace detail (`GET /api/memory/{id}?records=`): long-form summary
/// plus the newest records. Topics/entries tables stay server-side — the
/// records are what the workspace view actually reads.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MemoryDetail {
    pub summary: String,
    pub records: Vec<MemoryRecord>,
}

impl MemoryDetail {
    pub fn from_json(v: &serde_json::Value) -> Self {
        Self {
            summary: v
                .get("summary")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            records: v
                .get("records")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().map(MemoryRecord::from_json).collect())
                .unwrap_or_default(),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn fetch(host: &str, port: u16, workspace_id: &str) -> Result<Self, String> {
        let v = crate::transport::get_json(
            host,
            port,
            &format!("/api/memory/{workspace_id}?records=20"),
        )?;
        Ok(Self::from_json(&v))
    }
}

pub struct MemoryState {
    pub host: String,
    pub port: u16,
    pub workspaces: Vec<MemoryWs>,
    pub loading: bool,
    pub error: Option<String>,
    pub last_ok: f64,
    pub auto: bool,
    pub inbox: Option<std::sync::mpsc::Receiver<Result<Vec<MemoryWs>, String>>>,
    /// Selected workspace id (`MemoryWorkspace` active tab); its detail
    /// renders below the list.
    pub selected: String,
    /// Detail for `selected` (`GET /api/memory/{id}?records=`).
    pub detail: Option<MemoryDetail>,
    pub detail_loading: bool,
    pub detail_error: Option<String>,
    pub detail_inbox: Option<std::sync::mpsc::Receiver<Result<MemoryDetail, String>>>,
}

impl std::fmt::Debug for MemoryState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryState")
            .field("workspaces", &self.workspaces.len())
            .field("loading", &self.loading)
            .field("error", &self.error)
            .finish()
    }
}

impl Clone for MemoryState {
    fn clone(&self) -> Self {
        Self {
            host: self.host.clone(),
            port: self.port,
            workspaces: self.workspaces.clone(),
            loading: self.loading,
            error: self.error.clone(),
            last_ok: self.last_ok,
            auto: self.auto,
            inbox: None,
            selected: self.selected.clone(),
            detail: self.detail.clone(),
            detail_loading: self.detail_loading,
            detail_error: self.detail_error.clone(),
            detail_inbox: None,
        }
    }
}

impl Default for MemoryState {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8124,
            workspaces: Vec::new(),
            loading: false,
            error: None,
            last_ok: 0.0,
            auto: true,
            inbox: None,
            selected: String::new(),
            detail: None,
            detail_loading: false,
            detail_error: None,
            detail_inbox: None,
        }
    }
}

impl MemoryState {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn fetch(host: &str, port: u16) -> Result<Vec<MemoryWs>, String> {
        let v = crate::transport::get_json(host, port, "/api/memory")?;
        Ok(v.get("workspaces")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().map(MemoryWs::from_json).collect())
            .unwrap_or_default())
    }
}

/// Drain a worker inbox into a vec; reports whether the sender is gone.
pub fn drain_results<T>(
    inbox: &mut Option<std::sync::mpsc::Receiver<Result<T, String>>>,
) -> (Vec<Result<T, String>>, bool) {
    let mut out = Vec::new();
    while let Some(msg) = inbox.as_ref().and_then(|rx| rx.try_recv().ok()) {
        out.push(msg);
    }
    let dead = inbox.as_ref().is_some_and(|rx| {
        matches!(
            rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        )
    });
    if dead {
        *inbox = None;
    }
    (out, dead)
}

/// Spawn worker refreshes. No-op while one is in flight.
pub fn refresh_house(s: &mut HouseState) {
    if s.loading {
        return;
    }
    s.loading = true;
    s.error = None;
    let (tx, rx) = std::sync::mpsc::channel();
    s.inbox = Some(rx);
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let result = match crate::transport::fetch_json("/api/house/state").await {
            Ok(v) => Ok(HouseSnapshot::from_json(&v)),
            Err(e) => Err(e),
        };
        let _ = tx.send(result);
    });
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (host, port) = (s.host.clone(), s.port);
        std::thread::spawn(move || {
            let _ = tx.send(HouseSnapshot::fetch(&host, port));
        });
    }
}

/// Spawn worker refreshes. No-op while one is in flight.
pub fn refresh_memory(s: &mut MemoryState) {
    if s.loading {
        return;
    }
    s.loading = true;
    s.error = None;
    let (tx, rx) = std::sync::mpsc::channel();
    s.inbox = Some(rx);
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let result = match crate::transport::fetch_json("/api/memory").await {
            Ok(v) => Ok(v
                .get("workspaces")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().map(MemoryWs::from_json).collect())
                .unwrap_or_default()),
            Err(e) => Err(e),
        };
        let _ = tx.send(result);
    });
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (host, port) = (s.host.clone(), s.port);
        std::thread::spawn(move || {
            let _ = tx.send(MemoryState::fetch(&host, port));
        });
    }
}

/// Select a workspace and load its detail. Tapping the active row again
/// collapses it (React's tab toggle).
pub fn select_memory(s: &mut MemoryState, id: &str) {
    if s.selected == id {
        s.selected.clear();
        s.detail = None;
        s.detail_error = None;
        return;
    }
    s.selected = id.to_string();
    s.detail = None;
    s.detail_error = None;
    refresh_memory_detail(s);
}

/// Detail worker (`GET /api/memory/{id}?records=20`). No-op while one is in
/// flight; a second tap collapses via [`select_memory`] instead of stacking.
pub fn refresh_memory_detail(s: &mut MemoryState) {
    if s.detail_loading || s.selected.trim().is_empty() {
        return;
    }
    s.detail_loading = true;
    s.detail_error = None;
    let (tx, rx) = std::sync::mpsc::channel();
    s.detail_inbox = Some(rx);
    let ws = s.selected.clone();
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let path = format!("/api/memory/{ws}?records=20");
        let result = match crate::transport::fetch_json(&path).await {
            Ok(v) => Ok(MemoryDetail::from_json(&v)),
            Err(e) => Err(e),
        };
        let _ = tx.send(result);
    });
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (host, port) = (s.host.clone(), s.port);
        std::thread::spawn(move || {
            let _ = tx.send(MemoryDetail::fetch(&host, port, &ws));
        });
    }
}

/// Drain the detail inbox. Shared by every host like the list polls.
pub fn poll_memory_detail(s: &mut MemoryState) -> bool {
    let (msgs, _) = drain_results(&mut s.detail_inbox);
    let mut changed = false;
    for msg in msgs {
        match msg {
            Ok(detail) => {
                s.detail = Some(detail);
                s.detail_error = None;
            }
            Err(message) => {
                s.detail_error = Some(message);
            }
        }
        s.detail_loading = false;
        changed = true;
    }
    changed
}

fn refresh_button<S: 'static>(key: &str, loading: bool, on_press: impl Fn(&mut S) + 'static) -> El {
    text(if loading { "…" } else { "Refresh" }.to_string())
        .key(key.to_string())
        .role("button")
        .focusable(true)
        .font(rune::tok::TextStyle::new(11.0, 600.0))
        .px(12.0)
        .py(8.0)
        .radius(8.0)
        .fill(ink("#ffffff0d"))
        .color(ink("#d4d4d8"))
        .on_press(on_press)
}

/// House board: room sections with device rows (name, state, online dot).
pub fn house_view<S: HouseHost + 'static>(s: &HouseState) -> El {
    let mut body = col()
        .key("house-board")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(ink("#0a0a0a"))
        .px(24.0)
        .pt(18.0)
        .gap(14.0)
        .child(
            row()
                .key("house-head")
                .w_full()
                .items_center()
                .child(
                    text("House".to_string())
                        .key("house-title")
                        .grow(1.0)
                        .font(rune::tok::TextStyle::new(15.0, 650.0))
                        .color(ink("#f2f2f5")),
                )
                .child(refresh_button("house-refresh", s.loading, |s: &mut S| {
                    refresh_house(s.house_mut());
                })),
        );
    if s.devices.is_empty() && !s.loading {
        body = body.child(
            text("no devices reported yet".to_string())
                .key("house-empty")
                .font(rune::tok::TextStyle::new(12.0, 400.0))
                .color(ink("#52525b")),
        );
    }
    // Group devices under their rooms, in room order; unhomed trailing.
    let mut ordered_rooms = s.rooms.clone();
    for d in &s.devices {
        if !ordered_rooms.iter().any(|(id, _)| id == &d.room_id) {
            ordered_rooms.push((d.room_id.clone(), d.room_id.clone()));
        }
    }
    for (room_id, room_name) in &ordered_rooms {
        let members: Vec<&HouseDevice> =
            s.devices.iter().filter(|d| &d.room_id == room_id).collect();
        if members.is_empty() {
            continue;
        }
        let mut section = col()
            .key(format!("house-room-{room_id}"))
            .w_full()
            .gap(4.0)
            .child(
                text(room_name.clone())
                    .key(format!("house-room-{room_id}-name"))
                    .font(rune::tok::TextStyle::new(11.0, 650.0))
                    .color(ink("#71717a")),
            );
        for d in members {
            section = section.child(
                row()
                    .key(format!("house-dev-{id}", id = d.id))
                    .w_full()
                    .items_center()
                    .gap(10.0)
                    .radius(10.0)
                    .fill(ink("#141417"))
                    .border(1.0, ink("#ffffff14"))
                    .px(12.0)
                    .py(9.0)
                    .child(
                        rune::el()
                            .key(format!("house-dev-{id}-dot", id = d.id))
                            .size(7.0, 7.0)
                            .radius(4.0)
                            .fill(ink(if d.online { "#7ee0a8" } else { "#52525b" })),
                    )
                    .child(
                        col()
                            .grow(1.0)
                            .min_h(0.0)
                            .child(
                                text(d.name.clone())
                                    .key(format!("house-dev-{id}-name", id = d.id))
                                    .font(rune::tok::TextStyle::new(12.0, 600.0))
                                    .color(ink("#e4e4e7")),
                            )
                            .child(
                                text(d.dtype.clone())
                                    .key(format!("house-dev-{id}-type", id = d.id))
                                    .font(rune::tok::TextStyle::new(10.0, 400.0))
                                    .color(ink("#71717a")),
                            ),
                    )
                    .child(
                        text(d.state_line.clone())
                            .key(format!("house-dev-{id}-state", id = d.id))
                            .font(rune::tok::TextStyle::new(11.0, 500.0))
                            .color(ink("#c9c9d1")),
                    ),
            );
        }
        body = body.child(section);
    }
    if let Some(err) = &s.error {
        body = body.child(
            text(err.clone())
                .key("house-error")
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .color(ink("#fda4af")),
        );
    }
    body
}

/// Memory board: workspace rows with entry counts.
pub fn memory_view<S: MemoryHost + 'static>(s: &MemoryState) -> El {
    let mut body = col()
        .key("memory-board")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(ink("#0a0a0a"))
        .px(24.0)
        .pt(18.0)
        .gap(10.0)
        .child(
            row()
                .key("memory-head")
                .w_full()
                .items_center()
                .child(
                    text("Memory".to_string())
                        .key("memory-title")
                        .grow(1.0)
                        .font(rune::tok::TextStyle::new(15.0, 650.0))
                        .color(ink("#f2f2f5")),
                )
                .child(refresh_button("memory-refresh", s.loading, |s: &mut S| {
                    refresh_memory(s.memory_mut());
                })),
        );
    if s.workspaces.is_empty() && !s.loading {
        body = body.child(
            text("no workspaces indexed yet".to_string())
                .key("memory-empty")
                .font(rune::tok::TextStyle::new(12.0, 400.0))
                .color(ink("#52525b")),
        );
    }
    for ws in &s.workspaces {
        let id = ws.id.clone();
        let selected = ws.id == s.selected;
        let mut item = row()
            .key(format!("memory-ws-{id}", id = ws.id))
            .w_full()
            .items_center()
            .gap(10.0)
            .radius(10.0)
            .fill(ink(if selected { "#1b1b20" } else { "#141417" }))
            .border(1.0, ink(if selected { "#ffffff30" } else { "#ffffff14" }))
            .px(12.0)
            .py(9.0)
            .role("button")
            .focusable(true)
            .label(format!("Open {name} memory", name = ws.name))
            .on_press(move |s: &mut S| {
                select_memory(s.memory_mut(), &id);
            })
            .child(
                text(ws.name.clone())
                    .key(format!("memory-ws-{id}-name", id = ws.id))
                    .grow(1.0)
                    .font(rune::tok::TextStyle::new(12.0, 600.0))
                    .color(ink("#e4e4e7")),
            )
            .child(
                text(format!("{} entries", ws.entries))
                    .key(format!("memory-ws-{id}-count", id = ws.id))
                    .font(rune::tok::TextStyle::new(11.0, 400.0))
                    .color(ink("#8a8a93")),
            );
        if selected {
            item = item.child(
                text(if s.detail_loading { "…" } else { "▾" }.to_string())
                    .key(format!("memory-ws-{id}-chev", id = ws.id))
                    .font(rune::tok::TextStyle::new(11.0, 600.0))
                    .color(ink("#71717a")),
            );
        }
        body = body.child(item);
    }
    body = memory_detail_view(body, s);
    if let Some(err) = &s.error {
        body = body.child(
            text(err.clone())
                .key("memory-error")
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .color(ink("#fda4af")),
        );
    }
    body
}

/// Detail panel for the selected workspace: long-form summary plus newest
/// records (`MemoryWorkspace` detail half). Read-only: summary edits and
/// consolidation stay behind explicit buttons server-side for now.
fn memory_detail_view(mut body: El, s: &MemoryState) -> El {
    if s.selected.trim().is_empty() {
        return body;
    }
    body = body.child(
        row()
            .key("memory-detail-head")
            .w_full()
            .items_center()
            .gap(8.0)
            .child(
                text("Detail".to_string())
                    .key("memory-detail-title")
                    .grow(1.0)
                    .font(rune::tok::TextStyle::new(12.0, 600.0))
                    .color(ink("#a1a1aa")),
            )
            .child(
                text(if s.detail_loading {
                    "…".to_string()
                } else {
                    format!(
                        "{} records",
                        s.detail.as_ref().map(|d| d.records.len()).unwrap_or(0)
                    )
                })
                .key("memory-detail-count")
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .color(ink("#71717a")),
            ),
    );
    if let Some(detail) = &s.detail {
        let summary = if detail.summary.trim().is_empty() {
            "No long-form summary yet.".to_string()
        } else {
            detail.summary.clone()
        };
        body = body.child(
            text(summary)
                .key("memory-detail-summary")
                .font(rune::tok::TextStyle::new(12.0, 400.0))
                .wrap()
                .color(ink("#c9c9d1")),
        );
        if detail.records.is_empty() && !s.detail_loading {
            body = body.child(
                text("no records yet".to_string())
                    .key("memory-detail-empty")
                    .font(rune::tok::TextStyle::new(11.0, 400.0))
                    .color(ink("#52525b")),
            );
        }
        for r in &detail.records {
            body = body.child(
                col()
                    .key(format!("memory-record-{id}", id = r.id))
                    .w_full()
                    .gap(4.0)
                    .radius(10.0)
                    .fill(ink("#141417"))
                    .border(1.0, ink("#ffffff14"))
                    .px(12.0)
                    .py(9.0)
                    .child(
                        row()
                            .key(format!("memory-record-{id}-head", id = r.id))
                            .w_full()
                            .items_center()
                            .gap(8.0)
                            .child(
                                text(r.request.clone())
                                    .key(format!("memory-record-{id}-request", id = r.id))
                                    .grow(1.0)
                                    .font(rune::tok::TextStyle::new(12.0, 600.0))
                                    .color(ink("#e4e4e7")),
                            )
                            .child(
                                text(r.created.clone())
                                    .key(format!("memory-record-{id}-created", id = r.id))
                                    .font(rune::tok::TextStyle::new(10.0, 400.0))
                                    .color(ink("#71717a")),
                            ),
                    )
                    .child(
                        text(r.summary.clone())
                            .key(format!("memory-record-{id}-summary", id = r.id))
                            .font(rune::tok::TextStyle::new(11.5, 400.0))
                            .wrap()
                            .color(ink("#a1a1aa")),
                    ),
            );
        }
    }
    if let Some(err) = &s.detail_error {
        body = body.child(
            text(err.clone())
                .key("memory-detail-error")
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
    fn house_snapshot_parses_live_shape() {
        let v = serde_json::json!({
            "rooms": [{"id": "living", "name": "Living Room"}],
            "devices": [
                {"id": "blind-living", "name": "Window Blinds", "roomId": "living",
                 "type": "blinds", "state": {"position": 0.5}, "capabilities": ["position"],
                 "isOnline": true},
                {"id": "cam-garage", "name": "Garage Camera", "roomId": "garage",
                 "type": "camera", "state": {"recording": true}, "capabilities": [],
                 "isOnline": false},
            ],
        });
        let snap = HouseSnapshot::from_json(&v);
        assert_eq!(
            snap.rooms,
            vec![("living".to_string(), "Living Room".to_string())]
        );
        assert_eq!(snap.devices.len(), 2);
        assert_eq!(snap.devices[0].state_line, "50%");
        assert!(snap.devices[0].online);
        assert_eq!(snap.devices[1].state_line, "rec");
        assert!(!snap.devices[1].online);
    }

    #[test]
    fn memory_workspaces_parse_live_shape() {
        let v = serde_json::json!({
            "workspaces": [
                {"id": "_general", "name": "General",
                 "status": {"entryCount": 12}},
                {"id": "_house", "name": "House", "status": {}},
            ],
        });
        let s = MemoryState {
            workspaces: v["workspaces"]
                .as_array()
                .unwrap()
                .iter()
                .map(MemoryWs::from_json)
                .collect(),
            ..MemoryState::default()
        };
        assert_eq!(s.workspaces[0].entries, 12);
        assert_eq!(s.workspaces[1].entries, 0);
    }

    #[test]
    fn memory_detail_parses_records_and_summary() {
        let v = serde_json::json!({
            "summary": "Likes terse replies.",
            "records": [
                {"id": "r1", "request": "Summarize this", "summary": "Did it",
                 "createdAt": "2026-11-01"},
                {"id": "r2", "request": "Second", "summary": "", "createdAt": ""},
            ],
        });
        let d = MemoryDetail::from_json(&v);
        assert_eq!(d.summary, "Likes terse replies.");
        assert_eq!(d.records.len(), 2);
        assert_eq!(d.records[0].request, "Summarize this");
        assert_eq!(d.records[1].summary, "");
        // Missing keys default empty, never null-panic.
        let empty = MemoryDetail::from_json(&serde_json::json!({}));
        assert_eq!(empty, MemoryDetail::default());
    }

    #[test]
    fn select_memory_toggles_and_collapses() {
        let mut s = MemoryState::default();
        select_memory(&mut s, "ws1");
        assert_eq!(s.selected, "ws1");
        // Worker spawned (or failed instantly with no server — either way the
        // loading flag was set and no detail landed).
        select_memory(&mut s, "ws1");
        assert!(s.selected.is_empty(), "second tap collapses");
        assert!(s.detail.is_none());
    }
}
