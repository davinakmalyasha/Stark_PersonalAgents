//! Single-page shell — one window, chat types only, no workspace routes.
//!
//! Layout mirrors the desktop app: session tabs on top (one per open chat +
//! New session), then a row of icon rail + side panel + canvas map. The rail
//! switches the panel (Chats / Agents / Group / Cloud / Memory / House); the
//! canvas always stays mounted, so a running stream keeps draining whatever
//! panel is up. A transcript sheet opens over the map for the active session.

use rune::{col, row, text, AppSpec, Cx, El};

use crate::cloud::{cloud_view, poll_cloud_state, CloudHost, CloudState};
use crate::projects::{poll_projects_state, ProjectsHost};
use crate::rooms::{
    drain_results, house_view, memory_view, poll_memory_detail, refresh_house, refresh_memory,
    HouseHost, HouseState, MemoryHost, MemoryState,
};
use crate::rune_canvas::{chats_panel, CanvasHost, CanvasState, RailTab};
use crate::rune_transcript::{
    drain_run_inbox, transcript_body, RuneTranscriptState, Thread, TranscriptHost,
};

#[derive(Debug, Clone)]
pub struct ShellState {
    pub transcript: RuneTranscriptState,
    pub personal: RuneTranscriptState,
    pub canvas: CanvasState,
    pub cloud: CloudState,
    pub house: HouseState,
    pub memory: MemoryState,
}

impl Default for ShellState {
    fn default() -> Self {
        let mut personal = RuneTranscriptState::screenshot();
        personal.title = "Personal".to_string();
        personal.rows = vec![];
        personal.live = false;
        // Distinct thread scope (notes + abort key off it, like `chat.id`).
        personal.chat_id = crate::notes::mint_id("rune-personal");
        Self {
            transcript: RuneTranscriptState::screenshot(),
            personal,
            canvas: CanvasState::map(),
            cloud: CloudState::default(),
            house: HouseState::default(),
            memory: MemoryState::default(),
        }
    }
}

impl TranscriptHost for ShellState {
    fn thread(&self, which: Thread) -> &RuneTranscriptState {
        match which {
            Thread::Main => &self.transcript,
            // The sheet's card thread; falls back to Main when closed.
            Thread::Open => self.canvas.sheet_thread().unwrap_or(&self.transcript),
        }
    }
    fn thread_mut(&mut self, which: Thread) -> &mut RuneTranscriptState {
        match which {
            Thread::Main => &mut self.transcript,
            Thread::Open => {
                if self.canvas.sessions.sheet.is_some() && self.canvas.sheet_thread().is_some() {
                    self.canvas.sheet_thread_mut().expect("checked above")
                } else {
                    &mut self.transcript
                }
            }
        }
    }
}

impl CanvasHost for ShellState {
    fn canvas_mut(&mut self) -> &mut CanvasState {
        &mut self.canvas
    }
}

impl HouseHost for ShellState {
    fn house_mut(&mut self) -> &mut HouseState {
        &mut self.house
    }
}

impl MemoryHost for ShellState {
    fn memory_mut(&mut self) -> &mut MemoryState {
        &mut self.memory
    }
}

impl ProjectsHost for ShellState {
    fn projects_mut(&mut self) -> &mut crate::projects::ProjectsState {
        &mut self.canvas.projects
    }
}

impl crate::agents::AgentsHost for ShellState {
    fn agents_mut(&mut self) -> &mut crate::agents::AgentsState {
        &mut self.canvas.agents
    }
}

impl CloudHost for ShellState {
    fn cloud_mut(&mut self) -> &mut CloudState {
        &mut self.cloud
    }
}

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

/// One session tab: card title + close. Press selects and focuses the card;
/// double-press opens its transcript sheet (React opens the chat view).
fn session_tab(s: &ShellState, id: &str) -> El {
    let title = s
        .canvas
        .card(id)
        .map(|c| c.title.clone())
        .or_else(|| s.canvas.sessions.threads.get(id).map(|t| t.title.clone()))
        .unwrap_or_else(|| "Chat".to_string());
    let active = s.canvas.active_id == id;
    let select = id.to_string();
    let open = id.to_string();
    let close = id.to_string();
    let mut el = row()
        .key(format!("session-{id}"))
        .role("tab")
        .items_center()
        .gap(6.0)
        .px(10.0)
        .py(6.0)
        .radius(8.0)
        .color(ink(if active { "#f4f4f5" } else { "#71717a" }))
        .on_press(move |s: &mut ShellState| {
            s.canvas.select_card(&select);
        })
        .on_double_press(move |s: &mut ShellState| {
            s.canvas.open_sheet(&open);
        })
        .child(
            text(title)
                .font(rune::tok::TextStyle::new(12.0, 600.0))
                .color(ink(if active { "#f4f4f5" } else { "#a1a1aa" })),
        )
        .child(
            text("×".to_string())
                .key(format!("session-{id}-close"))
                .role("button")
                .label("Close session")
                .font(rune::tok::TextStyle::new(12.0, 400.0))
                .color(ink("#71717a"))
                .on_press(move |s: &mut ShellState| {
                    s.canvas.close_session(&close);
                }),
        );
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

/// Icon rail button (glyph + label for assistive tech).
fn rail_button(s: &ShellState, tab: RailTab) -> El {
    let active = s.canvas.rail == tab;
    let mut el = rune::el()
        .key(format!("rail-{}", tab.label()))
        .role("button")
        .label(format!("{} panel", tab.label()))
        .size(40.0, 40.0)
        .radius(11.0)
        .center()
        .on_press(move |s: &mut ShellState| {
            s.canvas.rail = tab;
        })
        .child(
            text(tab.glyph().to_string())
                .font(rune::tok::TextStyle::new(16.0, 400.0))
                .color(ink(if active { "#f4f4f5" } else { "#71717a" })),
        );
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

/// Side panel content for the active rail tab. Chat-local panels read the
/// canvas; cloud/memory/house read shell state.
fn side_panel(s: &ShellState) -> El {
    let mut panel = col()
        .key("side-panel")
        .w(crate::rune_canvas::MAP_SIDEBAR_W - crate::rune_canvas::RAIL_W)
        .h_full()
        .fill(ink("#0f0f0f"))
        .px(12.0)
        .pt(12.0)
        .gap(8.0);
    match s.canvas.rail {
        RailTab::Chats => {
            panel = panel.child(chats_panel::<ShellState>(&s.canvas));
        }
        RailTab::Agents => {
            panel = panel.child(crate::agents::agents_section::<ShellState>(
                &s.canvas.agents,
            ));
        }
        RailTab::Group => {
            panel = panel.child(crate::projects::projects_section::<ShellState>(
                &s.canvas.projects,
            ));
        }
        RailTab::Cloud => {
            panel = panel.child(cloud_view::<ShellState>(&s.cloud));
        }
        RailTab::Memory => {
            panel = panel.child(memory_view::<ShellState>(&s.memory));
        }
        RailTab::House => {
            panel = panel.child(house_view::<ShellState>(&s.house));
        }
    }
    panel
}

/// Transcript sheet for the open session: full thread (stream, notes,
/// composer, settings) over the map, like opening a chat.
fn transcript_sheet(s: &ShellState) -> Option<El> {
    let id = s.canvas.sessions.sheet.clone()?;
    let thread = s.canvas.sessions.threads.get(&id)?;
    let sheet = col()
        .key("session-sheet")
        .w(560.0)
        .h_full()
        .radius(16.0)
        .fill(ink("#0a0a0a"))
        .border(1.0, ink("#ffffff26"))
        .child(
            row()
                .key("session-sheet-head")
                .w_full()
                .items_center()
                .px(14.0)
                .pt(10.0)
                .pb(6.0)
                .child(
                    text(thread.title.clone())
                        .key("session-sheet-title")
                        .grow(1.0)
                        .font(rune::tok::TextStyle::new(13.0, 650.0))
                        .color(ink("#f2f2f5")),
                )
                .child(
                    text("×".to_string())
                        .key("session-sheet-close")
                        .role("button")
                        .label("Close chat")
                        .font(rune::tok::TextStyle::new(14.0, 400.0))
                        .color(ink("#8a8a93"))
                        .on_press(move |s: &mut ShellState| {
                            s.canvas.close_sheet();
                        }),
                ),
        )
        .child(transcript_body::<ShellState>(thread, Thread::Open));
    Some(sheet)
}

pub fn shell_view(s: &ShellState, _: &mut Cx<ShellState>) -> El {
    let mut tabs = row()
        .key("shell-tabs")
        .w_full()
        .items_center()
        .gap(4.0)
        .px(12.0)
        .pt(10.0)
        .pb(6.0);
    for id in s.canvas.sessions.order.clone() {
        tabs = tabs.child(session_tab(s, &id));
    }
    tabs = tabs.child(
        text("+ New session".to_string())
            .key("session-new")
            .role("button")
            .label("New session")
            .font(rune::tok::TextStyle::new(12.0, 600.0))
            .px(12.0)
            .py(6.0)
            .radius(8.0)
            .color(ink("#71717a"))
            .on_press(move |s: &mut ShellState| {
                s.canvas.new_session();
            }),
    );
    let rail = col()
        .key("icon-rail")
        .w(crate::rune_canvas::RAIL_W)
        .h_full()
        .fill(ink("#0c0c0d"))
        .items_center()
        .pt(10.0)
        .gap(4.0)
        .child(
            text("+".to_string())
                .key("rail-new")
                .role("button")
                .label("New session")
                .font(rune::tok::TextStyle::new(16.0, 400.0))
                .color(ink("#a1a1aa"))
                .on_press(move |s: &mut ShellState| {
                    s.canvas.new_session();
                }),
        )
        .child(rail_button(s, RailTab::Chats))
        .child(rail_button(s, RailTab::Agents))
        .child(rail_button(s, RailTab::Group))
        .child(rail_button(s, RailTab::Cloud))
        .child(rail_button(s, RailTab::Memory))
        .child(rail_button(s, RailTab::House))
        .child(
            text("⚙".to_string())
                .key("rail-settings")
                .role("button")
                .label("Open settings")
                .font(rune::tok::TextStyle::new(16.0, 400.0))
                .color(ink("#71717a"))
                .on_press(move |s: &mut ShellState| {
                    s.thread_mut(Thread::Main).settings_open = true;
                }),
        );
    let main = row()
        .key("stark-main")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(ink("#0a0a0a"))
        .child(rail)
        .child(side_panel(s))
        .child(crate::rune_canvas::canvas_body::<ShellState>(&s.canvas));
    let mut root = col()
        .key("stark-shell")
        .w_full()
        .h_full()
        .fill(ink("#0a0a0a"))
        .child(tabs)
        .child(main);
    if s.canvas.sessions.sheet.is_some() {
        if let Some(sheet) = transcript_sheet(s) {
            root = root.child(
                rune::el()
                    .key("session-overlay")
                    .absolute(0.0, 0.0)
                    .w_full()
                    .h_full()
                    .fill(rune::Color {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 0.6,
                    })
                    .center()
                    .modal(true, move |s: &mut ShellState| {
                        s.canvas.close_sheet();
                    })
                    .child(sheet),
            );
        }
    }
    if s.canvas.agents.selected.is_some() {
        root = root.child(crate::agents::agent_sheet::<ShellState>(&s.canvas.agents));
    }
    if s.transcript.settings_open {
        root = root.child(crate::rune_transcript::settings_sheet::<ShellState>(
            &s.transcript,
            Thread::Main,
        ));
    }
    root
}

/// `poll` drains every session thread (runs + notes) whatever panel is up
/// and refreshes status rooms at most every 10–15s, so runs started from the
/// dock keep streaming on the map. Card `running` flags mirror thread liveness.
pub fn shell_poll(s: &mut ShellState, clock: f64, tx: &mut rune::Tx) -> bool {
    let mut changed = drain_run_inbox(&mut s.transcript, clock, tx);
    changed |= drain_run_inbox(&mut s.personal, clock, tx);
    changed |= crate::notes::poll_notes_state(&mut s.transcript.notes);
    changed |= crate::notes::poll_notes_state(&mut s.personal.notes);
    crate::rune_transcript::poll_provider_state(&mut s.transcript, clock);
    crate::rune_transcript::poll_provider_state(&mut s.personal, clock);
    // Session threads: drain + sync card flags.
    let ids: Vec<String> = s.canvas.sessions.order.clone();
    for id in ids {
        if let Some(thread) = s.canvas.sessions.threads.get_mut(&id) {
            changed |= drain_run_inbox(thread, clock, tx);
            changed |= crate::notes::poll_notes_state(&mut thread.notes);
            crate::rune_transcript::poll_provider_state(thread, clock);
            let live = thread.live;
            if let Some(card) = s.canvas.cards.iter_mut().find(|c| c.id == id) {
                if card.running != live {
                    card.running = live;
                    changed = true;
                }
            }
        }
    }
    changed |= s.canvas.voice.drain(1.0 / 60.0);
    changed |= poll_house(s, clock);
    changed |= poll_memory(s, clock);
    changed |= poll_projects(s, clock);
    changed |= crate::agents::poll_agents_state(&mut s.canvas.agents, clock);
    changed |= poll_cloud_state(&mut s.cloud, clock);
    changed
}

/// Drain the house inbox; auto-refresh at most every 15s.
fn poll_house(s: &mut ShellState, clock: f64) -> bool {
    let (msgs, _) = drain_results(&mut s.house.inbox);
    let mut changed = false;
    for msg in msgs {
        match msg {
            Ok(snap) => {
                s.house.rooms = snap.rooms;
                s.house.devices = snap.devices;
                s.house.error = None;
                s.house.last_ok = clock;
            }
            Err(message) => {
                s.house.error = Some(message);
            }
        }
        s.house.loading = false;
        changed = true;
    }
    if s.house.auto && !s.house.loading && clock - s.house.last_ok > 15.0 {
        refresh_house(&mut s.house);
        changed = true;
    }
    changed
}

/// Drain the memory inbox; auto-refresh at most every 15s.
fn poll_memory(s: &mut ShellState, clock: f64) -> bool {
    let (msgs, _) = drain_results(&mut s.memory.inbox);
    let mut changed = false;
    for msg in msgs {
        match msg {
            Ok(workspaces) => {
                s.memory.workspaces = workspaces;
                s.memory.error = None;
                s.memory.last_ok = clock;
            }
            Err(message) => {
                s.memory.error = Some(message);
            }
        }
        s.memory.loading = false;
        changed = true;
    }
    if s.memory.auto && !s.memory.loading && clock - s.memory.last_ok > 15.0 {
        refresh_memory(&mut s.memory);
        changed = true;
    }
    changed |= poll_memory_detail(&mut s.memory);
    changed
}

/// Drain the projects rail inbox; auto-refresh at most every 15s. Shared
/// with the board via [`poll_projects_state`] — one drain path everywhere.
fn poll_projects(s: &mut ShellState, clock: f64) -> bool {
    poll_projects_state(&mut s.canvas.projects, clock)
}

fn init_shell() -> ShellState {
    ShellState::default()
}

fn apply_home(s: &mut ShellState) {
    // One page: every scene lands on the map. The rail picks the panel.
    s.canvas.rail = RailTab::Chats;
}

fn apply_panel_cloud(s: &mut ShellState) {
    s.canvas.rail = RailTab::Cloud;
}

fn apply_panel_memory(s: &mut ShellState) {
    s.canvas.rail = RailTab::Memory;
}

pub fn shell_spec() -> AppSpec<ShellState> {
    let mut spec = AppSpec::new("Stark", (1280.0, 900.0), init_shell, shell_view);
    spec.scenes = &[
        ("home", apply_home),
        ("cloud", apply_panel_cloud),
        ("memory", apply_panel_memory),
        // Compatibility aliases: everything lands on the one page.
        ("coding", apply_home),
        ("personal", apply_home),
        ("house", apply_home),
        ("admin", apply_home),
        ("design", apply_home),
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
    fn single_page_mounts_tabs_rail_panel_and_canvas() {
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(ShellState::default(), shell_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        // Session tabs seeded from the scene cards, rail + chats panel up,
        // canvas always mounted — one page, no routes.
        assert!(rt.find("session-chat:ws-general:chat-1").is_some());
        assert!(rt.find("session-new").is_some());
        assert!(rt.find("icon-rail").is_some());
        assert!(rt.find("map-sidebar").is_some());
        assert!(rt.find("canvas-world").is_some());
        // Rail switches the panel; the canvas never unmounts.
        let (x, y) = rt.find("rail-Cloud").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(rt.state.canvas.rail, RailTab::Cloud);
        assert!(rt.find("cloud-board").is_some());
        assert!(rt.find("canvas-world").is_some(), "canvas stays mounted");
    }

    #[test]
    fn session_tabs_select_new_and_close() {
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(ShellState::default(), shell_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        let n = rt.state.canvas.sessions.order.len();
        // New session adds a card + tab and selects it.
        let (x, y) = rt.find("session-new").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(rt.state.canvas.sessions.order.len(), n + 1);
        let id = rt.state.canvas.active_id.clone();
        assert!(rt.find(&format!("session-{id}")).is_some());
        // Tab press focuses the first scene card.
        let first = "chat:ws-general:chat-1".to_string();
        let (x, y) = rt.find(&format!("session-{first}")).unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(rt.state.canvas.active_id, first);
        // Close removes the tab, card and thread.
        let (x, y) = rt.find(&format!("session-{id}-close")).unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        assert!(!rt.state.canvas.sessions.order.contains(&id));
        assert!(rt.state.canvas.card(&id).is_none());
        assert!(!rt.state.canvas.sessions.threads.contains_key(&id));
    }

    #[test]
    fn personal_thread_stays_independent() {
        // The Personal thread still exists as fallback state (standalone
        // transcript spec); sending there never touches Main.
        let mut state = ShellState::default();
        let main_rows = state.transcript.rows.len();
        state.personal.api_port = 1;
        state.personal.edit.text = "personal hi".to_string();
        crate::rune_transcript::stage_send(&mut state.personal);
        assert_eq!(state.personal.rows.len(), 1);
        assert_eq!(state.transcript.rows.len(), main_rows);
    }

    #[test]
    fn cloud_refresh_loads_active_run() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            // Serve the active read and the board status read; anything else
            // gets an empty handoffs-shaped 404 (best-effort pickers).
            for _ in 0..4 {
                let (mut sock, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(_) => break,
                };
                let mut buf = [0u8; 4096];
                let mut raw = Vec::new();
                while let Ok(n) = sock.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    if raw.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let head = String::from_utf8_lossy(&raw);
                let body = if head.contains("/api/runs/active") {
                    r#"{"running":true,"workspace_path":"D:/proj","chat_id":"c9","started_at":"2026-11-09T00:00:00Z"}"#.to_string()
                } else if head.contains("/api/cloud/status") {
                    r#"{"origin":"desk","cap":4,"configured":false,"runs":[]}"#.to_string()
                } else if head.contains("/api/cloud/handoffs") {
                    r#"{"handoffs":[{"id":"h1","chat_id":"c9","workspace_id":"ws1","branch":"cloud/x","task":"fix","status":"staged","origin":"desk","package":"p.zip","updated_at":"2026-11-09T14:30:00Z"}]}"#.to_string()
                } else if head.contains("/api/workspaces") {
                    r#"{"workspaces":[]}"#.to_string()
                } else {
                    r#"{}"#.to_string()
                };
                let _ = sock.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                );
            }
        });
        let env = Env::new(true, 1.0);
        let mut state = ShellState::default();
        state.cloud.host = "127.0.0.1".to_string();
        state.cloud.port = port;
        state.cloud.auto = false;
        state.canvas.rail = RailTab::Cloud;
        let mut rt = Runtime::new(state, shell_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        assert!(rt.find("cloud-board").is_some());
        let (x, y) = rt.find("cloud-refresh").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert!(rt.state.cloud.loading);
        // Poll until the worker thread's message lands (local mock: ms).
        // `rt.update` (not direct `rt.state` mutation) commits the tree —
        // direct mutation leaves a stale commit behind.
        let mut waited = 0;
        while (rt.state.cloud.active.is_none() || rt.state.cloud.status.is_none()) && waited < 500 {
            rt.update(|s, tx| {
                shell_poll(s, 0.0, tx);
            });
            if rt.state.cloud.active.is_none() || rt.state.cloud.status.is_none() {
                std::thread::sleep(std::time::Duration::from_millis(10));
                waited += 1;
            }
        }
        let active = rt.state.cloud.active.clone().expect("active run stored");
        assert!(active.running);
        assert_eq!(active.workspace_path, "D:/proj");
        assert!(!rt.state.cloud.loading);
        // Board: status grid, staged handoff chip, unconfigured hint.
        // Settle frames: the handoff subtree commits a frame behind the
        // section shell (headless hit-testing lags one commit).
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        for key in [
            "cloud-grid",
            "cloud-handoffs",
            "cloud-handoff-h1-status",
            "cloud-unconfigured",
        ] {
            assert!(rt.find(key).is_some(), "{key} mounted");
        }
    }

    #[test]
    fn house_and_memory_rooms_load_from_mock() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        fn serve_once(body: &'static str) -> u16 {
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
                sock.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .unwrap();
            });
            port
        }
        let house_port = serve_once(
            r#"{"rooms":[{"id":"living","name":"Living Room"}],"devices":[{"id":"blind-living","name":"Window Blinds","roomId":"living","type":"blinds","state":{"position":1},"capabilities":[],"isOnline":true}]}"#,
        );
        let mem_port = serve_once(
            r#"{"workspaces":[{"id":"_general","name":"General","status":{"entryCount":7}}]}"#,
        );
        let env = Env::new(true, 1.0);
        let mut state = ShellState::default();
        state.house.host = "127.0.0.1".to_string();
        state.house.port = house_port;
        state.house.auto = false;
        state.memory.host = "127.0.0.1".to_string();
        state.memory.port = mem_port;
        state.memory.auto = false;
        state.canvas.rail = RailTab::House;
        let mut rt = Runtime::new(state, shell_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        assert!(rt.find("house-board").is_some());
        // Refresh from the House tab, then drain via shell poll through
        // `rt.update` (the AppHandle path): external data enters the frame
        // the same way in tests and in the window.
        let (x, y) = rt.find("house-refresh").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        let mut waited = 0;
        while rt.state.house.devices.is_empty() && waited < 500 {
            rt.update(|s, tx| {
                shell_poll(s, 0.0, tx);
            });
            rt.frame(0.0);
            std::thread::sleep(std::time::Duration::from_millis(10));
            waited += 1;
        }
        assert_eq!(rt.state.house.devices.len(), 1);
        assert_eq!(rt.state.house.devices[0].state_line, "100%");
        rt.frame(0.0);
        assert!(rt.find("house-dev-blind-living-name").is_some());
        // Memory panel loads its board the same way.
        let (x, y) = rt.find("rail-Memory").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(rt.state.canvas.rail, RailTab::Memory);
        // Settle frames after the route switch before tapping: hit-testing
        // lags one commit in stepped headless frames (real windows run
        // continuously, so hits are always fresh there).
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        let (x, y) = rt.find("memory-refresh").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        let mut waited = 0;
        while rt.state.memory.workspaces.is_empty() && waited < 500 {
            rt.update(|s, tx| {
                shell_poll(s, 0.0, tx);
            });
            rt.frame(0.0);
            std::thread::sleep(std::time::Duration::from_millis(10));
            waited += 1;
        }
        assert_eq!(
            rt.state.memory.workspaces.len(),
            1,
            "memory error: {:?}",
            rt.state.memory.error
        );
        assert_eq!(rt.state.memory.workspaces[0].entries, 7);
        rt.frame(0.0);
        assert!(rt.find("memory-ws-_general-name").is_some());
    }

    #[test]
    fn projects_rail_loads_from_mock() {
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
            let body = r#"{"projects":[{"id":"p1","title":"Website","status":"doing","items_done":3,"items_total":4,"awaiting_approval":1},{"id":"p2","goal":"Docs","status":"paused","items_done":0,"items_total":0,"milestones_done":1,"milestones_total":2,"awaiting_approval":0}],"scoped":false}"#;
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
        // Projects live in the Group panel now: scope global, poll off, and
        // read through the canvas projects state.
        state.canvas.projects.host = "127.0.0.1".to_string();
        state.canvas.projects.port = port;
        state.canvas.projects.auto = false;
        state.canvas.rail = RailTab::Group;
        let mut rt = Runtime::new(state, shell_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        assert!(rt.find("side-projects").is_some());
        assert!(rt.find("side-projects-scope").is_some());
        let (x, y) = rt.find("side-projects-refresh").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        let mut waited = 0;
        while rt.state.canvas.projects.projects.is_empty() && waited < 500 {
            rt.update(|s, tx| {
                shell_poll(s, 0.0, tx);
            });
            rt.frame(0.0);
            std::thread::sleep(std::time::Duration::from_millis(10));
            waited += 1;
        }
        assert_eq!(rt.state.canvas.projects.projects.len(), 2);
        rt.frame(0.0);
        for key in [
            "side-projects-p1-title",
            "side-projects-p1-bar",
            "side-projects-p2-title",
        ] {
            assert!(rt.find(key).is_some(), "{key} mounted");
        }
    }

    #[test]
    fn shell_send_and_poll_stream_across_panels() {
        use crate::stream::SseFrame;
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(ShellState::default(), shell_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        // Stage a run on Main, flip to the Cloud panel, drain via shell poll:
        // one page, so the run lands whatever panel is up.
        rt.state.transcript.api_port = 1;
        rt.state.transcript.edit.text = "hi".to_string();
        crate::rune_transcript::stage_send(&mut rt.state.transcript);
        assert!(rt.state.transcript.live);
        let (x, y) = rt.find("rail-Cloud").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        // Simulate the run thread delivering frames; poll applies them.
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

    #[test]
    fn dock_send_runs_active_session_and_opens_sheet() {
        // Map dock send lands in the ACTIVE card thread (chat types only)
        // and opens its transcript sheet.
        let env = Env::new(true, 1.0);
        let mut state = ShellState::default();
        state.canvas.edit.text = "hello map".to_string();
        let active = state.canvas.active_id.clone();
        // Point the active thread at the dead port (deterministic: the run
        // thread fails fast into an error frame).
        {
            let thread = state.canvas.thread_mut(&active);
            thread.api_port = 1;
        }
        let mut rt = Runtime::new(state, shell_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        let (x, y) = rt.find("composer-send").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(
            rt.state.canvas.sessions.sheet.as_deref(),
            Some(active.as_str())
        );
        let thread = rt.state.canvas.sessions.threads.get(&active).unwrap();
        assert!(thread.rows.iter().any(|r| r.body == "hello map"));
        assert!(rt.find("session-sheet").is_some(), "sheet mounts");
    }
}
