//! Single-page shell — one window, chat types only, no workspace routes.
//!
//! Layout mirrors the desktop app: one custom titlebar holds the open-session
//! tabs, the new-session action and the navigation controls, then a row of
//! icon rail + side panel + canvas map. The rail switches the panel (Chats /
//! Agents / Group / Cloud / Memory / House); the canvas always stays mounted,
//! so a running stream keeps draining whatever panel is up. A transcript sheet
//! opens over the map for the active session.

use crate::keys::EnterPress;
use rune::{col, row, text, text_input, AppSpec, Cx, El, LineCap, Path, Shape};

use crate::cloud::{cloud_view, poll_cloud_state, CloudHost, CloudState};
use crate::projects::{poll_projects_state, ProjectsHost};
use crate::rooms::{
    drain_results, house_view, memory_view, poll_memory_detail, refresh_house, refresh_memory,
    HouseHost, HouseState, MemoryHost, MemoryState,
};
use crate::rune_canvas::{canvas_body, chats_panel, CanvasHost, CanvasState, RailTab, RAIL_W};
use crate::rune_transcript::{
    drain_run_inbox, home_transcript_body, RuneTranscriptState, Thread, TranscriptHost,
};
use crate::settings::UpdatePhase;

/// Live window size (from the root's laid-out rect at the top of every frame).
/// The first frame before layout uses the spec size.
static mut WINDOW_W: f32 = 1280.0;
static mut WINDOW_H: f32 = 900.0;
static mut WINDOW_SAFE_BOTTOM: f32 = 0.0;

/// Sidebar/side-panel fold threshold (the reference UI `md:` = 768).
const PANEL_FOLD_W: f32 = 768.0;
/// The app-drawn titlebar: session tabs and window identity in one row. Rune
/// takes the native Windows titlebar away and draws the caption buttons over
/// the trailing edge (`Cx::chrome_insets`).
const TOPBAR_H: f32 = 44.0;
/// Gap between session tabs and the compact new-session control.
const TAB_GAP: f32 = 4.0;
/// Side panel width (`HouseSidebar` w-[248px]).
const PANEL_W: f32 = 248.0;
/// Right-hand subagent session panel (reference split view).
const DRILL_W: f32 = 420.0;
/// Phone bottom nav (icons + labels), before safe-area padding.
const MOBILE_NAV_H: f32 = 52.0;

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

fn rgba(r: f32, g: f32, b: f32, a: f32) -> rune::Color {
    rune::Color { r, g, b, a }
}

/// One stroked icon from a 24×24 path, Lucide geometry scaled to `size`.
fn icon_shape(path: Path, size: f32, color: rune::Color) -> Shape {
    Shape::new()
        .stroke_path(&path, crate::lucide_stroke(size), LineCap::Round, color)
        .view_box(24.0, 24.0)
}

/// Chevron up/down (Lucide 24-box) for the home chat collapse control.
fn chevron(size: f32, up: bool, color: rune::Color) -> Shape {
    let path = if up {
        Path::new()
            .move_to([18.0, 15.0])
            .line_to([12.0, 9.0])
            .line_to([6.0, 15.0])
    } else {
        Path::new()
            .move_to([6.0, 9.0])
            .line_to([12.0, 15.0])
            .line_to([18.0, 9.0])
    };
    icon_shape(path, size, color)
}

fn icon_new_chat(size: f32, color: rune::Color) -> Shape {
    // SquarePen: rectangle with an open top-right corner + pen.
    let mut p = Path::new()
        .move_to([12.0, 3.0])
        .line_to([5.0, 3.0])
        .cubic_to([3.9, 3.0], [3.0, 3.9], [3.0, 5.0])
        .line_to([3.0, 19.0])
        .cubic_to([3.0, 20.1], [3.9, 21.0], [5.0, 21.0])
        .line_to([19.0, 21.0])
        .cubic_to([20.1, 21.0], [21.0, 20.1], [21.0, 19.0])
        .line_to([21.0, 12.0]);
    p = p
        .move_to([18.4, 2.6])
        .cubic_to([19.2, 1.8], [20.5, 1.8], [21.3, 2.6])
        .cubic_to([22.1, 3.4], [22.1, 4.7], [21.3, 5.5])
        .line_to([12.0, 15.0])
        .line_to([8.0, 16.0])
        .line_to([9.0, 12.0])
        .close();
    icon_shape(p, size, color)
}

fn icon_home(size: f32, color: rune::Color) -> Shape {
    // Lucide v1.17 `house` (alias `Home`): door + rounded-eave roof.
    let p = Path::new()
        .move_to([15.0, 21.0])
        .line_to([15.0, 13.0])
        .cubic_to([15.0, 12.45], [14.55, 12.0], [14.0, 12.0])
        .line_to([10.0, 12.0])
        .cubic_to([9.45, 12.0], [9.0, 12.45], [9.0, 13.0])
        .line_to([9.0, 21.0])
        .move_to([3.0, 10.0])
        .cubic_to([3.0, 9.41], [3.26, 8.85], [3.71, 8.47])
        .line_to([10.71, 2.47])
        .cubic_to([11.45, 1.84], [12.55, 1.84], [13.29, 2.47])
        .line_to([20.29, 8.47])
        .cubic_to([20.74, 8.85], [21.0, 9.41], [21.0, 10.0])
        .line_to([21.0, 19.0])
        .cubic_to([21.0, 20.1], [20.1, 21.0], [19.0, 21.0])
        .line_to([5.0, 21.0])
        .cubic_to([3.9, 21.0], [3.0, 20.1], [3.0, 19.0])
        .close();
    icon_shape(p, size, color)
}

fn icon_group(size: f32, color: rune::Color) -> Shape {
    // Two bots side by side, same rounded-head shape as the empty Projects
    // mark: antenna, ears, eyes. The rail reads as "agents", not "people".
    let mut p = Path::new();
    let head = |mut p: Path, x0: f32, y0: f32, x1: f32, y1: f32, r: f32| {
        p = p.move_to([x0 + r, y0]);
        p = p.line_to([x1 - r, y0]);
        p = p.cubic_to([x1, y0], [x1, y0], [x1, y0 + r]);
        p = p.line_to([x1, y1 - r]);
        p = p.cubic_to([x1, y1], [x1, y1], [x1 - r, y1]);
        p = p.line_to([x0 + r, y1]);
        p = p.cubic_to([x0, y1], [x0, y1], [x0, y1 - r]);
        p = p.line_to([x0, y0 + r]);
        p = p.cubic_to([x0, y0], [x0, y0], [x0 + r, y0]);
        p.close()
    };

    // Left bot.
    p = p
        .move_to([6.0, 8.6])
        .line_to([6.0, 6.0])
        .line_to([9.0, 6.0]);
    p = head(p, 1.2, 8.6, 10.8, 18.6, 2.0);
    p = p.move_to([2.4, 12.6]).line_to([1.0, 12.6]);
    p = p.move_to([9.6, 12.6]).line_to([11.0, 12.6]);
    p = p.move_to([4.2, 12.4]).line_to([4.2, 13.8]);
    p = p.move_to([7.8, 12.4]).line_to([7.8, 13.8]);

    // Right bot, mirrored.
    p = p
        .move_to([18.0, 8.6])
        .line_to([18.0, 6.0])
        .line_to([15.0, 6.0]);
    p = head(p, 13.2, 8.6, 22.8, 18.6, 2.0);
    p = p.move_to([14.4, 12.6]).line_to([13.0, 12.6]);
    p = p.move_to([21.6, 12.6]).line_to([23.0, 12.6]);
    p = p.move_to([16.2, 12.4]).line_to([16.2, 13.8]);
    p = p.move_to([19.8, 12.4]).line_to([19.8, 13.8]);

    icon_shape(p, size, color)
}

fn icon_cloud(size: f32, color: rune::Color) -> Shape {
    // Lucide v1.17 `cloud`.
    let p = Path::new()
        .move_to([17.5, 19.0])
        .line_to([9.0, 19.0])
        .cubic_to([5.4, 19.0], [2.38, 16.26], [2.03, 12.67])
        .cubic_to([1.69, 9.09], [4.12, 5.82], [7.66, 5.13])
        .cubic_to([11.2, 4.44], [14.68, 6.55], [15.71, 10.0])
        .line_to([17.5, 10.0])
        .cubic_to([19.99, 10.0], [22.0, 12.01], [22.0, 14.5])
        .cubic_to([22.0, 16.99], [19.99, 19.0], [17.5, 19.0])
        .close();
    icon_shape(p, size, color)
}

fn icon_refresh(size: f32, color: rune::Color) -> Shape {
    crate::icons::lucide(
        &[
            "M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8",
            "M21 3v5h-5",
            "M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16",
            "M8 16H3v5",
        ],
        size,
        2.0,
        color,
    )
}

fn icon_settings(size: f32, color: rune::Color) -> Shape {
    // Lucide v1.17 `settings`: six-lobed gear outline + centre circle.
    let p = Path::new()
        .move_to([9.67, 4.14])
        .cubic_to([9.79, 2.93], [10.79, 2.02], [12.0, 2.02])
        .cubic_to([13.21, 2.02], [14.22, 2.93], [14.33, 4.14])
        .cubic_to([14.4, 4.9], [14.83, 5.58], [15.49, 5.96])
        .cubic_to([16.15, 6.34], [16.96, 6.37], [17.65, 6.05])
        .cubic_to([18.75, 5.55], [20.04, 5.97], [20.64, 7.01])
        .cubic_to([21.24, 8.05], [20.96, 9.38], [19.98, 10.08])
        .cubic_to([19.35, 10.52], [18.98, 11.24], [18.98, 12.0])
        .cubic_to([18.98, 12.76], [19.35, 13.48], [19.98, 13.91])
        .cubic_to([20.96, 14.62], [21.24, 15.95], [20.64, 16.99])
        .cubic_to([20.04, 18.03], [18.75, 18.45], [17.65, 17.95])
        .cubic_to([16.96, 17.63], [16.15, 17.66], [15.49, 18.04])
        .cubic_to([14.83, 18.42], [14.4, 19.1], [14.33, 19.86])
        .cubic_to([14.22, 21.06], [13.21, 21.98], [12.0, 21.98])
        .cubic_to([10.79, 21.98], [9.79, 21.06], [9.67, 19.86])
        .cubic_to([9.6, 19.1], [9.17, 18.42], [8.51, 18.04])
        .cubic_to([7.85, 17.66], [7.04, 17.63], [6.35, 17.95])
        .cubic_to([5.25, 18.45], [3.96, 18.03], [3.36, 16.99])
        .cubic_to([2.76, 15.95], [3.04, 14.62], [4.02, 13.92])
        .cubic_to([4.65, 13.48], [5.02, 12.76], [5.02, 12.0])
        .cubic_to([5.02, 11.24], [4.65, 10.52], [4.02, 10.08])
        .cubic_to([3.04, 9.38], [2.76, 8.05], [3.36, 7.01])
        .cubic_to([3.96, 5.97], [5.25, 5.56], [6.35, 6.05])
        .cubic_to([7.04, 6.37], [7.85, 6.34], [8.51, 5.96])
        .cubic_to([9.17, 5.58], [9.6, 4.9], [9.67, 4.14])
        .move_to([12.0, 9.0])
        .cubic_to([13.66, 9.0], [15.0, 10.34], [15.0, 12.0])
        .cubic_to([15.0, 13.66], [13.66, 15.0], [12.0, 15.0])
        .cubic_to([10.34, 15.0], [9.0, 13.66], [9.0, 12.0])
        .cubic_to([9.0, 10.34], [10.34, 9.0], [12.0, 9.0])
        .close();
    icon_shape(p, size, color)
}

fn rail_icon(tab: RailTab, size: f32, color: rune::Color) -> Shape {
    match tab {
        RailTab::Chats => icon_home(size, color),
        RailTab::Group => icon_group(size, color),
        RailTab::Cloud => icon_cloud(size, color),
        // Not in the reference UI's rail (single-page: chats, projects, cloud only);
        // mapped for completeness if a panel reuses the helper.
        RailTab::Agents | RailTab::Memory | RailTab::House => icon_home(size, color),
    }
}

/// Phone/browser pairing gate. Desktop never shows it: loopback requests are
/// already trusted. The token lives in localStorage and every wasm request
/// carries `X-API-Token` (see `transport`).
pub struct PairingState {
    pub locked: bool,
    pub token: rune::TextEdit,
    pub error: Option<String>,
    pub busy: bool,
    /// In-flight `/api/health` probe after a token submit. Dropped on clone.
    #[allow(dead_code)] // only drained by the wasm poll
    pub inbox: Option<std::sync::mpsc::Receiver<Result<(), String>>>,
}

impl std::fmt::Debug for PairingState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairingState")
            .field("locked", &self.locked)
            .field("token_len", &self.token.text.len())
            .field("error", &self.error)
            .field("busy", &self.busy)
            .finish()
    }
}

impl Default for PairingState {
    fn default() -> Self {
        Self {
            locked: false,
            token: rune::TextEdit::new(""),
            error: None,
            busy: false,
            inbox: None,
        }
    }
}

impl Clone for PairingState {
    fn clone(&self) -> Self {
        Self {
            locked: self.locked,
            token: self.token.clone(),
            error: self.error.clone(),
            busy: self.busy,
            inbox: None,
        }
    }
}

/// Hover-revealed home-card controls. `Live` has no `Debug`, so the shell
/// keeps it behind a tiny newtype to preserve `ShellState`'s derives.
#[derive(Clone)]
pub struct HomeControls(pub rune::Live<f32>);

impl std::fmt::Debug for HomeControls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HomeControls")
    }
}

#[derive(Debug, Clone)]
pub struct ShellState {
    pub transcript: RuneTranscriptState,
    pub personal: RuneTranscriptState,
    pub canvas: CanvasState,
    pub cloud: CloudState,
    pub house: HouseState,
    pub memory: MemoryState,
    /// Live chat list + open-tab snapshot (sidebar, cards, history).
    pub chats: crate::chats::ChatsState,
    /// Global Settings modal; it lives above every chat.
    pub settings: crate::settings::SettingsState,
    pub pairing: PairingState,
    /// Phone drawer. Desktop ignores it; below `md` it is closed until the
    /// topbar or bottom nav asks for a panel.
    pub mobile_panel_open: bool,
    /// Desktop chat panel (the chat list / agents / projects board docked
    /// beside the rail). The rail itself never hides: it owns the toggle and
    /// every destination, so the panel alone can collapse.
    pub panel_open: bool,
    /// Home chat card folded to its prompt dock (`HouseChat` minimize). The
    /// card is not keyed by chat, so this persists across chat switches like
    /// The reference UI's local `minimized` state.
    pub home_minimized: bool,
    /// Hover-revealed collapse control (`HouseChat`'s floating chevron).
    /// Desktop rests at 0 and fades in with the card; phones keep it visible.
    pub home_controls: HomeControls,
    /// Transcript whose audio is allowed to keep playing. Virtual rows unmount
    /// freely; switching chats stops the previous thread's clips.
    pub audio_owner: String,
    /// Last audio-engine revision, so a load failure/end repaints once.
    pub audio_rev: u64,
}

impl Default for ShellState {
    fn default() -> Self {
        let mut personal = RuneTranscriptState::screenshot();
        personal.title = "Personal".to_string();
        personal.rows = vec![];
        personal.live = false;
        // Distinct thread scope (notes + abort key off it, like `chat.id`).
        personal.chat_id = crate::notes::mint_id("rune-personal");
        personal.workspace_id = "_personal".to_string();
        personal.chat_ready = false;
        Self {
            transcript: RuneTranscriptState::screenshot(),
            personal,
            canvas: CanvasState::map(),
            cloud: CloudState::default(),
            house: HouseState::default(),
            memory: MemoryState::default(),
            chats: crate::chats::ChatsState::default(),
            settings: crate::settings::SettingsState::default(),
            pairing: PairingState::default(),
            mobile_panel_open: false,
            panel_open: true,
            home_minimized: false,
            home_controls: HomeControls(rune::Live::new(0.0)),
            audio_owner: "main".to_string(),
            audio_rev: 0,
        }
    }
}

impl crate::settings::SettingsHost for ShellState {
    fn settings(&self) -> &crate::settings::SettingsState {
        &self.settings
    }
    fn settings_mut(&mut self) -> &mut crate::settings::SettingsState {
        &mut self.settings
    }
}

impl TranscriptHost for ShellState {
    fn thread(&self, which: Thread) -> &RuneTranscriptState {
        match which {
            Thread::Main => &self.transcript,
            // The sheet's card thread; falls back to Main when closed.
            Thread::Open => self.canvas.sheet_thread().unwrap_or(&self.transcript),
            Thread::Card(index) => self
                .canvas
                .sessions
                .order
                .get(index)
                .and_then(|id| self.canvas.sessions.threads.get(id))
                .unwrap_or(&self.transcript),
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
            Thread::Card(index) => {
                let id = self.canvas.sessions.order.get(index).cloned();
                match id {
                    Some(id) => self.canvas.thread_mut(&id),
                    None => &mut self.transcript,
                }
            }
        }
    }
}

impl CanvasHost for ShellState {
    fn canvas_mut(&mut self) -> &mut CanvasState {
        &mut self.canvas
    }

    fn chats_mut(&mut self) -> Option<&mut crate::chats::ChatsState> {
        Some(&mut self.chats)
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

/// One session tab: card title + close. Press activates the chat (the reference UI
/// `SessionTabs`: the active tab drives the home chat card).
fn session_tab(s: &ShellState, id: &str, width: f32) -> El {
    let title = s
        .canvas
        .card(id)
        .map(|c| c.title.clone())
        .or_else(|| s.canvas.sessions.threads.get(id).map(|t| t.title.clone()))
        .unwrap_or_else(|| "Chat".to_string());
    let kind = s
        .canvas
        .card(id)
        .map(|c| c.kind.clone())
        .unwrap_or_else(|| "Main".to_string());
    // `isSpawnedChat`: a chat row with a parent gets the Bot glyph and the
    // `↳ ` title prefix (the reference UI `SessionTabs`).
    let spawned = s
        .canvas
        .chat_rows
        .iter()
        .find(|c| c.card_id() == id)
        .is_some_and(|c| c.parent_chat_id.is_some());
    let active = s.canvas.active_id == id;
    let running = s.canvas.card(id).map(|c| c.running).unwrap_or(false);
    let select = id.to_string();
    let close = id.to_string();
    let title_text = if spawned {
        format!("↳ {title}")
    } else {
        title
    };
    let mut title_style = rune::tok::TextStyle::new(12.0, 400.0);
    title_style.line_height = 12.0;
    // Rune measures text to whole points (`measure` rounds up); the reference UI keeps
    // The fractional advance. Trimming the rounding off the right padding
    // keeps the tab's outer width — and the next tab's x — exact.
    let advance = rune::text::advance(&title_text, title_style);
    let right_pad = 11.0 - (advance.ceil() - advance);
    let mut el = row()
        .key(format!("session-{id}"))
        .role("tab")
        .focusable(true)
        .focus_selected(active)
        .focus_ring(2.0, 2.0, rgba(1.0, 1.0, 1.0, 0.55))
        .w(width)
        .h(30.0)
        .items_center()
        .gap(8.0)
        .pl(11.0)
        .pr(right_pad)
        .radius(8.0)
        .border(1.0, ink(if active { "#ffffff24" } else { "#00000000" }))
        .hover_group()
        .on_press_tx_enter(move |s: &mut ShellState, tx: &mut rune::Tx| {
            s.canvas.select_card(&select);
            tx.scroll_into_view(format!("session-{select}"));
        });
    if !active {
        // SessionTabs inactive: hover:bg-white/[0.05] hover:text-zinc-200.
        el = el.hover(rgba(1.0, 1.0, 1.0, 0.05));
    }
    el = el
        .child(
            rune::el()
                .key(format!("session-{id}-icon"))
                .size(12.0, 12.0)
                .inert()
                .shape(if spawned {
                    crate::rune_canvas::bot_icon(
                        12.0,
                        ink(if active { "#e4e4e7" } else { "#71717a" }),
                    )
                } else if kind == "Coding" {
                    tab_code_icon(12.0, ink(if active { "#e4e4e7" } else { "#71717a" }))
                } else {
                    tab_chat_icon(12.0, ink(if active { "#e4e4e7" } else { "#71717a" }))
                }),
        )
        .child({
            let title = text(title_text)
                .key(format!("session-{id}-title"))
                .grow(1.0)
                .min_w(0.0)
                .truncate()
                .font(title_style)
                .color(ink(if active { "#ffffff" } else { "#a1a1aa" }));
            if active {
                title
            } else {
                title.group_color(ink("#e4e4e7"))
            }
        });
    if running {
        // `session-run-dot` (6pt, breathing glow) between title and close.
        el = el.child(
            rune::el()
                .key(format!("session-{id}-run"))
                .size(6.0, 6.0)
                .radius(3.0)
                .fill(ink("#fafafa"))
                .shadow(0.0, 8.0, home_glow())
                .looping(rune::LoopSpec::opacity(1.6, 0.0, 0.55, 1.0)),
        );
    }
    el = el.child(
        rune::el()
            .key(format!("session-{id}-close"))
            .role("button")
            .focusable(true)
            .label("Close session")
            .size(18.0, 18.0)
            .center()
            .radius(6.0)
            .group_opacity(if active { 1.0 } else { 0.0 }, 1.0)
            .hover(rgba(1.0, 1.0, 1.0, if active { 0.12 } else { 0.10 }))
            .hover_group()
            .on_press_enter(move |s: &mut ShellState| {
                s.canvas.close_session(&close);
            })
            .child({
                let x_icon = |color: rune::Color| {
                    Shape::new()
                        .line(
                            [6.0, 6.0],
                            [18.0, 18.0],
                            crate::lucide_stroke(11.0),
                            LineCap::Round,
                            color,
                        )
                        .line(
                            [18.0, 6.0],
                            [6.0, 18.0],
                            crate::lucide_stroke(11.0),
                            LineCap::Round,
                            color,
                        )
                        .view_box(24.0, 24.0)
                };
                rune::el()
                    .size(11.0, 11.0)
                    .inert()
                    .child(
                        rune::el()
                            .size(11.0, 11.0)
                            .inert()
                            .shape(x_icon(ink(if active { "#71717a" } else { "#52525b" }))),
                    )
                    .child(
                        rune::el()
                            .size(11.0, 11.0)
                            .absolute(0.0, 0.0)
                            .inert()
                            .group_opacity(0.0, 1.0)
                            .shape(x_icon(ink("#ffffff"))),
                    )
            }),
    );
    if active {
        el = el
            .fill(rune::Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.10,
            })
            // `shadow-[inset_0_1px_0_rgba(255,255,255,0.06)]`.
            .box_shadow(rune::Shadow {
                x: 0.0,
                y: 1.0,
                blur: 0.0,
                spread: 0.0,
                color: ink("#ffffff0f"),
                inset: true,
            });
    }
    el
}

/// Soft white glow (`box-shadow: 0 0 8px rgba(255,255,255,.65)`).
fn home_glow() -> rune::Color {
    rune::Color {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 0.65,
    }
}

/// Tab glyph: `MessageSquare` at 12pt.
fn tab_chat_icon(size: f32, color: rune::Color) -> Shape {
    // Lucide v1.17 `message-square`.
    let p = Path::new()
        .move_to([22.0, 17.0])
        .cubic_to([22.0, 18.1], [21.1, 19.0], [20.0, 19.0])
        .line_to([6.83, 19.0])
        .cubic_to([6.3, 19.0], [5.79, 19.21], [5.41, 19.59])
        .line_to([3.21, 21.79])
        .cubic_to([3.01, 21.99], [2.7, 22.05], [2.44, 21.94])
        .cubic_to([2.17, 21.83], [2.0, 21.57], [2.0, 21.29])
        .line_to([2.0, 5.0])
        .cubic_to([2.0, 3.9], [2.9, 3.0], [4.0, 3.0])
        .line_to([20.0, 3.0])
        .cubic_to([21.1, 3.0], [22.0, 3.9], [22.0, 5.0])
        .close();
    Shape::new()
        .stroke_path(&p, crate::lucide_stroke(size), LineCap::Round, color)
        .view_box(24.0, 24.0)
}

/// Tab glyph: `Code2` at 12pt.
fn tab_code_icon(size: f32, color: rune::Color) -> Shape {
    let p = Path::new()
        .move_to([18.0, 16.0])
        .line_to([22.0, 12.0])
        .line_to([18.0, 8.0])
        .move_to([6.0, 8.0])
        .line_to([2.0, 12.0])
        .line_to([6.0, 16.0])
        .move_to([14.5, 4.0])
        .line_to([9.5, 20.0]);
    Shape::new()
        .stroke_path(&p, crate::lucide_stroke(size), LineCap::Round, color)
        .view_box(24.0, 24.0)
}

/// Icon rail button (Lucide-shaped icon + label for assistive tech).
/// Phone bottom navigation (`HouseMobileNav`): same destinations as the
/// desktop rail without stealing chat width. Safe-area bottom padding keeps
/// The bar above the home indicator.
fn mobile_nav(s: &ShellState, safe_bottom: f32) -> El {
    let waiting: u64 = s.canvas.projects.projects.iter().map(|p| p.waiting).sum();
    let mut nav = row()
        .key("house-mobile-nav")
        .w_full()
        .h(MOBILE_NAV_H + safe_bottom)
        .items_stretch()
        .gap(4.0)
        .px(8.0)
        .pt(4.0)
        .pb(4.0 + safe_bottom)
        .fill(ink("#0c0c0c"))
        .border_sides([1.0, 0.0, 0.0, 0.0], ink("#ffffff12"))
        .border_layout();
    #[derive(Clone, Copy)]
    enum NavGlyph {
        Rail(RailTab),
        New,
        Settings,
    }
    let glyph = |glyph: NavGlyph, color: rune::Color| match glyph {
        NavGlyph::Rail(tab) => rail_icon(tab, 18.0, color),
        NavGlyph::New => icon_new_chat(18.0, color),
        NavGlyph::Settings => icon_settings(18.0, color),
    };
    let item = |key: &str,
                label: &'static str,
                active: bool,
                icon: NavGlyph,
                badge: u64,
                action: std::rc::Rc<dyn Fn(&mut ShellState)>| {
        let color = ink(if active { "#fafafa" } else { "#71717a" });
        let mut button = col()
            .key(key)
            .role("button")
            .focusable(true)
            .label(label)
            .help(label)
            .grow(1.0)
            .items_center()
            .justify_center()
            .gap(2.0)
            .py(6.0)
            .radius(8.0)
            .on_press_enter(move |s: &mut ShellState| action(s))
            .child(
                rune::el()
                    .size(18.0, 18.0)
                    .inert()
                    .shape(glyph(icon, color)),
            )
            .child(
                text(label.to_string())
                    .font(rune::tok::TextStyle::new(9.0, 400.0))
                    .color(color),
            );
        if badge > 0 {
            button = button.child(
                text(badge.to_string())
                    .absolute(30.0, 0.0)
                    .font(crate::motion::tabular(rune::tok::TextStyle::new(
                        8.0, 700.0,
                    )))
                    .color(ink("#0a0a0d")),
            );
        }
        button
    };
    nav = nav
        .child(item(
            "mobile-chats",
            "Chats",
            s.canvas.rail == RailTab::Chats,
            NavGlyph::Rail(RailTab::Chats),
            0,
            std::rc::Rc::new(|s: &mut ShellState| {
                s.canvas.rail = RailTab::Chats;
                s.mobile_panel_open = true;
            }),
        ))
        .child(item(
            "mobile-new",
            "New",
            false,
            NavGlyph::New,
            0,
            std::rc::Rc::new(|s: &mut ShellState| {
                s.canvas.new_session();
                s.mobile_panel_open = false;
            }),
        ))
        .child(item(
            "mobile-projects",
            "Projects",
            s.canvas.rail == RailTab::Group,
            NavGlyph::Rail(RailTab::Group),
            waiting,
            std::rc::Rc::new(|s: &mut ShellState| {
                s.canvas.rail = RailTab::Group;
                s.mobile_panel_open = true;
                if !s.canvas.projects.loading && s.canvas.projects.projects.is_empty() {
                    crate::projects::refresh_projects(&mut s.canvas.projects);
                }
            }),
        ))
        .child(item(
            "mobile-cloud",
            "Cloud",
            s.canvas.rail == RailTab::Cloud,
            NavGlyph::Rail(RailTab::Cloud),
            0,
            std::rc::Rc::new(|s: &mut ShellState| {
                s.canvas.rail = RailTab::Cloud;
                s.mobile_panel_open = true;
            }),
        ))
        .child(item(
            "mobile-settings",
            "Settings",
            false,
            NavGlyph::Settings,
            0,
            std::rc::Rc::new(open_settings),
        ));
    nav
}

fn open_settings(s: &mut ShellState) {
    let (host, port) = s
        .canvas
        .sheet_thread()
        .map(|t| (t.api_host.clone(), t.api_port))
        .unwrap_or_else(|| {
            (
                crate::rune_transcript::DEFAULT_API_HOST.to_string(),
                crate::rune_transcript::api_port(),
            )
        });
    s.settings.open(host, port);
}

fn rail_button(s: &ShellState, tab: RailTab) -> El {
    let active = s.canvas.rail == tab;
    let color = ink(if active { "#fafafa" } else { "#71717a" });
    let mut el = rune::el()
        .key(format!("rail-{}", tab.label()))
        .role("button")
        .focusable(true)
        .label(format!("{} panel", tab.label()))
        .help(tab.label().to_string())
        .size(36.0, 36.0)
        .radius(12.0)
        .center()
        .hover_group()
        .on_press_enter(move |s: &mut ShellState| {
            s.canvas.rail = tab;
            // Picking a destination always brings its panel back into view;
            // the rail itself never moves.
            s.panel_open = true;
            if tab == RailTab::Group
                && !s.canvas.projects.loading
                && s.canvas.projects.projects.is_empty()
            {
                crate::projects::refresh_projects(&mut s.canvas.projects);
            }
        })
        // `.house-rail-btn:active { transform: scale(.94) }`.
        .press_scale(0.94)
        .child({
            let mut icon = rune::el()
                .key(format!("rail-{}-icon", tab.label()))
                .size(17.0, 17.0)
                .inert()
                .child(
                    rune::el()
                        .size(17.0, 17.0)
                        .inert()
                        .shape(rail_icon(tab, 17.0, color)),
                );
            if !active {
                icon = icon.child(
                    rune::el()
                        .size(17.0, 17.0)
                        .absolute(0.0, 0.0)
                        .inert()
                        .group_opacity(0.0, 1.0)
                        .shape(rail_icon(tab, 17.0, ink("#e4e4e7"))),
                );
            }
            icon
        });
    if active {
        el = el.fill(rune::Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.10,
        });
    } else {
        // HouseRail inactive: hover:text-zinc-200 hover:bg-white/[0.07].
        el = el.hover(rgba(1.0, 1.0, 1.0, 0.07));
    }
    el
}

/// Side panel content for the active rail tab. Chat-local panels read the
/// canvas; cloud/memory/house read shell state.
fn side_panel(s: &ShellState, width: f32) -> El {
    // The chats panel owns its own gutters (`HouseSidebar`); the other boards
    // sit inside the shared 12pt shell padding.
    let chats = s.canvas.rail == RailTab::Chats;
    let mut panel = col()
        .key("side-panel")
        .w(width)
        .h_full()
        .fill(ink("#0f0f0f"))
        .border_sides([0.0, 1.0, 0.0, 0.0], ink("#ffffff12"))
        .border_layout();
    if !chats {
        panel = panel.px(12.0).pt(12.0).gap(8.0);
    }
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

/// Home chat card (`HouseChat variant="home"`): the active session's
/// conversation, bottom-anchored over the map, prompt dock always visible.
/// Width hugs the reference UI's `md:max-w-[740px]`, height caps at 74vh.
fn home_chat_overlay(s: &ShellState, stage_w: f32, stage_h: f32, window_h: f32) -> Option<El> {
    // The reference UI's `HouseChat` is mounted even with no open chats: a fresh
    // account sees the empty "What to do Boss?" card and prompt dock over the
    // empty canvas. With no sheet, reuse the always-present main thread.
    let (thread, which) = match s.canvas.sheet_thread() {
        Some(thread) => (thread, Thread::Open),
        None if s.canvas.cards.is_empty() => (&s.transcript, Thread::Main),
        None => return None,
    };
    // The reference UI caps the wrapper (`md:max-w-[740px]`) and the wrapper's own
    // `px-2` gutters sit inside that cap: the card is at most 724 wide.
    // At 800 the stage is narrower than the cap, so this matches the old
    // `stage_w - 16` exactly.
    let card_w = stage_w.clamp(260.0, 740.0) - 16.0;
    let mut card = col()
        .key("home-chat-card")
        .w(card_w)
        .radius(16.0)
        .border(1.0, ink("#ffffff14"))
        // The reference UI is `rgba(13,13,16,.94)` over `backdrop-filter: blur(18px)`.
        // The web renderer's backdrop copy leaves the canvas cards' text
        // legible through 6% bleed; at 99% the composite is the same flat
        // colour the reference UI shows (the desktop blur still smooths the remainder).
        .fill(ink("#0d0d10fc"))
        // `.house-chat-card { backdrop-filter: blur(18px) }`: without it the
        // fill leaves sharp canvas ghosts under the conversation.
        .backdrop_blur(18.0)
        .px(8.0)
        .pt(6.0)
        .pb(4.0)
        .clip_rounded(16.0)
        // CSS borders take layout room; matching that keeps the inner
        // transcript/dock exactly the reference UI's 469pt (487 - 2 - 16).
        .border_layout()
        .on_hover_change({
            let controls = s.home_controls.0.clone();
            move |_: &mut ShellState, inside, tx| {
                controls.set(if inside { 1.0 } else { 0.0 });
                tx.live_only();
            }
        });
    // `max-h-[74vh]` on the home variant. Empty and message threads both
    // saturate it at this window size; an explicit height (rather than a
    // `max_h` flex guess) is what keeps the scroll viewport from collapsing
    // inside Rune's auto-height column. Folded, the card hugs the dock.
    let minimized = s.home_minimized;
    if !minimized {
        let h = (window_h * 0.74).clamp(220.0, (stage_h - 8.0).max(220.0));
        card = card.h(h);
    }
    card = card.child(home_transcript_body::<ShellState>(
        thread, which, card_w, window_h, minimized,
    ));
    // The reference UI `HouseChat` home: a floating 28pt chevron at the card's top-right.
    // It rests hidden on desktop and fades in with the card; phones keep it
    // visible. The button folds the message area and leaves the prompt dock.
    let icon_color = if (unsafe { WINDOW_W }) < PANEL_FOLD_W {
        ink("#71717a")
    } else {
        ink("#52525b")
    };
    let mut collapse = rune::el()
        .key("home-chat-collapse")
        .size(28.0, 28.0)
        .radius(8.0)
        .center()
        .pin(None, Some(6.0), Some(6.0), None)
        .role("button")
        .focusable(true)
        .label(if minimized {
            "Expand conversation"
        } else {
            "Collapse conversation"
        })
        .help(if minimized {
            "Expand conversation"
        } else {
            "Collapse conversation"
        })
        .hover(rgba(255.0, 255.0, 255.0, 0.08))
        .focus_ring(1.0, 1.0, rgba(255.0, 255.0, 255.0, 0.5))
        .on_press_enter(|s: &mut ShellState| {
            s.home_minimized = !s.home_minimized;
        })
        .child(
            rune::el()
                .key("home-chat-collapse-icon")
                .size(14.0, 14.0)
                .inert()
                .shape(chevron(14.0, !minimized, icon_color)),
        );
    // The reference UI's `md:` is the viewport, not the canvas stage; a narrow stage
    // behind a wide sidebar must not pin the control visible.
    collapse = if (unsafe { WINDOW_W }) < PANEL_FOLD_W {
        collapse.opacity(1.0)
    } else {
        collapse.live_opacity(s.home_controls.0.smooth(crate::motion::HOVER_SPRING))
    };
    card = card.child(collapse);
    Some(
        col()
            .key("home-chat-overlay")
            // Left/right + bottom pinned, auto height: the card hugs the
            // dock for an empty thread and is 74vh once messages exist.
            .pin(Some(0.0), None, Some(0.0), Some(8.0))
            .items_center()
            .child(card),
    )
}

/// Modals owned by the active card thread. They render at the shell root so
/// The home chat card's clipping never crops them (the reference UI portals its dialogs
/// over the whole workspace).
/// Thread whose subagent session is open, if any. Mirrors the home card's
/// own fallback: with no sheet, a fresh account still owns the main thread.
fn drill_thread(s: &ShellState) -> Option<&crate::rune_transcript::RuneTranscriptState> {
    s.canvas.sheet_thread().or({
        if s.canvas.cards.is_empty() {
            Some(&s.transcript)
        } else {
            None
        }
    })
}

fn active_chat_sheets(s: &ShellState) -> Vec<El> {
    // A fresh account has no canvas sheet yet, but the always-mounted home
    // card still owns the main thread's dock. Its modals must mount at the
    // shell root too, otherwise the provider/settings pills would be inert
    // until a chat is opened.
    let Some(t) = s.canvas.sheet_thread().or({
        if s.canvas.cards.is_empty() {
            Some(&s.transcript)
        } else {
            None
        }
    }) else {
        return Vec::new();
    };
    let mut sheets = Vec::new();
    if t.notes.open {
        sheets.push(crate::notes::notes_sheet::<ShellState>(
            &t.notes,
            Thread::Open,
        ));
    }
    if t.stats_open {
        sheets.push(crate::token_stats::stats_sheet::<ShellState>(
            &t.usage,
            &t.model,
            std::rc::Rc::new(move |s: &mut ShellState| {
                s.thread_mut(Thread::Open).stats_open = false;
            }),
        ));
    }
    if t.agent_config.open {
        sheets.push(crate::agent_config::agent_config_sheet::<ShellState>(
            &t.agent_config,
            Thread::Open,
        ));
    }
    if t.provider_keys.open {
        let (w, h) = unsafe { (WINDOW_W, WINDOW_H) };
        sheets.push(crate::provider_keys::provider_keys_sheet::<ShellState>(
            &t.provider_keys,
            Thread::Open,
            w,
            h,
        ));
    }
    if let Some(file) = t.attachment_preview.clone() {
        // The card clips its content, so the message lightbox lives at the
        // shell root like the reference UI's fixed overlay.
        let (w, h) = unsafe { (WINDOW_W, WINDOW_H) };
        sheets.push(crate::rune_transcript::attachment_lightbox::<ShellState>(
            &file,
            w,
            h,
            &t.workspace_id,
            &t.chat_id,
            |s: &mut ShellState| {
                s.thread_mut(Thread::Open).attachment_preview = None;
            },
        ));
    }
    sheets
}

/// Lucide `Menu` for the rail's chat-panel toggle.
fn icon_menu(size: f32, color: rune::Color) -> Shape {
    crate::icons::lucide(&["M4 6h16", "M4 12h16", "M4 18h16"], size, 2.0, color)
}

/// The one app-drawn titlebar: open-session tabs, the compact new-session
/// action, and Rune's Windows caption buttons over the trailing edge. The
/// band drags the window; presses on its controls win.
///
/// Navigation lives in the rail, not here: the topbar is only the chat strip.
fn shell_topbar(s: &ShellState, cx: &mut Cx<ShellState>, w: f32) -> El {
    let chrome = cx.chrome_insets();
    cx.caption_color(ink("#d4d4d8"));
    let top_h = if chrome.height > 0.0 {
        chrome.height
    } else {
        TOPBAR_H
    };

    // Give tabs equal shares of the visible strip, like the reference UI:
    // 210pt with room, shrinking to 96pt before the strip scrolls.
    let tabs_n = s.canvas.sessions.order.len();
    // `+`, both outer gutters and the gap; the rest is the tab strip. The
    // caption reservation is part of the trailing gutter.
    let fixed = 8.0 + 6.0 + chrome.trailing;
    let strip_w = (w - chrome.leading - fixed).max(120.0);
    // Each inactive pair carries a hairline divider (1pt + its two gaps).
    let sep_total = 1.0 + TAB_GAP * 2.0;
    let separators = tabs_n.saturating_sub(1) as f32;
    let tab_w = if tabs_n == 0 {
        210.0
    } else {
        ((strip_w - TAB_GAP * separators - sep_total * separators) / tabs_n as f32)
            .clamp(96.0, 210.0)
    };
    // The strip hugs the tabs, and `+` sits right after it. Beyond the
    // visible width the strip scrolls instead of pushing the button aside.
    let tabs_width = if tabs_n == 0 {
        0.0
    } else {
        (tab_w * tabs_n as f32 + TAB_GAP * separators + sep_total * separators).min(strip_w)
    };
    let mut tabs = row()
        .key("shell-tabs")
        .w(tabs_width)
        .h_full()
        .items_center()
        .gap(TAB_GAP)
        // A new tab or a resize must not spring the strip's width: the `+`
        // and the caption buttons stay put while the tab itself fades in.
        .instant_changes()
        // The reference UI's strip is `overflow-x-auto`; keep every tab
        // reachable when the window is narrow.
        .scroll_x()
        .scroll_edge_fade(18.0)
        .role("tablist")
        .label("Sessions")
        .focus_group();
    let mut previous_active = false;
    for (index, id) in s.canvas.sessions.order.clone().iter().enumerate() {
        let active = s.canvas.active_id == *id;
        // A divider between two quiet tabs; the active pill separates itself.
        if index > 0 && !active && !previous_active {
            tabs = tabs.child(
                rune::el()
                    .key(format!("tab-sep-{index}"))
                    .w(1.0)
                    .h(14.0)
                    .radius(0.5)
                    .fill(ink("#ffffff14"))
                    .inert(),
            );
        }
        tabs = tabs.child(session_tab(s, id, tab_w));
        previous_active = active;
    }

    row()
        .key("shell-titlebar")
        .role("toolbar")
        .label("Title bar")
        .w_full()
        .h(top_h)
        .items_center()
        .gap(TAB_GAP)
        .pl(chrome.leading + 8.0)
        .pr(chrome.trailing + 6.0)
        // The rail's surface, so the titlebar reads as chrome rather than as
        // another row of canvas.
        .fill(ink("#0c0c0c"))
        .border_sides([0.0, 0.0, 1.0, 0.0], ink("#ffffff14"))
        // Tab widths, the `+` and the caption reservation change together;
        // springing their positions would briefly overlap hit targets.
        .instant_changes()
        .window_drag()
        .child(tabs)
}

pub fn shell_view(s: &ShellState, cx: &mut Cx<ShellState>) -> El {
    // Window size for this frame: the root's laid-out rect (rebuilds on
    // resize). Spec size stands in until the first layout lands.
    let (w, h) = cx
        .window_layout("stark-shell")
        .map(|r| (r[2], r[3]))
        .filter(|(w, h)| *w > 1.0 && *h > 1.0)
        .unwrap_or((crate::rune_canvas::MAP_W, crate::rune_canvas::MAP_H));
    // Browser notch/home-indicator inset; zero on native.
    let safe_bottom = cx.safe_area()[2];
    // SAFETY: the view runs single-threaded on the UI thread.
    unsafe {
        WINDOW_W = w;
        WINDOW_H = h;
        WINDOW_SAFE_BOTTOM = safe_bottom;
    }
    let topbar = shell_topbar(s, cx, w);
    let folded = w < PANEL_FOLD_W;

    let rail = col()
        .key("icon-rail")
        .w(RAIL_W)
        .h_full()
        .fill(ink("#0c0c0c"))
        .items_center()
        .py(12.0)
        .gap(4.0)
        .border_sides([0.0, 1.0, 0.0, 0.0], ink("#ffffff12"))
        .border_layout()
        .child(
            rune::el()
                .key("rail-menu")
                .role("button")
                .focusable(true)
                .label("Toggle chat panel")
                .help("Toggle chat panel")
                .size(36.0, 36.0)
                .radius(12.0)
                .center()
                .hover(rgba(1.0, 1.0, 1.0, 0.07))
                .hover_group()
                .on_press_enter(move |s: &mut ShellState| {
                    // Only the panel collapses; the rail stays so settings,
                    // chats, group, and cloud remain reachable.
                    s.panel_open = !s.panel_open;
                })
                .press_scale(0.94)
                .child(
                    rune::el()
                        .key("rail-menu-icon")
                        .size(17.0, 17.0)
                        .inert()
                        .child(
                            rune::el()
                                .size(17.0, 17.0)
                                .inert()
                                .shape(icon_menu(17.0, ink("#71717a"))),
                        )
                        .child(
                            rune::el()
                                .size(17.0, 17.0)
                                .absolute(0.0, 0.0)
                                .inert()
                                .group_opacity(0.0, 1.0)
                                .shape(icon_menu(17.0, ink("#e4e4e7"))),
                        ),
                ),
        )
        .child(
            rune::el()
                .key("rail-divider")
                .w(24.0)
                .h(1.0)
                .my(6.0)
                .fill(ink("#ffffff12")),
        )
        .child(rail_button(s, RailTab::Chats))
        .child(rail_button(s, RailTab::Group))
        .child(rail_button(s, RailTab::Cloud))
        .child(rune::el().key("rail-spacer").grow(1.0))
        .child(
            rune::el()
                .key("rail-settings")
                .role("button")
                .focusable(true)
                .label("Open settings")
                .help("Settings")
                .size(36.0, 36.0)
                .radius(12.0)
                .center()
                .hover(rgba(1.0, 1.0, 1.0, 0.07))
                .hover_group()
                .on_press_enter(move |s: &mut ShellState| {
                    let (host, port) = s
                        .canvas
                        .sheet_thread()
                        .map(|t| (t.api_host.clone(), t.api_port))
                        .unwrap_or_else(|| {
                            (
                                crate::rune_transcript::DEFAULT_API_HOST.to_string(),
                                crate::rune_transcript::api_port(),
                            )
                        });
                    s.settings.open(host, port);
                })
                .press_scale(0.94)
                .child(
                    rune::el()
                        .key("rail-settings-icon")
                        .size(17.0, 17.0)
                        .inert()
                        .child(
                            rune::el()
                                .size(17.0, 17.0)
                                .inert()
                                .shape(icon_settings(17.0, ink("#71717a"))),
                        )
                        .child(
                            rune::el()
                                .size(17.0, 17.0)
                                .absolute(0.0, 0.0)
                                .inert()
                                .group_opacity(0.0, 1.0)
                                .shape(icon_settings(17.0, ink("#e4e4e7"))),
                        ),
                ),
        );
    // Desktop keeps the rail permanently; only its panel folds. Phones hide
    // both and open the drawer instead.
    let rail_visible = !folded;
    let panel_visible = !folded && s.panel_open;
    // A subagent session docks to the right of the chat, like the reference
    // split view, instead of replacing the conversation.
    let drill_thread = drill_thread(s).filter(|thread| thread.active_subagent.is_some() && !folded);
    let drill_w = if drill_thread.is_some() { DRILL_W } else { 0.0 };
    // The child chat already lives as a canvas card; bind the panel's rows and
    // composer to that thread so sends stream into the dock.
    let drill_chat = drill_thread.and_then(|thread| {
        thread.active_subagent.as_ref().and_then(|drill| {
            let card_id = format!("chat:{}:{}", drill.workspace_id, drill.child_id);
            if !s.canvas.sessions.threads.contains_key(&card_id) {
                return None;
            }
            s.canvas
                .sessions
                .order
                .iter()
                .position(|id| id == &card_id)
                .map(Thread::Card)
        })
    });
    let stage_w = (w
        - if rail_visible { RAIL_W } else { 0.0 }
        - if panel_visible { PANEL_W } else { 0.0 }
        - drill_w)
        .max(320.0);
    let stage_h = (h
        - TOPBAR_H
        - if folded {
            MOBILE_NAV_H + safe_bottom
        } else {
            0.0
        })
    .max(240.0);
    let mut stage = col()
        .key("canvas-stack")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        // `absolute inset-0 ... px-3 pt-3 sm:px-5`: the map is inset from the
        // stage edges while the chat overlay spans the full stage width.
        .child(
            col()
                .key("canvas-pad")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .child(canvas_body::<ShellState>(&s.canvas)),
        );
    if let Some(overlay) = home_chat_overlay(s, stage_w, stage_h, h) {
        stage = stage.child(overlay);
    }
    let mut main = row()
        .key("stark-main")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(ink("#0a0a0a"));
    if rail_visible {
        // Desktop: icon rail + docked panel. Below `md` both disappear and
        // The panel becomes an on-demand drawer (the reference UI `House.tsx`).
        main = main.child(rail);
        if panel_visible {
            main = main.child(side_panel(s, PANEL_W));
        }
    }
    main = main.child(stage);
    if let Some(thread) = drill_thread {
        if let Some(panel) = crate::rune_transcript::subagent_panel::<ShellState>(
            thread,
            Thread::Open,
            drill_chat,
            DRILL_W,
        ) {
            main = main.child(panel);
        }
    }
    let mut root = col()
        .key("stark-shell")
        .w_full()
        .h_full()
        .fill(ink("#0a0a0a"))
        .hover_motion_within(crate::motion::HOVER, crate::motion::HOVER)
        .on_key(
            |s: &mut ShellState, key: &rune::KeyEvent, _tx: &mut rune::Tx| {
                // The reference UI closes the phone drawer on Escape; modals keep their own
                // handlers because they sit above this root.
                if key.key == rune::Key::Escape && s.mobile_panel_open {
                    s.mobile_panel_open = false;
                    return true;
                }
                false
            },
        )
        .child(topbar)
        .child(main);
    if folded {
        root = root.child(mobile_nav(s, safe_bottom));
        if s.mobile_panel_open {
            // The drawer opens below the titlebar so its menu button stays
            // visible and can close it again.
            let drawer_w = (w * 0.84).clamp(240.0, 320.0);
            let body_h = (h - TOPBAR_H).max(0.0);
            root = root
                .child(
                    rune::el()
                        .key("side-backdrop")
                        .absolute(0.0, TOPBAR_H)
                        .size(w, body_h)
                        .fill(rgba(0.0, 0.0, 0.0, 0.6))
                        .backdrop_blur(4.0)
                        .on_press_enter(|s: &mut ShellState| s.mobile_panel_open = false),
                )
                .child(
                    rune::el()
                        .key("side-drawer")
                        .absolute(0.0, TOPBAR_H)
                        .w(drawer_w)
                        .h(body_h)
                        .transition(crate::motion::drawer_enter())
                        .box_shadow(rune::Shadow {
                            x: 18.0,
                            y: 0.0,
                            blur: 60.0,
                            spread: 0.0,
                            color: rgba(0.0, 0.0, 0.0, 0.45),
                            inset: false,
                        })
                        .child(side_panel(s, drawer_w)),
                );
        }
    }
    if s.canvas.agents.selected.is_some() {
        root = root.child(crate::agents::agent_sheet::<ShellState>(&s.canvas.agents));
    }
    if let Some(file) = s.transcript.attachment_preview.clone() {
        root = root.child(crate::rune_transcript::attachment_lightbox::<ShellState>(
            &file,
            w,
            h,
            &s.transcript.workspace_id,
            &s.transcript.chat_id,
            |s: &mut ShellState| {
                s.transcript.attachment_preview = None;
            },
        ));
    }
    for sheet in active_chat_sheets(s) {
        root = root.child(sheet);
    }
    if s.canvas.projects.dialog.open {
        root = root.child(crate::projects::new_project_dialog::<ShellState>(
            &s.canvas.projects,
            w,
            h,
        ));
    }
    if s.settings.open {
        root = root.child(crate::settings::settings_sheet::<ShellState>(
            &s.settings,
            w,
            h,
        ));
    }
    if s.canvas.projects.group.is_open() {
        root = root.child(crate::group::modal::group_modal::<ShellState>(
            &s.canvas.projects.group,
            w,
            h,
        ));
    }
    if s.pairing.locked {
        root = root.child(pairing_gate(s, w, h));
    }
    if let Some(toast) = update_toast(
        s,
        w,
        h,
        if folded {
            MOBILE_NAV_H + safe_bottom
        } else {
            0.0
        },
    ) {
        root = root.child(toast);
    }
    root
}

/// Stop audio owned by the transcript that is no longer on screen. The home
/// thread is `main`; an open card sheet is that card's id.
fn sync_audio_owner(s: &mut ShellState) -> bool {
    let owner = s
        .canvas
        .sessions
        .sheet
        .clone()
        .unwrap_or_else(|| "main".to_string());
    if owner == s.audio_owner {
        return false;
    }
    match s.audio_owner.as_str() {
        "main" => {
            s.transcript.audio.stop_all();
            s.personal.audio.stop_all();
        }
        id => {
            if let Some(thread) = s.canvas.sessions.threads.get_mut(id) {
                thread.audio.stop_all();
            }
        }
    }
    s.audio_owner = owner;
    true
}

/// At least one clip is loading or playing; the shell polls those at 4 Hz so
/// The scrub bar and clock advance without a continuous animation.
fn poll_audio(s: &mut ShellState) -> bool {
    let active = s.transcript.audio.has_active()
        || s.personal.audio.has_active()
        || s.canvas
            .sessions
            .threads
            .values()
            .any(|thread| thread.audio.has_active());
    let revision = s.transcript.audio.revision()
        ^ s.personal.audio.revision()
        ^ s.canvas
            .sessions
            .threads
            .values()
            .map(|thread| thread.audio.revision())
            .fold(0, u64::wrapping_add);
    let changed = revision != s.audio_rev;
    s.audio_rev = revision;
    changed || active
}

/// True when restarting cannot lose an in-flight run or typed message.
fn update_restart_safe(s: &ShellState) -> bool {
    if s.transcript.live || s.personal.live {
        return false;
    }
    if s.canvas.sessions.threads.values().any(|thread| thread.live) {
        return false;
    }
    if !s.transcript.edit.text.trim().is_empty()
        || !s.personal.edit.text.trim().is_empty()
        || s.canvas
            .inputs
            .values()
            .any(|input| !input.text.trim().is_empty())
        || s.canvas
            .sessions
            .threads
            .values()
            .any(|thread| !thread.edit.text.trim().is_empty())
        || s.canvas
            .sessions
            .threads
            .values()
            .any(|thread| !thread.draft_attachments.is_empty())
        || s.canvas
            .card_attachments
            .values()
            .any(|files| !files.is_empty())
    {
        return false;
    }
    true
}

/// Global updater toast: matches the reference UI's `UpdateToast` placement and keeps the
/// downloading/ready states visible until they resolve.
fn update_toast(s: &ShellState, w: f32, h: f32, bottom_inset: f32) -> Option<El> {
    let update = &s.settings.update;
    if matches!(update.phase, UpdatePhase::Idle | UpdatePhase::Unsupported) {
        return None;
    }
    if matches!(update.phase, UpdatePhase::Error | UpdatePhase::UpToDate)
        && update
            .shown_at
            .is_some_and(|at| crate::clock::now_millis().saturating_sub(at) > 6_000)
    {
        return None;
    }
    let tone = match update.phase {
        UpdatePhase::Error => "#f87171",
        UpdatePhase::UpToDate => "#a1a1aa",
        _ => "#e4e4e7",
    };
    let width = 320.0_f32.min((w - 24.0).max(220.0));
    let x = (w - width - 12.0).max(12.0);
    let y = (h - 12.0 - 44.0 - bottom_inset).max(12.0);
    let mut toast = row()
        .key("update-toast")
        .absolute(x, y)
        .w(width)
        .min_h(40.0)
        .items_center()
        .gap(8.0)
        .px(12.0)
        .py(8.0)
        .radius(12.0)
        .border(1.0, ink("#ffffff1a"))
        .border_layout()
        .fill(ink("#15151af2"))
        .backdrop_blur(10.0)
        .box_shadow(rune::Shadow {
            x: 0.0,
            y: 14.0,
            blur: 36.0,
            spread: 0.0,
            color: ink("#00000073"),
            inset: false,
        })
        .role("status")
        .live_region(rune::LiveRegion::Polite);
    let glyph = match update.phase {
        UpdatePhase::Downloading => icon_refresh(13.0, ink(tone)),
        UpdatePhase::UpToDate => crate::icons::circle_check(13.0, ink(tone)),
        UpdatePhase::Error => crate::icons::triangle_alert(13.0, ink(tone)),
        _ => icon_refresh(13.0, ink(tone)),
    };
    if update.phase == UpdatePhase::Downloading {
        toast = toast.child(
            rune::el()
                .size(13.0, 13.0)
                .inert()
                .looping(rune::LoopSpec::rotation(0.9))
                .child(rune::el().size(13.0, 13.0).inert().shape(glyph)),
        );
    } else {
        toast = toast.child(rune::el().size(13.0, 13.0).inert().shape(glyph));
    }
    toast = toast.child(
        text(update.message.clone())
            .key("update-toast-message")
            .grow(1.0)
            .min_w(0.0)
            .font(rune::tok::TextStyle::new(11.5, 400.0))
            .wrap()
            .color(ink(tone)),
    );
    if update.phase == UpdatePhase::Ready {
        toast = toast.child(
            rune::el()
                .key("update-toast-restart")
                .role("button")
                .focusable(true)
                .label("Restart now")
                .help("Restart now")
                .h(24.0)
                .px(8.0)
                .radius(6.0)
                .center()
                .hover(ink("#ffffff14"))
                .hover_group()
                .on_press_tx_enter(|_s: &mut ShellState, tx: &mut rune::Tx| tx.quit())
                .child(
                    text("Restart")
                        .font(rune::tok::TextStyle::new(10.5, 600.0))
                        .color(ink("#e4e4e7")),
                ),
        );
    }
    Some(toast)
}

/// Full-screen phone gate, shown when the server answered 401. The desktop
/// is the pairing host, so this only ever mounts in the web build.
fn pairing_gate(s: &ShellState, w: f32, h: f32) -> El {
    let card_w = (w - 48.0).clamp(280.0, 384.0);
    let can_submit = !s.pairing.busy && !s.pairing.token.text.trim().is_empty();
    let mut card = col()
        .key("pairing-gate-card")
        .w(card_w)
        .gap(18.0)
        .child(
            rune::el()
                .size(44.0, 44.0)
                .radius(14.0)
                .border(1.0, ink("#27272a"))
                .fill(ink("#18181b"))
                .center()
                .child(
                    rune::el()
                        .size(20.0, 20.0)
                        .inert()
                        .shape(icon_lock(20.0, ink("#e4e4e7"))),
                ),
        )
        .child(
            text("Pair this device".to_string())
                .font(rune::tok::TextStyle::new(18.0, 600.0))
                .color(ink("#fafafa")),
        )
        .child(
            text(
                "Enter the pairing token shown on the desktop, or open the link it gives you."
                    .to_string(),
            )
            .font(rune::tok::TextStyle::new(13.0, 400.0))
            .wrap()
            .color(ink("#a1a1aa")),
        );
    if let Some(error) = &s.pairing.error {
        card = card.child(
            text(error.clone())
                .key("pairing-error")
                .role("alert")
                .live_region(rune::LiveRegion::Assertive)
                .font(rune::tok::TextStyle::new(12.0, 400.0))
                .wrap()
                .color(ink("#f87171")),
        );
    }
    let mut input = text_input(&s.pairing.token, rune::tok::TextStyle::new(13.0, 400.0))
        .key("pairing-token")
        .grow(1.0)
        .h(46.0)
        .px(12.0)
        .radius(10.0)
        .fill(ink("#00000080"))
        .border(1.0, ink("#27272a"))
        .border_layout()
        .caret(ink("#e4e4e7"), crate::motion::TEXT_SELECTION)
        .placeholder("Pairing token", ink("#52525b"))
        .on_edit(|s: &mut ShellState| &mut s.pairing.token);
    input = input.on_input_key(
        |s: &mut ShellState, key: &rune::KeyEvent, tx: &mut rune::Tx| {
            if key.key != rune::Key::Enter || key.mods.shift {
                return false;
            }
            submit_pairing(s, tx);
            true
        },
    );
    let send = rune::el()
        .key("pairing-submit")
        .role("button")
        .focusable(true)
        .label("Continue")
        .size(46.0, 46.0)
        .radius(10.0)
        .center()
        // Palette lock: white primary when submittable, dark grey when not.
        .fill(ink(if can_submit { "#f4f4f5" } else { "#3f3f46" }))
        .opacity(if can_submit { 1.0 } else { 0.45 })
        .disabled(!can_submit)
        .on_press_tx_enter(move |s: &mut ShellState, tx: &mut rune::Tx| {
            submit_pairing(s, tx);
        })
        .child(rune::el().size(18.0, 18.0).inert().shape(icon_arrow_right(
            18.0,
            ink(if can_submit { "#09090b" } else { "#a1a1aa" }),
        )));
    card = card.child(
        row()
            .w_full()
            .items_center()
            .gap(10.0)
            .child(input)
            .child(send),
    );
    rune::el()
        .key("pairing-gate")
        .absolute(0.0, 0.0)
        .size(w, h)
        .fill(ink("#0a0a0c"))
        .center()
        .child(card)
}

fn submit_pairing(s: &mut ShellState, _tx: &mut rune::Tx) {
    if s.pairing.busy {
        return;
    }
    let token = s.pairing.token.text.trim().to_string();
    if token.is_empty() {
        return;
    }
    s.pairing.busy = true;
    s.pairing.error = None;
    #[cfg(target_arch = "wasm32")]
    {
        crate::transport::set_pairing_token(&token);
        let (tx, rx) = std::sync::mpsc::channel();
        s.pairing.inbox = Some(rx);
        wasm_bindgen_futures::spawn_local(async move {
            let result = crate::transport::fetch_json("/api/health")
                .await
                .map(|_| ());
            let _ = tx.send(result);
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        s.pairing.busy = false;
    }
}

fn icon_lock(size: f32, color: rune::Color) -> Shape {
    let x0 = size * (3.0 / 24.0);
    let x1 = size * (21.0 / 24.0);
    let y0 = size * (10.0 / 24.0);
    let y1 = size * (21.0 / 24.0);
    let body = Path::new()
        .move_to([x0, y0])
        .line_to([x1, y0])
        .line_to([x1, y1])
        .line_to([x0, y1])
        .close();
    let shackle = Path::new()
        .move_to([size * (7.0 / 24.0), size * (10.0 / 24.0)])
        .line_to([size * (7.0 / 24.0), size * (7.0 / 24.0)])
        .cubic_to(
            [size * (7.0 / 24.0), size * (4.0 / 24.0)],
            [size * (10.0 / 24.0), size * (3.0 / 24.0)],
            [size * 0.5, size * (3.0 / 24.0)],
        )
        .cubic_to(
            [size * (14.0 / 24.0), size * (3.0 / 24.0)],
            [size * (17.0 / 24.0), size * (4.0 / 24.0)],
            [size * (17.0 / 24.0), size * (7.0 / 24.0)],
        )
        .line_to([size * (17.0 / 24.0), size * (10.0 / 24.0)]);
    Shape::new()
        .stroke_path(&body, crate::lucide_stroke(size), LineCap::Round, color)
        .stroke_path(&shackle, crate::lucide_stroke(size), LineCap::Round, color)
        .view_box(24.0, 24.0)
}

fn icon_arrow_right(size: f32, color: rune::Color) -> Shape {
    let p = Path::new()
        .move_to([5.0, 12.0])
        .line_to([19.0, 12.0])
        .move_to([12.0, 5.0])
        .line_to([19.0, 12.0])
        .line_to([12.0, 19.0]);
    Shape::new()
        .stroke_path(&p, crate::lucide_stroke(size), LineCap::Round, color)
        .view_box(24.0, 24.0)
}

/// `poll` drains every session thread (runs + notes) whatever panel is up
/// and refreshes status rooms at most every 10–15s, so runs started from the
/// dock keep streaming on the map. Card `running` flags mirror thread liveness.
pub fn shell_poll(s: &mut ShellState, clock: f64, tx: &mut rune::Tx) -> bool {
    let mut changed = poll_pairing(s);
    #[cfg(target_arch = "wasm32")]
    {
        changed |= apply_web_picks(s);
        changed |= apply_web_uploads(s);
        if crate::transport::web_busy() {
            tx.wake_after(std::time::Duration::from_millis(250));
        }
    }
    // Keep the canvas stage sized to the live window (the reference UI's container
    // drives `fitView`). While the camera is auto it refits on resize.
    let (win_w, win_h, safe_bottom) = unsafe { (WINDOW_W, WINDOW_H, WINDOW_SAFE_BOTTOM) };
    let folded = win_w < PANEL_FOLD_W;
    let drill_open =
        !folded && drill_thread(s).is_some_and(|thread| thread.active_subagent.is_some());
    let stage_w = (win_w
        - if folded { 0.0 } else { RAIL_W }
        - if !folded && s.panel_open {
            PANEL_W
        } else {
            0.0
        }
        - if drill_open { DRILL_W } else { 0.0 }
        - 40.0)
        .max(320.0);
    let stage_h = (win_h
        - TOPBAR_H
        - 12.0
        - if folded {
            MOBILE_NAV_H + safe_bottom
        } else {
            0.0
        })
    .max(240.0);
    changed |= s.canvas.set_viewport(stage_w, stage_h);
    changed |= drain_run_inbox(&mut s.transcript, clock, tx);
    changed |= drain_run_inbox(&mut s.personal, clock, tx);
    changed |= crate::notes::poll_notes_state(&mut s.transcript.notes);
    changed |= crate::notes::poll_notes_state(&mut s.personal.notes);
    changed |= crate::rune_transcript::poll_provider_state(&mut s.transcript, clock);
    changed |= crate::rune_transcript::poll_provider_state(&mut s.personal, clock);
    changed |= crate::rune_transcript::poll_models_state(&mut s.transcript, clock);
    changed |= crate::rune_transcript::poll_models_state(&mut s.personal, clock);
    // Canvas threads (home card dock) poll the same services once the real
    // app entry has enabled live chats; unit tests stay offline.
    let services_auto = s.chats.auto;
    changed |= crate::agent_config::poll_agent_config(&mut s.transcript.agent_config);
    changed |= crate::agent_config::poll_agent_config(&mut s.personal.agent_config);
    changed |= crate::provider_keys::poll_provider_keys(&mut s.transcript.provider_keys);
    changed |= crate::provider_keys::poll_provider_keys(&mut s.personal.provider_keys);
    // Only the visible card's dock shows provider/models (the reference UI mounts one
    // PromptDock), so poll just that thread — not every open tab.
    if let Some(thread) = s.canvas.sheet_thread_mut() {
        if services_auto {
            thread.services_auto = true;
        }
        changed |= crate::rune_transcript::poll_provider_state(thread, clock);
        changed |= crate::rune_transcript::poll_models_state(thread, clock);
        changed |= crate::provider_keys::poll_provider_keys(&mut thread.provider_keys);
    }
    // Pending questions poll every 1.5s (`usePendingAsk`) for every open
    // surface. Canvas cards are visible in the map even while another chat is
    // active, so their compact Ask panels must stay live in the background.
    changed |= poll_thread_ask(&mut s.transcript, clock, tx);
    changed |= poll_thread_ask(&mut s.personal, clock, tx);
    // Session threads: drain + sync card flags + per-card ask polling.
    let ids: Vec<String> = s.canvas.sessions.order.clone();
    for id in &ids {
        if let Some(thread) = s.canvas.sessions.threads.get_mut(id) {
            if services_auto {
                thread.services_auto = true;
            }
            changed |= drain_run_inbox(thread, clock, tx);
            changed |= crate::notes::poll_notes_state(&mut thread.notes);
            changed |= crate::agent_config::poll_agent_config(&mut thread.agent_config);
            changed |= crate::provider_keys::poll_provider_keys(&mut thread.provider_keys);
            changed |= poll_thread_ask(thread, clock, tx);
            let live = thread.live;
            if let Some(card) = s.canvas.cards.iter_mut().find(|c| c.id == *id) {
                if card.running != live {
                    card.running = live;
                    changed = true;
                }
            }
        }
    }
    // A `chat` SSE frame names the real id for a local draft card. Rekey the
    // card/tab/thread now so history, abort, rewind and the sidebar all use
    // The server identity. Main/personal threads have no map card.
    let rekeys: Vec<(String, String, String)> = ids
        .iter()
        .filter_map(|id| {
            let (workspace, chat) = s.canvas.sessions.threads.get(id)?.pending_rekey.clone()?;
            Some((id.clone(), workspace, chat))
        })
        .collect();
    for (id, workspace, chat) in rekeys {
        if s.canvas.rekey_local_chat(&id, &workspace, &chat) {
            changed = true;
        } else if let Some(thread) = s.canvas.sessions.threads.get_mut(&id) {
            thread.pending_rekey = None;
        }
    }
    for thread in [&mut s.transcript, &mut s.personal] {
        if thread.pending_rekey.take().is_some() {
            changed = true;
        }
    }
    // Creating a chat / rewinding refreshes the sidebar (`refreshList`).
    let wants_refresh = s.transcript.refresh_list
        || s.personal.refresh_list
        || s.canvas
            .sessions
            .threads
            .values()
            .any(|thread| thread.refresh_list);
    if wants_refresh && !s.chats.loading {
        s.transcript.refresh_list = false;
        s.personal.refresh_list = false;
        for thread in s.canvas.sessions.threads.values_mut() {
            thread.refresh_list = false;
        }
        crate::chats::refresh(&mut s.chats);
        changed = true;
    }
    changed |= s.canvas.voice.drain(1.0 / 60.0);
    changed |= poll_house(s, clock);
    changed |= poll_memory(s, clock);
    changed |= poll_projects(s, clock);
    changed |= crate::agents::poll_agents_state(&mut s.canvas.agents, clock);
    changed |= crate::settings::poll_settings(&mut s.settings);
    s.settings.auto_update(clock);
    if let Some(left) = s.settings.next_auto_check_in(clock) {
        tx.wake_after(std::time::Duration::from_secs_f64(left.max(0.05)));
    }
    changed |= sync_audio_owner(s);
    if poll_audio(s) {
        tx.wake_after(std::time::Duration::from_millis(250));
        changed = true;
    }
    if s.settings.update_restart_ready() && update_restart_safe(s) {
        // The launcher sees `pending.json`, applies it while this process
        // exits, then starts the new UI and server together.
        tx.quit();
        return true;
    }
    changed |= poll_cloud_state(&mut s.cloud, clock);
    changed |= poll_chats(s, clock);
    // Async results arrive on channels, not on input: keep polling until
    // every in-flight fetch has landed (otherwise a quiet screen can sit on
    // stale state until the next click).
    let busy = s.chats.loading
        || (s.chats.auto && !s.chats.loaded)
        || s.transcript.provider.loading
        || s.transcript.models_loading
        || s.personal.provider.loading
        || s.personal.models_loading
        || s.canvas
            .sessions
            .threads
            .values()
            .any(|t| t.provider.loading || t.models_loading)
        || s.house.loading
        || s.memory.loading
        || s.cloud.loading
        || s.cloud.board_loading
        || s.canvas.projects.loading
        || s.canvas.agents.loading
        || s.settings.loading
        || s.settings.busy
        || s.settings.backup_busy;
    if busy {
        tx.wake_after(std::time::Duration::from_millis(100));
    }
    if s.chats.auto {
        // Keep the 2.5s run poll (and the 30s sessions poll) ticking on an
        // otherwise idle screen, exactly like the reference UI's intervals.
        let run_left = (2.5 - (clock - s.chats.last_run_poll)).max(0.01);
        tx.wake_after(std::time::Duration::from_secs_f64(run_left));
        if s.chats.loaded {
            let sessions_left = (30.0 - (clock - s.chats.last_poll)).max(0.01);
            tx.wake_after(std::time::Duration::from_secs_f64(sessions_left));
        }
    }
    if let Some(deadline) = s.canvas.sessions.next_deadline() {
        let ms = deadline.saturating_sub(crate::clock::now_millis()).max(1);
        tx.wake_after(std::time::Duration::from_millis(ms));
    }
    if s.canvas.sessions.deleting.is_some() {
        // ~60 fps while the row fades out.
        tx.wake_after(std::time::Duration::from_millis(16));
    }
    if trace_animating(s) {
        // ~60 fps while a trace body fades in or out.
        tx.wake_after(std::time::Duration::from_millis(16));
        changed = true;
    }
    if s.canvas.projects.group.is_open() {
        let g = &s.canvas.projects.group;
        let wait = if g.loading || g.sending || g.busy_item.is_some() || g.op_busy.is_some() {
            100
        } else {
            1000
        };
        tx.wake_after(std::time::Duration::from_millis(wait));
    }
    if tick_voice_breathe(s) {
        // `voice-breathe` is a 1.2 s scale/opacity keyframe. Rune has no
        // infinite scale loop; publish it into the Live tracks at 60 Hz while
        // a mic is live, then stop waking when the session ends.
        tx.wake_after(std::time::Duration::from_millis(16));
    }
    if let Some(at) = s.chats.ui_dirty_at {
        let left = (0.4 - (clock - at)).max(0.01);
        tx.wake_after(std::time::Duration::from_secs_f64(left));
    }
    changed
}

/// Whether any mounted transcript has a trace body inside its 180 ms open or
/// close fade. The shell forces a rebuild each frame so the opacity lands on
/// The poll clock rather than a presence spring.
fn trace_animating(s: &ShellState) -> bool {
    let now = crate::rune_transcript::frame_millis();
    let closing = |thread: &RuneTranscriptState| {
        thread.rows.iter().any(|row| {
            row.trace_anim_at
                .is_some_and(|at| now.saturating_sub(at) < 180)
        })
    };
    closing(&s.transcript)
        || closing(&s.personal)
        || s.canvas.sessions.threads.values().any(closing)
}

/// Publish the `voice-breathe` pose for every live voice bus. Clones share
/// their `Live` sources, so ticking the transcript, personal, canvas and
/// card-thread buses is cheap and consistent.
fn tick_voice_breathe(s: &ShellState) -> bool {
    let now = crate::clock::now_millis();
    let mut live = s.transcript.voice.tick_breathe(now);
    live |= s.personal.voice.tick_breathe(now);
    live |= s.canvas.voice.tick_breathe(now);
    for thread in s.canvas.sessions.threads.values() {
        live |= thread.voice.tick_breathe(now);
    }
    live
}

/// Start uploads for browser-picked files. Fresh cards create their chat
/// first, matching the reference UI's create-before-upload send path.
#[cfg(target_arch = "wasm32")]
fn apply_web_picks(s: &mut ShellState) -> bool {
    use crate::transport::WebTarget;
    let picks = crate::transport::take_web_picks();
    if picks.is_empty() {
        return false;
    }
    for pick in picks {
        let source = match &pick.target {
            WebTarget::Main => Some(&s.transcript),
            WebTarget::Open => s.canvas.sheet_thread().or(Some(&s.transcript)),
            WebTarget::Card(id) => s.canvas.sessions.threads.get(id),
        };
        let Some(thread) = source else {
            continue;
        };
        let chat_id = if thread.chat_ready {
            thread.chat_id.clone()
        } else {
            String::new()
        };
        crate::transport::spawn_web_upload(
            pick,
            thread.workspace_id.clone(),
            chat_id,
            thread.title.clone(),
        );
    }
    true
}

#[cfg(target_arch = "wasm32")]
fn apply_web_upload_to_thread(
    thread: &mut RuneTranscriptState,
    workspace_id: &str,
    chat_id: &str,
    created: bool,
    result: Result<Vec<crate::chats::ChatAttachment>, String>,
    append_to_draft: bool,
) -> Result<Vec<crate::chats::ChatAttachment>, String> {
    if created && !chat_id.is_empty() {
        thread.workspace_id = workspace_id.to_string();
        thread.chat_id = chat_id.to_string();
        thread.chat_ready = true;
        thread.pending_rekey = Some((workspace_id.to_string(), chat_id.to_string()));
        thread.refresh_list = true;
    }
    match result {
        Ok(files) => {
            if append_to_draft {
                thread.draft_attachments.extend(files.clone());
            }
            thread.attachments_error = None;
            Ok(files)
        }
        Err(error) => {
            thread.attachments_error = Some(error.clone());
            Err(error)
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn apply_web_uploads(s: &mut ShellState) -> bool {
    use crate::transport::WebTarget;
    let uploads = crate::transport::take_web_uploads();
    if uploads.is_empty() {
        return false;
    }
    for upload in uploads {
        let crate::transport::WebUpload {
            target,
            workspace_id,
            chat_id,
            created,
            result,
        } = upload;
        match target {
            WebTarget::Main => {
                let _ = apply_web_upload_to_thread(
                    &mut s.transcript,
                    &workspace_id,
                    &chat_id,
                    created,
                    result,
                    true,
                );
            }
            WebTarget::Open => {
                if let Some(thread) = s.canvas.sheet_thread_mut() {
                    let _ = apply_web_upload_to_thread(
                        thread,
                        &workspace_id,
                        &chat_id,
                        created,
                        result,
                        true,
                    );
                } else {
                    let _ = apply_web_upload_to_thread(
                        &mut s.transcript,
                        &workspace_id,
                        &chat_id,
                        created,
                        result,
                        true,
                    );
                }
            }
            WebTarget::Card(card_id) => {
                let applied = {
                    let thread = s.canvas.thread_mut(&card_id);
                    apply_web_upload_to_thread(
                        thread,
                        &workspace_id,
                        &chat_id,
                        created,
                        result,
                        false,
                    )
                };
                match applied {
                    Ok(files) => {
                        s.canvas
                            .card_attachments
                            .entry(card_id.clone())
                            .or_default()
                            .extend(files);
                        s.canvas.card_attachment_errors.remove(&card_id);
                    }
                    Err(error) => {
                        s.canvas.card_attachment_errors.insert(card_id, error);
                    }
                }
            }
        }
    }
    true
}

/// Browser pairing gate state. A failed API request flips the gate; a token
/// probe either unlocks the shell and refreshes chats or reports the token.
fn poll_pairing(s: &mut ShellState) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = s;
        false
    }
    #[cfg(target_arch = "wasm32")]
    {
        let mut changed = false;
        if crate::transport::auth_rejected() && !s.pairing.locked {
            s.pairing.locked = true;
            s.pairing.token.focused = true;
            changed = true;
        }
        if let Some(result) = s.pairing.inbox.as_ref().and_then(|rx| rx.try_recv().ok()) {
            s.pairing.inbox = None;
            s.pairing.busy = false;
            match result {
                Ok(()) => {
                    crate::transport::clear_auth_rejected();
                    s.pairing.locked = false;
                    s.pairing.error = None;
                    s.pairing.token.text.clear();
                    s.pairing.token.anchor = 0;
                    s.pairing.token.focus = 0;
                    crate::chats::refresh(&mut s.chats);
                }
                Err(_) => {
                    s.pairing.error = Some(
                        "That pairing token was not accepted. Check the token on the desktop."
                            .to_string(),
                    );
                }
            }
            changed = true;
        }
        if s.pairing.busy {
            changed = true;
        }
        changed
    }
}

/// One thread's pending-Ask poll. Keeping the 1.5s cadence and the 100ms
/// in-flight ticket here means every visible surface schedules its own wake,
/// including cards behind the active chat.
fn poll_thread_ask(thread: &mut RuneTranscriptState, clock: f64, tx: &mut rune::Tx) -> bool {
    let changed = crate::ask::poll_ask_state(thread, clock);
    if crate::ask::ask_busy(thread) {
        tx.wake_after(std::time::Duration::from_millis(100));
    } else if let Some(ms) = crate::ask::poll_wait_ms(thread, clock) {
        tx.wake_after(std::time::Duration::from_millis(ms));
    }
    changed
}

/// Drain the real-chat stream: adopt snapshots, apply per-chat history, and
/// auto-load once on the real app entry. Mirrors `useSessions`' two cadences:
/// a 30s sessions diff and a 2.5s active-run poll.
fn poll_chats(s: &mut ShellState, clock: f64) -> bool {
    use crate::chats::ChatsMsg;
    let mut changed = false;
    // Persist finished live messages, then refresh the canonical history.
    changed |= crate::chats::drain_pending_saves(&mut s.chats, &mut s.transcript);
    changed |= crate::chats::drain_refresh(&mut s.chats, &mut s.transcript);
    changed |= crate::chats::drain_pending_saves(&mut s.chats, &mut s.personal);
    changed |= crate::chats::drain_refresh(&mut s.chats, &mut s.personal);
    let ids: Vec<String> = s.canvas.sessions.threads.keys().cloned().collect();
    for id in ids {
        if let Some(thread) = s.canvas.sessions.threads.get_mut(&id) {
            changed |= crate::chats::drain_pending_saves(&mut s.chats, thread);
            changed |= crate::chats::drain_refresh(&mut s.chats, thread);
        }
    }
    while let Some(msg) = s.chats.inbox.as_ref().and_then(|rx| rx.try_recv().ok()) {
        match msg {
            ChatsMsg::Snapshot(Ok(snapshot)) => {
                if s.chats.booted {
                    // Later polls keep local tabs/active/pins/filter and only
                    // reconcile server data (the reference UI `useSessions` poll).
                    s.canvas.apply_chat_snapshot(&snapshot);
                } else {
                    // Boot: the persisted layout wins once. A genuinely empty
                    // account also clears the demo transcript, so the always-
                    // mounted home card shows its empty state, not the sample
                    // conversation.
                    s.canvas.adopt_boot_snapshot(&snapshot);
                    if snapshot.open_tabs.is_empty() && snapshot.chats.is_empty() {
                        s.transcript = RuneTranscriptState::empty_home();
                        s.personal = RuneTranscriptState::empty_home();
                    }
                    s.chats.booted = true;
                }
                s.chats.snapshot = snapshot;
                s.chats.loaded = true;
                s.chats.loading = false;
                s.chats.error = None;
                s.chats.last_poll = clock;
                changed = true;
            }
            ChatsMsg::Snapshot(Err(message)) => {
                s.chats.error = Some(message);
                s.chats.loading = false;
                changed = true;
            }
            ChatsMsg::Messages { key, rows } => {
                if !s.canvas.apply_chat_messages(&key, &rows) {
                    // A new chat created from a local card has no matching
                    // card id yet; land its history on the visible thread
                    // whose (workspace, chat) now matches.
                    if let Some(thread) = s.canvas.sheet_thread_mut() {
                        if format!("{}:{}", thread.workspace_id, thread.chat_id) == key {
                            thread.rows =
                                rows.iter().map(crate::rune_canvas::row_from_chat).collect();
                            thread.sync_usage_from_rows();
                            thread.live = false;
                            thread.aborting = false;
                        }
                    }
                }
                changed = true;
            }
            ChatsMsg::RefreshNeeded => {
                // Optimistic rename/delete failed: let the server's list win.
                if !s.chats.loading {
                    crate::chats::refresh(&mut s.chats);
                }
                changed = true;
            }
            ChatsMsg::ActiveRun(Ok(run)) => {
                s.chats.active_run = Some(run);
                s.chats.run_loading = false;
                changed |= sync_running_tab(s);
            }
            ChatsMsg::ActiveRun(Err(_)) => {
                // The reference UI catches the poll and keeps the last known dot.
                s.chats.run_loading = false;
            }
            ChatsMsg::Saved(result) => {
                s.chats.ui_saving = false;
                if result.is_err() {
                    // `saveUiState` swallows failures; the next change retries.
                    s.chats.ui_saved.clear();
                    s.chats.ui_dirty_at = None;
                }
                changed = true;
            }
        }
    }
    // Only start history fetches when the boot snapshot is not in flight:
    // all fetches share one result stream.
    if !s.chats.loading && !s.canvas.pending_chat_fetches.is_empty() {
        for (ws, chat) in std::mem::take(&mut s.canvas.pending_chat_fetches) {
            crate::chats::refresh_messages(&mut s.chats, &ws, &chat);
        }
        changed = true;
    }
    if s.chats.auto
        && !s.chats.loaded
        && !s.chats.loading
        && (s.chats.error.is_none() || clock - s.chats.last_try > 10.0)
    {
        s.chats.last_try = clock;
        crate::chats::refresh(&mut s.chats);
        changed = true;
    }
    // 30s sessions poll (`SESSIONS_POLL_MS`); the snapshot handler prunes and
    // auto-opens only when the list changed.
    if s.chats.auto && s.chats.loaded && !s.chats.loading && clock - s.chats.last_poll > 30.0 {
        s.chats.last_poll = clock;
        crate::chats::refresh(&mut s.chats);
        changed = true;
    }
    // 2.5s active-run poll (`RUN_POLL_MS`) — one pipeline at a time.
    if s.chats.auto && !s.chats.run_loading && clock - s.chats.last_run_poll > 2.5 {
        s.chats.last_run_poll = clock;
        crate::chats::refresh_active_run(&mut s.chats);
        changed = true;
    }
    changed |= tick_sidebar_editing(s);
    changed |= tick_chat_deletes(s, clock);
    changed |= poll_ui_state(s, clock);
    changed
}

/// Map `/api/runs/active` to the open tab carrying the dot.
fn sync_running_tab(s: &mut ShellState) -> bool {
    let running = s.canvas.running_tab_for(s.chats.active_run.as_ref());
    if s.canvas.running_tab == running {
        return false;
    }
    s.canvas.running_tab = running;
    true
}

/// Inline rename blur commit (`HouseSidebar` input `onBlur`): Rune mirrors the
/// field's focus into the model, so losing it signals the commit.
fn tick_sidebar_editing(s: &mut ShellState) -> bool {
    crate::rune_canvas::settle_sidebar_editing(s)
}

/// Two-step delete: first press arms a 2.5s confirm, the second fades the row
/// for 220ms before the optimistic removal + DELETE (the reference UI `HouseSidebar`).
fn tick_chat_deletes(s: &mut ShellState, _clock: f64) -> bool {
    let now = crate::clock::now_millis();
    // The 220ms fade recomputes opacity from the wall clock, so keep the
    // sidebar rebuilding while it runs.
    let mut changed = s.canvas.sessions.deleting.is_some();
    if let Some((_, at)) = &s.canvas.sessions.confirm_delete {
        if now >= *at {
            s.canvas.sessions.confirm_delete = None;
            changed = true;
        }
    }
    let due = s
        .canvas
        .sessions
        .deleting
        .as_ref()
        .filter(|(_, _, at)| now >= *at)
        .map(|(key, _, _)| key.clone());
    if let Some(key) = due {
        s.canvas.sessions.deleting = None;
        if let Some((ws, chat)) = key.split_once(':') {
            s.canvas.remove_chat_local(ws, chat);
            crate::chats::delete_chat(&mut s.chats, ws, chat);
        }
        changed = true;
    }
    changed
}

/// The reference UI's 400ms debounced `saveUiState`: write only when the layout payload
/// actually changed.
fn poll_ui_state(s: &mut ShellState, clock: f64) -> bool {
    if !s.chats.booted || !s.chats.loaded || s.chats.ui_saving {
        return false;
    }
    let payload = s.canvas.ui_state_json();
    let text = payload.to_string();
    if text == s.chats.ui_saved {
        s.chats.ui_dirty_at = None;
        return false;
    }
    match s.chats.ui_dirty_at {
        None => {
            s.chats.ui_dirty_at = Some(clock);
            false
        }
        Some(at) if clock - at >= 0.4 => {
            s.chats.ui_dirty_at = None;
            crate::chats::save_ui_state(&mut s.chats, payload);
            true
        }
        Some(_) => false,
    }
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
    let mut changed = poll_projects_state(&mut s.canvas.projects, clock);
    changed |= apply_created_project(s);
    changed |= resolve_pending_group(s);
    changed |= s.canvas.projects.group.poll(clock);
    changed |= crate::notes::poll_notes_state(&mut s.canvas.projects.group.notes);
    changed
}

/// Apply the full `NewProjectDialog` result once: open the created chat (and
/// seed its dock in solo mode), refresh the session list, then open the team
/// board in group mode. Mirrors `handleCreateProject` +
/// `handleCreateGroupFromDialog`.
fn apply_created_project(s: &mut ShellState) -> bool {
    let Some(created) = crate::projects::take_new_project(&mut s.canvas.projects) else {
        return false;
    };
    s.canvas.open_created_chat(&created);
    if !s
        .chats
        .snapshot
        .workspaces
        .iter()
        .any(|w| w.id == created.workspace.id)
    {
        s.chats.snapshot.workspaces.push(created.workspace.clone());
    }
    let chat_key = format!("{}:{}", created.workspace.id, created.chat_id);
    if !s.chats.snapshot.chats.iter().any(|c| c.key() == chat_key) {
        let at = crate::rune_transcript::now_iso();
        s.chats.snapshot.chats.push(crate::chats::ChatSummary {
            id: created.chat_id.clone(),
            workspace_id: created.workspace.id.clone(),
            workspace_name: created.workspace.name.clone(),
            workspace_kind: created.workspace.kind.clone(),
            title: created.chat_title.clone(),
            updated_at: at.clone(),
            created_at: at,
            ..crate::chats::ChatSummary::default()
        });
    }
    crate::chats::refresh(&mut s.chats);
    if let Some(project_id) = created.project_id {
        crate::projects::refresh_projects(&mut s.canvas.projects);
        let (host, port) = (s.canvas.projects.host.clone(), s.canvas.projects.port);
        s.canvas.projects.group.open(host, port, project_id);
    }
    true
}

/// Open the group route's project only after the live list contains it. A
/// failed or completed fetch without the id clears the request, so the route
/// can never hang with an unmounted modal.
fn resolve_pending_group(s: &mut ShellState) -> bool {
    let Some(id) = s.canvas.projects.group.pending_open.clone() else {
        return false;
    };
    if s.canvas.projects.projects.iter().any(|p| p.id == id) {
        let group = &mut s.canvas.projects.group;
        group.pending_open = None;
        let (host, port) = (group.host.clone(), group.port);
        group.open(host, port, id);
        return true;
    }
    if !s.canvas.projects.loading
        && (s.canvas.projects.error.is_some() || s.canvas.projects.last_ok > 0.0)
    {
        s.canvas.projects.group.pending_open = None;
        return true;
    }
    false
}

fn init_shell() -> ShellState {
    // The real app entry (native window and browser) loads live chats; unit
    // tests construct `ShellState::default()` and stay offline.
    let mut shell = ShellState::default();
    shell.canvas.projects.host = crate::rune_transcript::DEFAULT_API_HOST.to_string();
    shell.canvas.projects.port = crate::rune_transcript::api_port();
    shell.canvas.projects.group.host = crate::rune_transcript::DEFAULT_API_HOST.to_string();
    shell.canvas.projects.group.port = crate::rune_transcript::api_port();
    shell.chats.auto = true;
    // The dock's provider/model pills load on mount (the reference UI's PromptDock
    // effect), then refresh on their intervals.
    shell.transcript.services_auto = true;
    shell.personal.services_auto = true;
    shell
}

fn apply_home(s: &mut ShellState) {
    // One page: every scene lands on the map. The rail picks the panel.
    s.canvas.rail = RailTab::Chats;
}

fn apply_panel_cloud(s: &mut ShellState) {
    s.canvas.rail = RailTab::Cloud;
}

fn open_demo_sheet(s: &mut ShellState, stream: bool) {
    s.canvas.rail = RailTab::Chats;
    let id = s
        .canvas
        .cards
        .iter()
        .find(|card| card.id.starts_with("chat:"))
        .map(|card| card.id.clone());
    let Some(id) = id else { return };
    let thread = s.canvas.thread_mut(&id);
    let demo = if stream {
        RuneTranscriptState::stream_demo()
    } else {
        RuneTranscriptState::trace_demo()
    };
    *thread = demo;
    s.canvas.sessions.sheet = Some(id);
}

fn apply_stream(s: &mut ShellState) {
    open_demo_sheet(s, true);
}

fn apply_trace(s: &mut ShellState) {
    open_demo_sheet(s, false);
}

fn apply_shell_attachments(s: &mut ShellState) {
    s.canvas.rail = RailTab::Chats;
    let id = s
        .canvas
        .cards
        .iter()
        .find(|card| card.id.starts_with("chat:"))
        .map(|card| card.id.clone());
    let Some(id) = id else { return };
    *s.canvas.thread_mut(&id) = crate::rune_transcript::attachments_demo();
    s.canvas.sessions.sheet = Some(id);
}

fn apply_shell_attachments_lightbox(s: &mut ShellState) {
    apply_shell_attachments(s);
    let Some(id) = s.canvas.sessions.sheet.clone() else {
        return;
    };
    let file = s
        .canvas
        .thread_mut(&id)
        .rows
        .last()
        .and_then(|row| row.attachments.first())
        .cloned();
    if let Some(file) = file {
        s.canvas.thread_mut(&id).attachment_preview = Some(file);
    }
}

fn apply_settings(s: &mut ShellState) {
    s.settings.open(
        crate::rune_transcript::DEFAULT_API_HOST.to_string(),
        crate::rune_transcript::api_port(),
    );
}

fn apply_group(s: &mut ShellState) {
    // A real navigation: the Group rail loads its list, and the detail modal
    // opens from the card once that list contains the project. This keeps the
    // shell behind the backdrop identical to a user click.
    s.canvas.rail = RailTab::Group;
    s.canvas.projects.group.pending_open = Some("p1".to_string());
    crate::projects::refresh_projects(&mut s.canvas.projects);
}

fn apply_group_loading(s: &mut ShellState) {
    s.canvas.rail = RailTab::Group;
    let g = &mut s.canvas.projects.group;
    g.open_id = Some("p1".to_string());
    g.loading = true;
}

fn apply_ask(s: &mut ShellState) {
    // Same sheet wiring as the other demo routes; the thread owns a pending
    // batch so the dock renders the ask panel instead of the composer.
    s.canvas.rail = RailTab::Chats;
    let id = s
        .canvas
        .cards
        .iter()
        .find(|card| card.id.starts_with("chat:"))
        .map(|card| card.id.clone());
    let Some(id) = id else { return };
    *s.canvas.thread_mut(&id) = crate::rune_transcript::ask_demo();
    s.canvas.sessions.sheet = Some(id);
}

fn apply_panel_memory(s: &mut ShellState) {
    s.canvas.rail = RailTab::Memory;
}

/// Parity pose for the composer's live voice dot: the mic is reported live
/// and the breathing tracks are seeded at the keyframe's peak.
fn apply_voice(s: &mut ShellState) {
    let id = s.canvas.sessions.sheet.clone().or_else(|| {
        s.canvas
            .cards
            .iter()
            .find(|c| c.id.starts_with("chat:"))
            .map(|c| c.id.clone())
    });
    let Some(id) = id else { return };
    let thread = s.canvas.thread_mut(&id);
    thread.voice.live = true;
    thread.voice.level = 0.6;
    thread.voice.breathe_start_ms = crate::clock::now_millis().saturating_sub(600);
    let _ = thread.voice.tick_breathe(crate::clock::now_millis());
}

/// The exact Inter variable face the browser loads from Google Fonts.
/// The built-in static Inter is close but not the same instance.
const SHELL_FONTS: &[&[u8]] = &[include_bytes!("../assets/InterVariable.ttf")];

pub fn shell_spec() -> AppSpec<ShellState> {
    #[cfg(target_arch = "wasm32")]
    crate::transport::init_web_auth();
    let mut spec = AppSpec::new("Stark", (1280.0, 900.0), init_shell, shell_view);
    spec.scenes = &[
        ("home", apply_home),
        ("stream", apply_stream),
        ("trace", apply_trace),
        ("attachments", apply_shell_attachments),
        ("attachments-lightbox", apply_shell_attachments_lightbox),
        ("ask", apply_ask),
        ("voice", apply_voice),
        ("group", apply_group),
        ("group-loading", apply_group_loading),
        ("settings", apply_settings),
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
    spec.setup = Some(crate::image_cache::setup::<ShellState>);
    spec.fonts = SHELL_FONTS;
    // The browser's reference mono stack resolves `ui-monospace` to Consolas on
    // Windows; Rune's system-mono list prefers Cascadia Mono, so name the
    // browser's face outright for byte parity. Web keeps the bundled Plex.
    #[cfg(target_arch = "wasm32")]
    {
        spec.families = ("Inter", "IBM Plex Mono");
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        spec.families = ("Inter", "Consolas");
    }
    spec
}

pub fn run_shell_headless(extra_args: Vec<String>) {
    // Rune sizes the window/taskbar/Alt-Tab icons from this PNG. The executable
    // resource covers Explorer and Task Manager; this covers the live window.
    #[cfg(target_os = "windows")]
    let _ = rune::set_app_icon(include_bytes!("../../../assets/stark-icon.png").to_vec());
    // `STARK_FONT_SMOOTHING` is a parity-rig knob: DirectWrite is the
    // Windows default (the browser's path), `off`/`on` are the portable
    // pipelines. It never changes the shipped default.
    let mut app = rune::App::new(shell_spec());
    // One app-drawn titlebar replaces Windows' native caption. Rune keeps the
    // resize edges, Aero Snap, snap layouts, the window menu and animations,
    // and draws its caption buttons over the trailing edge of our band.
    #[cfg(not(target_arch = "wasm32"))]
    {
        app = app.titlebar(rune::Titlebar::new(TOPBAR_H));
    }
    if let Ok(mode) = std::env::var("STARK_FONT_SMOOTHING") {
        app = app.font_smoothing(match mode.as_str() {
            "off" => rune::FontSmoothing::Off,
            "on" => rune::FontSmoothing::On,
            _ => rune::FontSmoothing::DirectWrite,
        });
    }
    app.args(extra_args).run();
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn provider_pill_opens_without_an_open_chat() {
        let mut state = ShellState::default();
        state.chats.auto = false;
        state.canvas.cards.clear();
        state.canvas.sessions.order.clear();
        state.canvas.sessions.sheet = None;
        state.transcript.live = false;
        let mut rt = Runtime::new(state, shell_view, (1280.0, 900.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let (x, y) = rt.find("composer-provider").expect("home provider pill");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..6 {
            rt.frame(1.0 / 60.0);
        }
        assert!(
            rt.state.transcript.provider_keys.open,
            "the home dock provider pill opens the panel"
        );
        assert!(
            rt.find("providerkeys-overlay").is_some(),
            "the home card mounts the provider sheet at the shell root"
        );
    }

    #[test]
    fn update_toast_mounts_ready_and_downloading_states() {
        let env = Env::new(true, 1.0);
        let mut state = ShellState::default();
        state.settings.update.phase = UpdatePhase::Ready;
        state.settings.update.restart_required = true;
        state.settings.update.message = "Stark 2.7.0 is ready.".to_string();
        let mut rt = Runtime::new(state, shell_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        assert!(rt.find("update-toast").is_some());
        assert!(rt.find("update-toast-restart").is_some());

        let env = Env::new(true, 1.0);
        let mut state = ShellState::default();
        state.settings.update.phase = UpdatePhase::Downloading;
        state.settings.update.message = "Downloading Stark 2.7.0… 40%".to_string();
        let mut rt = Runtime::new(state, shell_view, (390.0, 844.0), env);
        rt.frame(0.0);
        assert!(rt.find("update-toast").is_some());
        assert!(rt.find("update-toast-restart").is_none());
    }

    #[test]
    fn update_restart_waits_for_runs_and_drafts() {
        let mut s = ShellState::default();
        s.transcript.live = false;
        s.personal.live = false;
        for thread in s.canvas.sessions.threads.values_mut() {
            thread.live = false;
        }
        assert!(update_restart_safe(&s));
        s.transcript.edit.text = "half a message".to_string();
        assert!(!update_restart_safe(&s));
        s.transcript.edit.text.clear();
        s.transcript.live = true;
        assert!(!update_restart_safe(&s));
        s.transcript.live = false;
        let id = s.canvas.cards[0].id.clone();
        s.canvas.thread_mut(&id).live = true;
        assert!(!update_restart_safe(&s));
    }

    #[test]
    fn home_card_width_matches_reference_wrapper_gutters() {
        for (w, h, expected) in [(1280.0, 900.0, 724.0), (800.0, 573.0, 487.0)] {
            let env = Env::new(true, 1.0);
            let mut rt = Runtime::new(ShellState::default(), shell_view, (w, h), env);
            rt.frame(0.0);
            let (rect, _) = rt.presented("home-chat-card").expect("card mounted");
            assert!(
                (rect[2] - expected).abs() < 0.51,
                "{w}x{h}: card width {} != {expected}",
                rect[2]
            );
        }
    }

    #[test]
    fn home_chat_collapse_folds_messages_but_keeps_the_dock() {
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(ShellState::default(), shell_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        let open = rt.presented("home-chat-card").expect("home card mounted").0[3];
        assert!(
            rt.find("home-chat-collapse").is_some(),
            "collapse control mounts"
        );
        assert!(rt.find("composer-dock").is_some(), "dock starts mounted");

        let (x, y) = rt.find("home-chat-collapse").expect("collapse control");
        // The control rests at 0 opacity on desktop, like the reference UI's hover-only
        // chevron; hover the card before pressing it.
        rt.pointer_move(x, y);
        for _ in 0..8 {
            rt.frame(1.0 / 60.0);
        }
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..60 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.state.home_minimized, "control folds the card");
        let folded = rt.presented("home-chat-card").expect("home card mounted").0[3];
        assert!(
            folded < open - 40.0,
            "folded card hugs the dock: {folded} !< {open}"
        );
        assert!(
            rt.find("composer-dock").is_some(),
            "prompt dock stays mounted"
        );
        assert!(
            rt.find("transcript-body").is_none(),
            "message area folds away"
        );

        let (x, y) = rt.find("home-chat-collapse").expect("collapse control");
        rt.pointer_move(x, y);
        for _ in 0..8 {
            rt.frame(1.0 / 60.0);
        }
        let (x, y) = rt.find("home-chat-collapse").expect("collapse control");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..60 {
            rt.frame(1.0 / 60.0);
        }
        assert!(!rt.state.home_minimized, "control expands the card");
        assert!(rt.find("transcript-body").is_some(), "message area returns");
    }

    #[test]
    fn phone_layout_hides_the_rail_and_opens_the_drawer_from_bottom_nav() {
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(ShellState::default(), shell_view, (390.0, 844.0), env);
        rt.frame(0.0);
        assert!(rt.find("icon-rail").is_none(), "rail hidden below md");
        assert!(rt.find("map-sidebar").is_none(), "panel starts closed");
        assert!(rt.find("house-mobile-nav").is_some(), "bottom nav mounted");
        let (x, y) = rt.find("mobile-chats").expect("chats button");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert!(rt.state.mobile_panel_open, "nav opens the drawer");
        assert!(rt.find("side-drawer").is_some(), "drawer mounted");
        assert!(rt.find("side-backdrop").is_some(), "backdrop mounted");
        assert!(rt.find("side-panel").is_some(), "chats panel in drawer");
        let (drawer, _) = rt.presented("side-drawer").expect("drawer rect");
        assert!(drawer[1] >= TOPBAR_H, "drawer starts below the titlebar");
        // The titlebar is chat-only now; the backdrop closes the drawer.
        assert!(rt.find("topbar-menu").is_none(), "no titlebar navigation");
        let (backdrop, _) = rt.presented("side-backdrop").expect("backdrop");
        let bx = backdrop[0] + backdrop[2] - 8.0;
        let by = backdrop[1] + backdrop[3] * 0.5;
        rt.pointer_down(bx, by);
        rt.pointer_up(bx, by);
        rt.frame(1.0 / 60.0);
        assert!(!rt.state.mobile_panel_open, "backdrop closes drawer");
        let (x, y) = rt.find("mobile-chats").expect("chats button");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert!(rt.state.mobile_panel_open);
        let (backdrop, _) = rt.presented("side-backdrop").expect("backdrop");
        let bx = backdrop[0] + backdrop[2] - 8.0;
        let by = backdrop[1] + backdrop[3] * 0.5;
        rt.pointer_down(bx, by);
        rt.pointer_up(bx, by);
        rt.frame(1.0 / 60.0);
        assert!(!rt.state.mobile_panel_open, "backdrop dismisses");
        assert!(rt.find("side-drawer").is_none());
    }

    #[test]
    fn single_page_mounts_tabs_rail_panel_and_canvas() {
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(ShellState::default(), shell_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        // Session tabs seeded from the scene cards, rail + chats panel up,
        // canvas always mounted — one page, no routes.
        assert!(rt.find("session-chat:ws-general:chat-1").is_some());
        // The titlebar is chat-only; new chats come from the panel header.
        assert!(rt.find("session-new").is_none());
        assert!(rt.find("map-new").is_some());
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
    fn rail_gear_opens_the_global_settings_modal() {
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(ShellState::default(), shell_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        let (x, y) = rt.find("rail-settings").expect("settings gear");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.state.settings.open);
        for key in ["settings-card", "settings-backdrop", "settings-title"] {
            assert!(rt.find(key).is_some(), "{key} mounted");
        }
        // Backdrop press dismisses the global modal, not a chat thread's.
        rt.pointer_down(100.0, 100.0);
        rt.pointer_up(100.0, 100.0);
        rt.frame(1.0 / 60.0);
        assert!(!rt.state.settings.open);
    }

    #[test]
    fn session_tabs_select_new_and_close() {
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(ShellState::default(), shell_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        let n = rt.state.canvas.sessions.order.len();
        // New session (panel header `+`) adds a card + tab and selects it.
        let (x, y) = rt.find("map-new").unwrap();
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
        // Thread survives the close (reopen restores history); the closed
        // id is listed for the Add menu.
        assert!(rt.state.canvas.sessions.threads.contains_key(&id));
        assert!(rt.state.canvas.closed.iter().any(|(cid, _)| cid == &id));
    }

    #[test]
    fn titlebar_merges_tabs_and_keeps_new_session_compact() {
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(ShellState::default(), shell_view, (1280.0, 900.0), env);
        rt.frame(0.0);

        // The whole shell chrome is one band at the top of the window.
        let (rect, _) = rt.presented("shell-titlebar").expect("titlebar");
        assert_eq!(rect[1], 0.0, "titlebar starts at the window top");
        assert!((rect[3] - TOPBAR_H).abs() < 0.5, "one 44pt row");
        assert!(rt.presented("shell-tabs").is_some());

        // The titlebar carries tabs only: no add/search controls, so the
        // strip always starts at the leading gutter.
        assert!(rt.find("session-new").is_none(), "no titlebar add button");
        assert!(rt.find("topbar-menu").is_none(), "no titlebar menu");
        let last = rt
            .state
            .canvas
            .sessions
            .order
            .last()
            .expect("a tab")
            .clone();
        let (tab, _) = rt.presented(&format!("session-{last}")).expect("last tab");
        assert!(tab[0] >= 0.0, "tab inside the strip");

        // Quiet tabs are separated by a hairline; the active pill is not.
        let ids = rt.state.canvas.sessions.order.clone();
        let active = rt.state.canvas.active_id.clone();
        let mut saw_separator = false;
        for index in 1..ids.len() {
            let key = format!("tab-sep-{index}");
            let adjacent_active = ids[index] == active || ids[index - 1] == active;
            if rt.find(&key).is_some() {
                saw_separator = true;
                assert!(!adjacent_active, "active tab keeps no divider");
            }
        }
        assert!(saw_separator, "open tabs show dividers");
    }

    #[test]
    fn subagent_drill_docks_right_of_the_chat() {
        let env = Env::new(true, 1.0);
        let mut state = ShellState::default();
        // Open the drill on the visible card thread (the sheet), which is
        // the same thread the home card renders.
        let active = state.canvas.active_id.clone();
        let thread = state.canvas.thread_mut(&active);
        // Use a real spawned card so the dock binds its child thread/composer.
        crate::rune_transcript::open_subagent(thread, "chat-2", "Dev", Some("ws-general"));
        thread.drill_inbox = None;
        let drill = thread.active_subagent.as_mut().expect("drill");
        drill.loading = false;
        drill.workspace_id = "ws-general".to_string();
        let mut rt = Runtime::new(state, shell_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        // The conversation stays mounted; the child session docks to its right.
        assert!(rt.find("home-chat-card").is_some(), "chat stays mounted");
        assert!(rt.find("subagent-panel").is_some(), "panel mounted");
        assert!(rt.find("subagent-panel-surface").is_some(), "panel surface");
        assert!(
            rt.find("subagent-panel-close").is_some(),
            "panel close button"
        );
        assert!(
            rt.find("subagent-composer-input").is_some(),
            "child prompt dock mounted"
        );
        let (card, _) = rt.presented("home-chat-card").expect("card rect");
        let (panel, _) = rt.presented("subagent-panel").expect("panel rect");
        assert!(
            panel[0] >= card[0] + card[2] - 1.0,
            "panel sits right of the card: panel {} card {}",
            panel[0],
            card[0] + card[2]
        );
        // Close returns the width to the chat.
        let (x, y) = rt.find("subagent-panel-close").expect("close placed");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..8 {
            rt.frame(1.0 / 60.0);
        }
        assert!(
            rt.state
                .canvas
                .sheet_thread()
                .is_none_or(|t| t.active_subagent.is_none()),
            "drill closed"
        );
        assert!(rt.find("subagent-panel").is_none(), "panel closed");
        assert!(rt.find("home-chat-card").is_some(), "chat still mounted");
    }

    #[test]
    fn rail_menu_folds_only_the_panel_and_rail_stays() {
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(ShellState::default(), shell_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        assert!(rt.find("icon-rail").is_some());
        assert!(rt.find("map-sidebar").is_some());
        assert!(rt.find("topbar-menu").is_none(), "topbar has no navigation");

        // The rail hamburger folds the chat panel only. Settings, chats,
        // group, and cloud stay reachable because the rail never hides.
        let (x, y) = rt.find("rail-menu").expect("rail menu");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..8 {
            rt.frame(1.0 / 60.0);
        }
        assert!(!rt.state.panel_open);
        assert!(rt.find("icon-rail").is_some(), "rail stays visible");
        assert!(rt.find("map-sidebar").is_none(), "panel folded");
        assert!(
            rt.find("rail-settings").is_some(),
            "settings stays reachable"
        );
        assert!(rt.find("rail-Chats").is_some(), "chats stays reachable");
        assert!(rt.find("shell-titlebar").is_some(), "titlebar stays");

        // Pressing the hamburger again restores the same panel.
        let (x, y) = rt.find("rail-menu").expect("rail menu restores");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.state.panel_open, "panel restored");
        assert!(rt.find("map-search").is_some());

        // Choosing a rail destination also brings the panel back.
        let (x, y) = rt.find("rail-Cloud").expect("cloud");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.state.panel_open, "destination opens its panel");
        assert!(rt.find("cloud-board").is_some());
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
        assert_eq!(state.personal.rows.len(), 2);
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
        // The same way in tests and in the window.
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
        // Memory panel loads its board the same way (the rail mirrors the reference UI:
        // chats/projects/cloud only, so switch the panel directly).
        rt.update(|s, _| {
            s.canvas.rail = RailTab::Memory;
        });
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
    fn a_project_card_opens_the_group_modal() {
        let env = Env::new(true, 1.0);
        let mut state = ShellState::default();
        state.canvas.rail = RailTab::Group;
        state.canvas.projects.auto = false;
        state.canvas.projects.projects = vec![crate::projects::GroupProject {
            id: "p1".to_string(),
            title: "Analytics".to_string(),
            status: "running".to_string(),
            done: 1,
            total: 4,
            waiting: 1,
        }];
        let mut rt = Runtime::new(state, shell_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        let (x, y) = rt.find("side-projects-p1").expect("project card");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert_eq!(
            rt.state.canvas.projects.group.open_id.as_deref(),
            Some("p1")
        );
        assert!(rt.find("gp-modal").is_some(), "modal mounts immediately");
        assert!(
            rt.find("gp-skeleton").is_some(),
            "detail starts as skeleton"
        );
        // The card holds the modal root, so a backdrop press closes it.
        rt.pointer_down(10.0, 10.0);
        rt.pointer_up(10.0, 10.0);
        rt.frame(0.0);
        assert!(
            rt.state.canvas.projects.group.open_id.is_none(),
            "backdrop dismisses the group modal"
        );
        assert!(rt.find("gp-modal").is_none());
    }

    #[test]
    fn new_project_dialog_opens_and_requires_a_folder() {
        use rune::{Key, KeyEvent, Mods};
        let env = Env::new(true, 1.0);
        let mut state = ShellState::default();
        state.canvas.rail = RailTab::Group;
        state.canvas.projects.auto = false;
        let mut rt = Runtime::new(state, shell_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        let (x, y) = rt.find("side-projects-new").expect("new project entry");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..20 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.state.canvas.projects.dialog.open);
        assert!(rt.find("new-project-card").is_some());
        assert!(rt.find("new-project-name").is_some());

        // Return in the focused name input keeps step 1 and reports the reference UI's
        // copy; the disabled Next itself takes no press.
        rt.key(KeyEvent {
            key: Key::Enter,
            mods: Mods::default(),
        });
        rt.frame(0.0);
        assert_eq!(rt.state.canvas.projects.dialog.step, 1);
        assert_eq!(
            rt.state.canvas.projects.dialog.error.as_deref(),
            Some("Choose a folder first.")
        );
        assert!(rt.find("new-project-error").is_some());
    }

    #[test]
    fn new_project_dialog_step_two_switches_mode_and_returns() {
        use rune::{Key, KeyEvent, Mods};
        let env = Env::new(true, 1.0);
        let mut state = ShellState::default();
        state.canvas.rail = RailTab::Group;
        state.canvas.projects.auto = false;
        state.canvas.projects.dialog.open = true;
        state.canvas.projects.dialog.step = 1;
        state.canvas.projects.dialog.path.text = "D:/work/project".to_string();
        let mut rt = Runtime::new(state, shell_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        for _ in 0..20 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.find("new-project-card").is_some());

        let (x, y) = rt.find("new-project-next").expect("next button");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..20 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(rt.state.canvas.projects.dialog.step, 2);
        assert!(rt.find("new-project-mode-group").is_some());
        assert!(rt.find("new-project-mode-solo").is_some());
        assert!(rt.find("new-project-task").is_some());
        assert!(rt.find("new-project-summary-path").is_some());
        // Step 2 focuses the task, like the reference UI's `taskRef` effect.
        rt.key(KeyEvent {
            key: Key::Text("x".to_string()),
            mods: Mods::default(),
        });
        rt.frame(0.0);
        assert_eq!(rt.state.canvas.projects.dialog.task.text, "x");

        let (x, y) = rt.find("new-project-mode-solo").expect("solo radio");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert_eq!(
            rt.state.canvas.projects.dialog.mode,
            crate::projects::NewProjectMode::Solo
        );

        let (x, y) = rt.find("new-project-back").expect("back button");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert_eq!(rt.state.canvas.projects.dialog.step, 1);
        assert!(rt.find("new-project-name").is_some());
    }

    #[test]
    fn new_project_dialog_backdrop_and_escape_dismiss() {
        use rune::{Key, KeyEvent, Mods};
        let env = Env::new(true, 1.0);
        let mut state = ShellState::default();
        state.canvas.rail = RailTab::Group;
        state.canvas.projects.auto = false;
        state.canvas.projects.dialog.open = true;
        let mut rt = Runtime::new(state, shell_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        for _ in 0..20 {
            rt.frame(1.0 / 60.0);
        }
        // A press on the backdrop is outside the card and dismisses.
        rt.pointer_down(10.0, 10.0);
        rt.pointer_up(10.0, 10.0);
        rt.frame(0.0);
        assert!(!rt.state.canvas.projects.dialog.open);
        assert!(rt.find("new-project-card").is_none());

        // Escape is the same dismiss, while the card keeps the modal root.
        let (x, y) = rt.find("side-projects-new").expect("new project entry");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..20 {
            rt.frame(1.0 / 60.0);
        }
        rt.key(KeyEvent {
            key: Key::Escape,
            mods: Mods::default(),
        });
        rt.frame(0.0);
        assert!(!rt.state.canvas.projects.dialog.open);
        assert!(rt.find("new-project-card").is_none());
    }

    #[test]
    fn new_project_dialog_close_resets_a_fresh_mount() {
        let env = Env::new(true, 1.0);
        let mut state = ShellState::default();
        state.canvas.rail = RailTab::Group;
        state.canvas.projects.auto = false;
        state.canvas.projects.dialog.open = true;
        state.canvas.projects.dialog.name.text = "Old name".to_string();
        state.canvas.projects.dialog.path.text = "D:/old".to_string();
        let mut rt = Runtime::new(state, shell_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        let (x, y) = rt.find("new-project-close").expect("close button");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert!(!rt.state.canvas.projects.dialog.open);
        assert!(rt.find("new-project-card").is_none());

        let (x, y) = rt.find("side-projects-new").expect("new project entry");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert_eq!(rt.state.canvas.projects.dialog.step, 1);
        assert!(rt.state.canvas.projects.dialog.name.text.is_empty());
        assert!(rt.state.canvas.projects.dialog.path.text.is_empty());
    }

    #[test]
    fn new_project_dialog_creates_workspace_chat_and_group_board() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::time::{Duration, Instant};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for _ in 0..8 {
                let Ok((mut sock, _)) = listener.accept() else {
                    break;
                };
                // Read the whole request: a response written while the body
                // is still unread resets the connection on Windows.
                let mut raw = Vec::new();
                let mut buf = [0u8; 8192];
                loop {
                    let n = sock.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&raw);
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len = text[..end]
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("Content-Length: ")
                                    .and_then(|value| value.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if raw.len() >= end + 4 + len {
                            break;
                        }
                    }
                }
                let req = String::from_utf8_lossy(&raw);
                let (status, body) = if req.starts_with("POST /api/workspaces ") {
                    (
                        "200 OK",
                        serde_json::json!({
                            "id": "ws-new",
                            "name": "New Project",
                            "path": "C:/work/new",
                            "exists": true,
                            "chatCount": 0,
                            "workspaceKind": "project"
                        })
                        .to_string(),
                    )
                } else if req.starts_with("POST /api/workspaces/ws-new/chats ") {
                    (
                        "200 OK",
                        serde_json::json!({"id": "chat-new", "title": "New Chat"}).to_string(),
                    )
                } else if req.starts_with("POST /api/groups ") {
                    (
                        "200 OK",
                        serde_json::json!({"id": "gp-new", "title": "Build it"}).to_string(),
                    )
                } else if req.starts_with("GET /api/groups/gp-new") {
                    (
                        "200 OK",
                        serde_json::json!({
                            "project": {"id": "gp-new", "goal": "Build it", "status": "running"},
                            "items": [],
                            "messages": [],
                            "members": []
                        })
                        .to_string(),
                    )
                } else if req.starts_with("GET /api/groups") {
                    (
                        "200 OK",
                        serde_json::json!({
                            "projects": [{
                                "id": "gp-new",
                                "title": "Build it",
                                "status": "running",
                                "items_done": 0,
                                "items_total": 1
                            }],
                            "scoped": false
                        })
                        .to_string(),
                    )
                } else {
                    ("404 Not Found", "{}".to_string())
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(response.as_bytes());
            }
        });

        let mut state = ShellState::default();
        state.canvas.projects.auto = false;
        state.canvas.projects.host = "127.0.0.1".to_string();
        state.canvas.projects.port = port;
        state.canvas.projects.dialog.open = true;
        state.canvas.projects.dialog.step = 2;
        state.canvas.projects.dialog.name.text = "New Project".to_string();
        state.canvas.projects.dialog.path.text = "C:/work/new".to_string();
        state.canvas.projects.dialog.task.text = "Build it".to_string();
        crate::projects::create_new_project(&mut state.canvas.projects);
        let started = Instant::now();
        while state.canvas.projects.dialog.busy && started.elapsed() < Duration::from_secs(5) {
            crate::projects::poll_projects_state(&mut state.canvas.projects, 1.0);
            if state.canvas.projects.dialog.busy {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        assert!(!state.canvas.projects.dialog.busy, "worker finished");
        assert!(
            !state.canvas.projects.dialog.open,
            "dialog closes on success: {:?}",
            state.canvas.projects.dialog.error
        );
        assert!(poll_projects(&mut state, 1.0), "result applied");
        assert_eq!(
            state.canvas.active_id, "chat:ws-new:chat-new",
            "created chat is active"
        );
        let thread = state.canvas.thread_mut("chat:ws-new:chat-new").clone();
        assert_eq!(thread.workspace_id, "ws-new");
        assert_eq!(thread.chat_id, "chat-new");
        assert!(thread.chat_ready);
        assert!(thread.edit.text.is_empty(), "group mode seeds no prompt");
        assert!(
            state
                .canvas
                .chat_rows
                .iter()
                .any(|chat| chat.key() == "ws-new:chat-new"),
            "new chat registered on the map"
        );
        assert_eq!(
            state.canvas.projects.group.open_id.as_deref(),
            Some("gp-new"),
            "team board opens"
        );
    }

    #[test]
    fn solo_project_seeds_the_new_chat_prompt() {
        let mut state = ShellState::default();
        let created = crate::projects::NewProjectCreated {
            workspace: crate::chats::ChatWorkspace {
                id: "ws-solo".to_string(),
                name: "Solo".to_string(),
                kind: "project".to_string(),
                path: "C:/solo".to_string(),
                chat_count: 0,
                exists: true,
            },
            chat_id: "chat-solo".to_string(),
            chat_title: "New Chat".to_string(),
            project_id: None,
            prompt: "Research the docs".to_string(),
        };
        state.canvas.open_created_chat(&created);
        assert_eq!(state.canvas.active_id, "chat:ws-solo:chat-solo");
        let thread = state.canvas.thread_mut("chat:ws-solo:chat-solo").clone();
        assert_eq!(thread.edit.text, "Research the docs");
        assert_eq!(thread.edit.focus, "Research the docs".len());
        assert_eq!(thread.workspace_id, "ws-solo");
        assert_eq!(thread.chat_id, "chat-solo");
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
        f_tx.send(SseFrame::new(
            "delta",
            serde_json::json!({"kind":"text","text":"42"}),
        ))
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
            .any(|row| row.body.contains("42")));
    }

    #[test]
    fn dock_send_runs_active_session_and_opens_sheet() {
        // Home chat card send lands in the ACTIVE card thread and the card
        // stays open over the map.
        let env = Env::new(true, 1.0);
        let mut state = ShellState::default();
        let active = state.canvas.active_id.clone();
        // Point the active thread at the dead port (deterministic: the run
        // thread fails fast into an error frame).
        {
            let thread = state.canvas.thread_mut(&active);
            thread.edit.text = "hello map".to_string();
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
        assert!(rt.find("home-chat-card").is_some(), "home chat card mounts");
    }
    #[test]
    fn closing_a_trace_fades_the_body_while_the_row_folds() {
        let env = Env::new(true, 1.0);
        let mut state = ShellState::default();
        apply_trace(&mut state);
        let mut rt = Runtime::new(state, shell_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        let open_h = rt
            .presented("trace-agent-trace-body")
            .expect("open trace body")
            .0[3];
        let (x, y) = rt.find("trace-agent-trace-head").expect("trace head");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        rt.frame(1.0 / 60.0);
        {
            let thread = rt.state.canvas.sheet_thread().expect("trace sheet");
            let row = thread
                .rows
                .iter()
                .find(|row| row.key == "trace-agent")
                .expect("trace row");
            assert!(!row.trace_open, "the header closes the accordion");
            assert!(row.trace_anim_at.is_some(), "the close starts the clock");
        }
        // Pin the pose mid-fade: a real frame after the press is only a few
        // milliseconds in, so the eased opacity is still near 1.
        rt.update(|s, _| {
            crate::rune_transcript::set_frame_millis(1_000);
            if let Some(thread) = s.canvas.sheet_thread_mut() {
                if let Some(row) = thread.rows.iter_mut().find(|row| row.key == "trace-agent") {
                    row.trace_anim_at = Some(910);
                }
            }
        });
        for _ in 0..4 {
            rt.frame(1.0 / 60.0);
        }
        let (rect, opacity) = rt
            .presented("trace-agent-trace-body")
            .expect("body stays mounted through the fade");
        assert!(
            opacity > 0.1 && opacity < 0.9,
            "body is mid-fade: {opacity} at {rect:?}"
        );
        assert!(
            rect[3] > 1.0 && rect[3] < open_h - 2.0,
            "the row folds its content: {rect:?} (open {open_h})"
        );
        assert!(
            rect[1] < 900.0 && rect[1] > 0.0,
            "body rides the header margin: {rect:?}"
        );
        // After the 180 ms window the normal child unmounts. It never becomes
        // a presence layer, so there is no ghost below the folded row.
        rt.update(|s, _| {
            crate::rune_transcript::set_frame_millis(2_000);
            if let Some(thread) = s.canvas.sheet_thread_mut() {
                if let Some(row) = thread.rows.iter_mut().find(|row| row.key == "trace-agent") {
                    row.trace_anim_at = Some(1_780);
                }
            }
        });
        rt.frame(1.0 / 60.0);
        assert!(
            rt.find("trace-agent-trace-body").is_none(),
            "the body unmounts when the fade ends"
        );
    }
    #[test]
    fn group_route_opens_only_after_the_list_contains_the_project() {
        let mut state = ShellState::default();
        state.canvas.projects.auto = false;
        state.canvas.projects.port = 1;
        state.canvas.projects.group.port = 1;
        apply_group(&mut state);
        assert_eq!(
            state.canvas.projects.group.pending_open.as_deref(),
            Some("p1")
        );
        assert!(
            state.canvas.projects.loading,
            "the route loads the live list"
        );
        assert!(
            state.canvas.projects.group.open_id.is_none(),
            "the modal is not mounted before the list arrives"
        );

        // Still loading: the request stays pending and no modal mounts.
        state.canvas.projects.loading = true;
        assert!(!resolve_pending_group(&mut state));
        assert!(state.canvas.projects.group.open_id.is_none());

        // The fresh list contains the project: open it and consume the request.
        state.canvas.projects.loading = false;
        state.canvas.projects.last_ok = 1.0;
        state.canvas.projects.projects = vec![crate::projects::GroupProject {
            id: "p1".to_string(),
            title: "Analytics".to_string(),
            status: "running".to_string(),
            done: 1,
            total: 4,
            waiting: 1,
        }];
        assert!(resolve_pending_group(&mut state));
        assert_eq!(state.canvas.projects.group.open_id.as_deref(), Some("p1"));
        assert!(state.canvas.projects.group.pending_open.is_none());
    }

    #[test]
    fn group_route_clears_a_missing_project_instead_of_hanging() {
        let mut state = ShellState::default();
        state.canvas.projects.group.pending_open = Some("gone".to_string());
        state.canvas.projects.loading = false;
        state.canvas.projects.last_ok = 1.0;
        state.canvas.projects.projects.clear();
        assert!(resolve_pending_group(&mut state));
        assert!(state.canvas.projects.group.pending_open.is_none());
        assert!(state.canvas.projects.group.open_id.is_none());
    }
}
