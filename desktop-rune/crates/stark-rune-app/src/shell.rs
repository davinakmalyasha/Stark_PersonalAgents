//! Unified shell — transcript and canvas map under one `AppSpec` with route tabs.
//!
//! Mirrors `frontend/src/AppShell.tsx` hash routes (`home` map + `coding`
//! transcript): one window, one state, tab bar switching. Sub-views stay
//! mounted per route (no cross-route element keys), and both states survive
//! switching — a running stream keeps draining while the map is up.

use rune::{col, row, text, AppSpec, Cx, El};

use crate::rune_canvas::{canvas_body, CanvasHost, CanvasState};
use crate::rune_transcript::{
    drain_run_inbox, transcript_body, RuneTranscriptState, Thread, TranscriptHost,
};

/// Shell routes. `Map` is the home canvas; `Chat` the main transcript;
/// `Personal` the personal room; `Cloud` the runs status board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Route {
    #[default]
    Map,
    Chat,
    Personal,
    Cloud,
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

    pub fn fetch(host: &str, port: u16) -> Result<Self, String> {
        let v = crate::transport::get_json(host, port, "/api/runs/active")?;
        Ok(Self::from_json(&v))
    }
}

pub struct CloudState {
    pub host: String,
    pub port: u16,
    pub active: Option<CloudActive>,
    pub loading: bool,
    pub error: Option<String>,
    pub last_ok: f64,
    pub auto: bool,
    pub inbox: Option<std::sync::mpsc::Receiver<Result<CloudActive, String>>>,
}

impl std::fmt::Debug for CloudState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudState")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("active", &self.active)
            .field("loading", &self.loading)
            .field("error", &self.error)
            .field("last_ok", &self.last_ok)
            .field("auto", &self.auto)
            .field("inbox", &self.inbox.is_some())
            .finish()
    }
}

impl Clone for CloudState {
    fn clone(&self) -> Self {
        Self {
            host: self.host.clone(),
            port: self.port,
            active: self.active.clone(),
            loading: self.loading,
            error: self.error.clone(),
            last_ok: self.last_ok,
            auto: self.auto,
            inbox: None,
        }
    }
}

impl Default for CloudState {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8124,
            active: None,
            loading: false,
            error: None,
            last_ok: 0.0,
            auto: true,
            inbox: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ShellState {
    pub route: Route,
    pub transcript: RuneTranscriptState,
    pub personal: RuneTranscriptState,
    pub canvas: CanvasState,
    pub cloud: CloudState,
}

impl Default for ShellState {
    fn default() -> Self {
        let mut personal = RuneTranscriptState::screenshot();
        personal.title = "Personal".to_string();
        personal.rows = vec![];
        personal.live = false;
        Self {
            route: Route::Map,
            transcript: RuneTranscriptState::screenshot(),
            personal,
            canvas: CanvasState::map(),
            cloud: CloudState::default(),
        }
    }
}

impl TranscriptHost for ShellState {
    fn thread(&self, which: Thread) -> &RuneTranscriptState {
        match which {
            Thread::Main => &self.transcript,
            Thread::Personal => &self.personal,
        }
    }
    fn thread_mut(&mut self, which: Thread) -> &mut RuneTranscriptState {
        match which {
            Thread::Main => &mut self.transcript,
            Thread::Personal => &mut self.personal,
        }
    }
}

impl CanvasHost for ShellState {
    fn canvas_mut(&mut self) -> &mut CanvasState {
        &mut self.canvas
    }
}

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

fn tab(s: &ShellState, route: Route, label: &str, key: &str) -> El {
    let active = s.route == route;
    let mut el = text(label.to_string())
        .key(key.to_string())
        .role("tab")
        .font(rune::tok::TextStyle::new(12.0, 600.0))
        .px(14.0)
        .py(8.0)
        .radius(8.0)
        .color(ink(if active { "#f4f4f5" } else { "#71717a" }))
        .on_press(move |s: &mut ShellState| {
            s.route = route;
        });
    if active {
        el = el.fill(rune::Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.08,
        });
    }
    el
}

pub fn shell_view(s: &ShellState, _: &mut Cx<ShellState>) -> El {
    let body = match s.route {
        Route::Map => canvas_body::<ShellState>(&s.canvas),
        Route::Chat => transcript_body::<ShellState>(&s.transcript, Thread::Main),
        Route::Personal => transcript_body::<ShellState>(&s.personal, Thread::Personal),
        Route::Cloud => cloud_view(&s.cloud),
    };
    col()
        .key("stark-shell")
        .w_full()
        .h_full()
        .fill(ink("#0a0a0a"))
        .child(
            row()
                .key("shell-tabs")
                .w_full()
                .items_center()
                .gap(4.0)
                .px(12.0)
                .pt(10.0)
                .pb(6.0)
                .child(tab(s, Route::Map, "Map", "tab-map"))
                .child(tab(s, Route::Chat, "Chat", "tab-chat"))
                .child(tab(s, Route::Personal, "Personal", "tab-personal"))
                .child(tab(s, Route::Cloud, "Cloud", "tab-cloud")),
        )
        .child(body)
}

/// Cloud runs board: the live `/api/runs/active` status plus a refresh that
/// re-fetches on a worker thread. Mirrors `CloudRuns.tsx` refresh semantics
/// (manual refresh + 10s auto poll, keep-last-on-failure).
fn cloud_view(s: &CloudState) -> El {
    let status = match &s.active {
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
        .px(24.0)
        .pt(18.0)
        .gap(10.0)
        .child(
            text("Cloud runs".to_string())
                .key("cloud-title")
                .font(rune::tok::TextStyle::new(15.0, 650.0))
                .color(ink("#f2f2f5")),
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
                        .fill(ink(if status == "Running" {
                            "#7ee0a8"
                        } else {
                            "#52525b"
                        })),
                )
                .child(
                    col()
                        .grow(1.0)
                        .min_h(0.0)
                        .child(
                            text(status.to_string())
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
                )
                .child(
                    text(if s.loading { "…" } else { "Refresh" }.to_string())
                        .key("cloud-refresh")
                        .role("button")
                        .label("Refresh cloud status")
                        .font(rune::tok::TextStyle::new(11.0, 600.0))
                        .px(12.0)
                        .py(8.0)
                        .radius(8.0)
                        .fill(ink("#ffffff0d"))
                        .color(ink("#d4d4d8"))
                        .on_press(|s: &mut ShellState| {
                            refresh_cloud(s);
                        }),
                ),
        );
    if let Some(err) = &s.error {
        body = body.child(
            text(err.clone())
                .key("cloud-error")
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .color(ink("#fda4af")),
        );
    }
    body
}

/// Spawn a worker-thread refresh of `/api/runs/active`. No-op while one is in
/// flight; failures surface as an error row, never a stuck spinner.
pub fn refresh_cloud(s: &mut ShellState) {
    if s.cloud.loading {
        return;
    }
    s.cloud.loading = true;
    s.cloud.error = None;
    let (tx, rx) = std::sync::mpsc::channel();
    s.cloud.inbox = Some(rx);
    let (host, port) = (s.cloud.host.clone(), s.cloud.port);
    std::thread::spawn(move || {
        let _ = tx.send(CloudActive::fetch(&host, port));
    });
}

/// `poll` drains transcript inboxes whatever route is up and refreshes Cloud
/// at most every 10s, so runs started in Chat keep streaming on the Map.
pub fn shell_poll(s: &mut ShellState, clock: f64, tx: &mut rune::Tx) -> bool {
    let mut changed = drain_run_inbox(&mut s.transcript, clock, tx);
    changed |= drain_run_inbox(&mut s.personal, clock, tx);
    // Cloud inbox first (a refresh may have landed), then auto-refresh.
    let mut seen = false;
    while let Some(msg) = s.cloud.inbox.as_ref().and_then(|rx| rx.try_recv().ok()) {
        match msg {
            Ok(active) => {
                s.cloud.active = Some(active);
                s.cloud.error = None;
                s.cloud.last_ok = clock;
            }
            Err(message) => {
                s.cloud.error = Some(message);
            }
        }
        s.cloud.loading = false;
        seen = true;
    }
    if seen {
        let dead = s.cloud.inbox.as_ref().is_some_and(|rx| {
            matches!(
                rx.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Disconnected)
            )
        });
        if dead {
            s.cloud.inbox = None;
        }
        changed = true;
    }
    if s.cloud.auto && !s.cloud.loading && clock - s.cloud.last_ok > 10.0 {
        refresh_cloud(s);
        changed = true;
    }
    changed
}

fn init_shell() -> ShellState {
    ShellState::default()
}

fn apply_map(s: &mut ShellState) {
    s.route = Route::Map;
}

fn apply_chat(s: &mut ShellState) {
    s.route = Route::Chat;
}

fn apply_personal(s: &mut ShellState) {
    s.route = Route::Personal;
}

fn apply_cloud(s: &mut ShellState) {
    s.route = Route::Cloud;
}

pub fn shell_spec() -> AppSpec<ShellState> {
    let mut spec = AppSpec::new("Stark", (1280.0, 900.0), init_shell, shell_view);
    spec.scenes = &[
        ("map", apply_map),
        ("chat", apply_chat),
        ("personal", apply_personal),
        ("cloud", apply_cloud),
    ];
    spec.poll = Some(shell_poll);
    spec
}

pub fn run_shell_headless(extra_args: Vec<String>) {
    rune::App::new(shell_spec()).args(extra_args).run();
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn tabs_switch_routes_and_preserve_state() {
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(ShellState::default(), shell_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        // Map first: canvas keys present, transcript rows absent.
        assert!(rt.find("canvas-world").is_some());
        assert!(rt.find("transcript-viewport").is_none());
        // Type in Chat without leaving Map? Switch first, then type.
        let (x, y) = rt.find("tab-chat").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert_eq!(rt.state.route, Route::Chat);
        assert!(rt.find("transcript-viewport").is_some());
        assert!(rt.find("canvas-world").is_none());
        // Map state survives the round trip.
        let (x, y) = rt.find("tab-map").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert_eq!(rt.state.route, Route::Map);
        assert!(rt.find("chat:ws-general:chat-1").is_some());
    }

    #[test]
    fn personal_room_has_own_thread() {
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(ShellState::default(), shell_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        let (x, y) = rt.find("tab-personal").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert_eq!(rt.state.route, Route::Personal);
        assert!(rt.find("transcript-viewport").is_some());
        // Main thread rows are not mounted in the personal room.
        assert!(rt.find("msg-0").is_none());
        // Sending in personal stages only there.
        let main_rows = rt.state.transcript.rows.len();
        rt.state.personal.api_port = 1;
        rt.state.personal.edit.text = "personal hi".to_string();
        crate::rune_transcript::stage_send(&mut rt.state.personal);
        assert_eq!(rt.state.personal.rows.len(), 1);
        assert_eq!(rt.state.transcript.rows.len(), main_rows);
    }

    #[test]
    fn cloud_refresh_loads_active_run() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let mut raw = Vec::new();
            loop {
                let n = sock.read(&mut buf).unwrap();
                raw.extend_from_slice(&buf[..n]);
                if raw.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let body = r#"{"running":true,"workspace_path":"D:/proj","chat_id":"c9","started_at":"2026-11-09T00:00:00Z"}"#;
            sock.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .unwrap();
        });
        let env = Env::new(true, 1.0);
        let mut state = ShellState::default();
        state.cloud.host = "127.0.0.1".to_string();
        state.cloud.port = port;
        state.cloud.auto = false;
        state.route = Route::Cloud;
        let mut rt = Runtime::new(state, shell_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        assert!(rt.find("cloud-board").is_some());
        let (x, y) = rt.find("cloud-refresh").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert!(rt.state.cloud.loading);
        // Poll until the worker thread's message lands (local mock: ms).
        let mut run_tx = rune::Tx::default();
        let mut waited = 0;
        while rt.state.cloud.active.is_none() && waited < 500 {
            shell_poll(&mut rt.state, 0.0, &mut run_tx);
            if rt.state.cloud.active.is_none() {
                std::thread::sleep(std::time::Duration::from_millis(10));
                waited += 1;
            }
        }
        let active = rt.state.cloud.active.clone().expect("active run stored");
        assert!(active.running);
        assert_eq!(active.workspace_path, "D:/proj");
        assert!(!rt.state.cloud.loading);
    }

    #[test]
    fn shell_send_and_poll_stream_while_on_map() {
        use crate::stream::SseFrame;
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(ShellState::default(), shell_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        // Stage a run from Chat, flip to Map, drain via shell poll.
        let (x, y) = rt.find("tab-chat").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        rt.state.transcript.api_port = 1;
        rt.state.transcript.edit.text = "hi".to_string();
        crate::rune_transcript::stage_send(&mut rt.state.transcript);
        assert!(rt.state.transcript.live);
        let (x, y) = rt.find("tab-map").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        // Simulate the run thread delivering frames; poll applies them on Map.
        let tx = rt.state.transcript.inbox.take();
        let (f_tx, f_rx) = std::sync::mpsc::channel();
        rt.state.transcript.inbox = Some(f_rx);
        f_tx.send(SseFrame {
            event: "delta".to_string(),
            data: serde_json::json!({"kind":"text","text":"42"}),
        })
        .unwrap();
        drop(f_tx);
        drop(tx);
        let mut run_tx = rune::Tx::default();
        assert!(shell_poll(&mut rt.state, 0.0, &mut run_tx));
        assert!(rt
            .state
            .transcript
            .rows
            .iter()
            .any(|r| r.body.contains("42")));
    }
}
