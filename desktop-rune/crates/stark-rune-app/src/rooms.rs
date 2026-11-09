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

pub struct MemoryState {
    pub host: String,
    pub port: u16,
    pub workspaces: Vec<MemoryWs>,
    pub loading: bool,
    pub error: Option<String>,
    pub last_ok: f64,
    pub auto: bool,
    pub inbox: Option<std::sync::mpsc::Receiver<Result<Vec<MemoryWs>, String>>>,
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

fn refresh_button<S: 'static>(key: &str, loading: bool, on_press: impl Fn(&mut S) + 'static) -> El {
    text(if loading { "…" } else { "Refresh" }.to_string())
        .key(key.to_string())
        .role("button")
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
        body = body.child(
            row()
                .key(format!("memory-ws-{id}", id = ws.id))
                .w_full()
                .items_center()
                .gap(10.0)
                .radius(10.0)
                .fill(ink("#141417"))
                .border(1.0, ink("#ffffff14"))
                .px(12.0)
                .py(9.0)
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
                ),
        );
    }
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
}
