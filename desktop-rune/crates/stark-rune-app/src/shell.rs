//! Single-page shell — one window, chat types only, no workspace routes.
//!
//! Layout mirrors the desktop app: session tabs on top (one per open chat +
//! New session), then a row of icon rail + side panel + canvas map. The rail
//! switches the panel (Chats / Agents / Group / Cloud / Memory / House); the
//! canvas always stays mounted, so a running stream keeps draining whatever
//! panel is up. A transcript sheet opens over the map for the active session.

use rune::{col, row, text, AppSpec, Cx, El, LineCap, Path, Shape};

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

/// Live window size (from the root's laid-out rect at the top of every frame).
/// The first frame before layout uses the spec size.
static mut WINDOW_W: f32 = 1280.0;
static mut WINDOW_H: f32 = 900.0;

/// Sidebar/side-panel fold threshold (React `md:` = 768).
const PANEL_FOLD_W: f32 = 768.0;
/// Top session-tab bar (`HouseHeader` md:h-[52px]).
const TAB_H: f32 = 52.0;
/// Side panel width (`HouseSidebar` w-[248px]).
const PANEL_W: f32 = 248.0;

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

/// One stroked icon from a 24×24 path, Lucide geometry scaled to `size`.
fn icon_shape(path: Path, size: f32, color: rune::Color) -> Shape {
    Shape::new()
        .stroke_path(&path, crate::lucide_stroke(size), LineCap::Round, color)
        .view_box(24.0, 24.0)
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

fn icon_folder(size: f32, color: rune::Color) -> Shape {
    // Lucide v1.17 `folder-kanban`: tabbed folder + three kanban ticks.
    let p = Path::new()
        .move_to([4.0, 20.0])
        .line_to([20.0, 20.0])
        .cubic_to([21.1, 20.0], [22.0, 19.1], [22.0, 18.0])
        .line_to([22.0, 8.0])
        .cubic_to([22.0, 6.9], [21.1, 6.0], [20.0, 6.0])
        .line_to([12.07, 6.0])
        .cubic_to([11.4, 6.0], [10.78, 5.66], [10.41, 5.1])
        .line_to([9.59, 3.9])
        .cubic_to([9.22, 3.34], [8.6, 3.0], [7.93, 3.0])
        .line_to([4.0, 3.0])
        .cubic_to([2.9, 3.0], [2.0, 3.9], [2.0, 5.0])
        .line_to([2.0, 18.0])
        .cubic_to([2.0, 19.1], [2.9, 20.0], [4.0, 20.0])
        .close()
        .move_to([8.0, 10.0])
        .line_to([8.0, 14.0])
        .move_to([12.0, 10.0])
        .line_to([12.0, 12.0])
        .move_to([16.0, 10.0])
        .line_to([16.0, 16.0]);
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
        RailTab::Group => icon_folder(size, color),
        RailTab::Cloud => icon_cloud(size, color),
        // Not in React's rail (single-page: chats, projects, cloud only);
        // mapped for completeness if a panel reuses the helper.
        RailTab::Agents | RailTab::Memory | RailTab::House => icon_home(size, color),
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

/// One session tab: card title + close. Press activates the chat (React
/// `SessionTabs`: the active tab drives the home chat card).
fn session_tab(s: &ShellState, id: &str) -> El {
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
    // `↳ ` title prefix (React `SessionTabs`).
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
    // Rune measures text to whole points (`measure` rounds up); React keeps
    // the fractional advance. Trimming the rounding off the right padding
    // keeps the tab's outer width — and the next tab's x — exact.
    let advance = rune::text::advance(&title_text, title_style);
    let right_pad = 11.0 - (advance.ceil() - advance);
    let mut el = row()
        .key(format!("session-{id}"))
        .role("tab")
        .max_w(210.0)
        .h(30.0)
        .items_center()
        .gap(8.0)
        .pl(11.0)
        .pr(right_pad)
        .radius(8.0)
        .border(1.0, ink(if active { "#ffffff24" } else { "#00000000" }))
        .hover_group()
        .on_press(move |s: &mut ShellState| {
            s.canvas.select_card(&select);
        })
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
        .child(
            text(title_text)
                .key(format!("session-{id}-title"))
                .grow(1.0)
                .min_w(0.0)
                .truncate()
                .font(title_style)
                .color(ink(if active { "#ffffff" } else { "#a1a1aa" })),
        );
    if running {
        // `session-run-dot` (6pt, breathing glow) between title and close.
        el = el.child(
            rune::el()
                .key(format!("session-{id}-run"))
                .size(6.0, 6.0)
                .radius(3.0)
                .fill(ink("#fafafa"))
                .shadow(0.0, 8.0, home_glow()),
        );
    }
    el = el.child(
        rune::el()
            .key(format!("session-{id}-close"))
            .role("button")
            .label("Close session")
            .size(18.0, 18.0)
            .center()
            .radius(6.0)
            .group_opacity(if active { 1.0 } else { 0.0 }, 1.0)
            .on_press(move |s: &mut ShellState| {
                s.canvas.close_session(&close);
            })
            .child(rune::el().size(11.0, 11.0).inert().shape({
                let color = ink(if active { "#71717a" } else { "#52525b" });
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
            })),
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
fn rail_button(s: &ShellState, tab: RailTab) -> El {
    let active = s.canvas.rail == tab;
    let color = ink(if active { "#fafafa" } else { "#71717a" });
    let mut el = rune::el()
        .key(format!("rail-{}", tab.label()))
        .role("button")
        .label(format!("{} panel", tab.label()))
        .size(36.0, 36.0)
        .radius(12.0)
        .center()
        .on_press(move |s: &mut ShellState| {
            s.canvas.rail = tab;
        })
        .child(
            rune::el()
                .key(format!("rail-{}-icon", tab.label()))
                .size(17.0, 17.0)
                .inert()
                .shape(rail_icon(tab, 17.0, color)),
        );
    if active {
        el = el.fill(rune::Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.10,
        });
    }
    el
}

/// Side panel content for the active rail tab. Chat-local panels read the
/// canvas; cloud/memory/house read shell state.
fn side_panel(s: &ShellState) -> El {
    // The chats panel owns its own gutters (`HouseSidebar`); the other boards
    // sit inside the shared 12pt shell padding.
    let chats = s.canvas.rail == RailTab::Chats;
    let mut panel = col()
        .key("side-panel")
        .w(PANEL_W)
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
/// Width hugs React's `md:max-w-[740px]`, height caps at 74vh.
fn home_chat_overlay(s: &ShellState, stage_w: f32, stage_h: f32, window_h: f32) -> Option<El> {
    let thread = s.canvas.sheet_thread()?;
    // React caps the wrapper (`md:max-w-[740px]`) and the wrapper's own
    // `px-2` gutters sit inside that cap: the card is at most 724 wide.
    // At 800 the stage is narrower than the cap, so this matches the old
    // `stage_w - 16` exactly.
    let card_w = stage_w.clamp(260.0, 740.0) - 16.0;
    let mut card = col()
        .key("home-chat-card")
        .w(card_w)
        .radius(16.0)
        .border(1.0, ink("#ffffff14"))
        // React is `rgba(13,13,16,.94)` over `backdrop-filter: blur(18px)`.
        // The web renderer's backdrop copy leaves the canvas cards' text
        // legible through 6% bleed; at 99% the composite is the same flat
        // colour React shows (the desktop blur still smooths the remainder).
        .fill(ink("#0d0d10fc"))
        // `.house-chat-card { backdrop-filter: blur(18px) }`: without it the
        // fill leaves sharp canvas ghosts under the conversation.
        .backdrop_blur(18.0)
        .px(8.0)
        .pt(6.0)
        .pb(4.0)
        .clip_rounded(16.0)
        // CSS borders take layout room; matching that keeps the inner
        // transcript/dock exactly React's 469pt (487 - 2 - 16).
        .border_layout();
    // `max-h-[74vh]` on the home variant. Empty and message threads both
    // saturate it at this window size; an explicit height (rather than a
    // `max_h` flex guess) is what keeps the scroll viewport from collapsing
    // inside Rune's auto-height column.
    let h = (window_h * 0.74).clamp(220.0, (stage_h - 8.0).max(220.0));
    card = card.h(h);
    card = card.child(home_transcript_body::<ShellState>(
        thread,
        Thread::Open,
        card_w,
        window_h,
    ));
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
/// the home chat card's clipping never crops them (React portals its dialogs
/// over the whole workspace).
fn active_chat_sheets(s: &ShellState) -> Vec<El> {
    let Some(t) = s.canvas.sheet_thread() else {
        return Vec::new();
    };
    let mut sheets = Vec::new();
    if t.settings_open {
        sheets.push(crate::rune_transcript::settings_sheet::<ShellState>(
            t,
            Thread::Open,
        ));
    }
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
    sheets
}

pub fn shell_view(s: &ShellState, cx: &mut Cx<ShellState>) -> El {
    // Window size for this frame: the root's laid-out rect (rebuilds on
    // resize). Spec size stands in until the first layout lands.
    let (w, h) = cx
        .window_layout("stark-shell")
        .map(|r| (r[2], r[3]))
        .filter(|(w, h)| *w > 1.0 && *h > 1.0)
        .unwrap_or((crate::rune_canvas::MAP_W, crate::rune_canvas::MAP_H));
    // SAFETY: the view runs single-threaded on the UI thread.
    unsafe {
        WINDOW_W = w;
        WINDOW_H = h;
    }
    let mut tabs = row()
        .key("shell-tabs")
        .w_full()
        .h(TAB_H)
        .items_center()
        .gap(4.0)
        .px(16.0)
        .pb(1.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], ink("#ffffff14"));
    for id in s.canvas.sessions.order.clone() {
        tabs = tabs.child(session_tab(s, &id));
    }
    let new_style = {
        let mut style = rune::tok::TextStyle::new(12.0, 400.0);
        style.line_height = 12.0;
        style
    };
    let new_advance = rune::text::advance("New session", new_style);
    let new_right_pad = 11.0 - (new_advance.ceil() - new_advance);
    tabs = tabs.child(
        row()
            .key("session-new")
            .role("button")
            .label("New session")
            .h(30.0)
            .items_center()
            .gap(6.0)
            .pl(11.0)
            .pr(new_right_pad)
            .radius(8.0)
            .border(1.0, ink("#ffffff1f"))
            .border_dash(3.0, 3.0)
            .on_press(move |s: &mut ShellState| {
                s.canvas.new_session();
            })
            .child(rune::el().size(13.0, 13.0).inert().shape({
                let color = ink("#71717a");
                Shape::new()
                    .line(
                        [12.0, 5.0],
                        [12.0, 19.0],
                        crate::lucide_stroke(13.0),
                        LineCap::Round,
                        color,
                    )
                    .line(
                        [5.0, 12.0],
                        [19.0, 12.0],
                        crate::lucide_stroke(13.0),
                        LineCap::Round,
                        color,
                    )
                    .view_box(24.0, 24.0)
            }))
            .child(
                text("New session".to_string())
                    .key("session-new-label")
                    .font(new_style)
                    .color(ink("#71717a")),
            ),
    );
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
                .key("rail-new")
                .role("button")
                .label("New chat")
                .size(36.0, 36.0)
                .radius(12.0)
                .center()
                .on_press(move |s: &mut ShellState| {
                    s.canvas.new_session();
                })
                .child(
                    rune::el()
                        .key("rail-new-icon")
                        .size(17.0, 17.0)
                        .inert()
                        .shape(icon_new_chat(17.0, ink("#71717a"))),
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
                .label("Open settings")
                .size(36.0, 36.0)
                .radius(12.0)
                .center()
                .on_press(move |s: &mut ShellState| {
                    let id = s.canvas.active_id.clone();
                    if !id.is_empty() {
                        s.canvas.thread_mut(&id).settings_open = true;
                    }
                })
                .child(
                    rune::el()
                        .key("rail-settings-icon")
                        .size(17.0, 17.0)
                        .inert()
                        .shape(icon_settings(17.0, ink("#71717a"))),
                ),
        );
    let stage_w = (w - RAIL_W - if folded { 0.0 } else { PANEL_W }).max(320.0);
    let stage_h = (h - TAB_H).max(240.0);
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
                .px(20.0)
                .pt(12.0)
                .child(canvas_body::<ShellState>(&s.canvas)),
        );
    if let Some(overlay) = home_chat_overlay(s, stage_w, stage_h, h) {
        stage = stage.child(overlay);
    }
    let main = row()
        .key("stark-main")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(ink("#0a0a0a"))
        .child(rail)
        .child(if folded {
            rune::el().key("side-spacer").w(0.0)
        } else {
            side_panel(s)
        })
        .child(stage);
    let mut root = col()
        .key("stark-shell")
        .w_full()
        .h_full()
        .fill(ink("#0a0a0a"))
        .child(tabs)
        .child(main);
    if folded {
        // Drawer: absolute over the map's left edge, same panel content.
        root = root.child(
            rune::el()
                .key("side-drawer")
                .absolute(RAIL_W, TAB_H)
                .h((h - TAB_H).max(0.0))
                .child(side_panel(s)),
        );
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
    for sheet in active_chat_sheets(s) {
        root = root.child(sheet);
    }
    root
}

/// `poll` drains every session thread (runs + notes) whatever panel is up
/// and refreshes status rooms at most every 10–15s, so runs started from the
/// dock keep streaming on the map. Card `running` flags mirror thread liveness.
pub fn shell_poll(s: &mut ShellState, clock: f64, tx: &mut rune::Tx) -> bool {
    // Keep the canvas stage sized to the live window (React's container
    // drives `fitView`). While the camera is auto it refits on resize.
    let (win_w, win_h) = unsafe { (WINDOW_W, WINDOW_H) };
    let folded = win_w < PANEL_FOLD_W;
    let stage_w = (win_w - RAIL_W - if folded { 0.0 } else { PANEL_W } - 40.0).max(320.0);
    let stage_h = (win_h - TAB_H - 12.0).max(240.0);
    let mut changed = s.canvas.set_viewport(stage_w, stage_h);
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
    // Only the visible card's dock shows provider/models (React mounts one
    // PromptDock), so poll just that thread — not every open tab.
    if let Some(thread) = s.canvas.sheet_thread_mut() {
        if services_auto {
            thread.services_auto = true;
        }
        changed |= crate::rune_transcript::poll_provider_state(thread, clock);
        changed |= crate::rune_transcript::poll_models_state(thread, clock);
    }
    // Session threads: drain + sync card flags.
    let ids: Vec<String> = s.canvas.sessions.order.clone();
    for id in ids {
        if let Some(thread) = s.canvas.sessions.threads.get_mut(&id) {
            changed |= drain_run_inbox(thread, clock, tx);
            changed |= crate::notes::poll_notes_state(&mut thread.notes);
            changed |= crate::agent_config::poll_agent_config(&mut thread.agent_config);
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
        || s.canvas.agents.loading;
    if busy {
        tx.wake_after(std::time::Duration::from_millis(100));
    }
    changed
}

/// Drain the real-chat stream: adopt snapshots, apply per-chat history, and
/// auto-load once on the real app entry.
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
                s.chats.snapshot = snapshot.clone();
                s.canvas.apply_chat_snapshot(&snapshot);
                s.chats.loaded = true;
                s.chats.loading = false;
                s.chats.error = None;
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
                            thread.live = false;
                            thread.aborting = false;
                        }
                    }
                }
                changed = true;
            }
        }
    }
    // Only start a history fetch when the boot snapshot is not in flight:
    // both share one result stream.
    if !s.chats.loading {
        if let Some((ws, chat)) = s.canvas.pending_chat_fetch.take() {
            crate::chats::refresh_messages(&mut s.chats, &ws, &chat);
            changed = true;
        }
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
    // The real app entry (native window and browser) loads live chats; unit
    // tests construct `ShellState::default()` and stay offline.
    let mut shell = ShellState::default();
    shell.chats.auto = true;
    // The dock's provider/model pills load on mount (React's PromptDock
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

fn apply_panel_memory(s: &mut ShellState) {
    s.canvas.rail = RailTab::Memory;
}

/// The exact Inter variable face the browser loads from Google Fonts.
/// The built-in static Inter is close but not the same instance.
const SHELL_FONTS: &[&[u8]] = &[include_bytes!("../assets/InterVariable.ttf")];

pub fn shell_spec() -> AppSpec<ShellState> {
    let mut spec = AppSpec::new("Stark", (1280.0, 900.0), init_shell, shell_view);
    spec.scenes = &[
        ("home", apply_home),
        ("stream", apply_stream),
        ("trace", apply_trace),
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
    spec.fonts = SHELL_FONTS;
    // Chromium's Tailwind mono stack resolves `ui-monospace` to Consolas on
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
    // `STARK_FONT_SMOOTHING` is a parity-rig knob: DirectWrite is the
    // Windows default (Chromium's path), `off`/`on` are the portable
    // pipelines. It never changes the shipped default.
    let mut app = rune::App::new(shell_spec());
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
    fn home_card_width_matches_react_wrapper_gutters() {
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
        // Thread survives the close (reopen restores history); the closed
        // id is listed for the Add menu.
        assert!(rt.state.canvas.sessions.threads.contains_key(&id));
        assert!(rt.state.canvas.closed.iter().any(|(cid, _)| cid == &id));
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
        // Memory panel loads its board the same way (the rail mirrors React:
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
}
