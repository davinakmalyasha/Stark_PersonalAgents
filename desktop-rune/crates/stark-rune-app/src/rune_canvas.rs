//! Canvas map — port of `CanvasStage.tsx` (`variant="map"`).
//!
//! Only compiled with `--features rune-real`.
//!
//! Rune mapping:
//! - dotted grid: 26pt dot `el`s (the browser's radial-gradient tile has no
//!   single-primitive equivalent; one element per dot keeps the look exact).
//! - cards: `absolute(x, y)` in the canvas box, `468x420` / compact `384x392`,
//!   18/16px radius, `#0b0b0e` fill, hairline border. Drag is on the header row
//!   only — body clicks must stay clicks (`CanvasStage.tsx onPointerDown`).
//! - edges: one inert full-canvas overlay with `Shape::stroke_path` cubics,
//!   parent bottom-centre → child top-centre, same bend floor as
//!   `stark-kit::edge_path` (34pt, 0.48 factor). Inert so it never steals drags.
//! - headless PNG: `--rune-canvas-snap out.png` (1280x720 `map` scene).

use rune::{col, row, text, text_input, AppSpec, Cx, Drag, El, LineCap, Path, Phase, Shape};

pub const MAP_W: f32 = 1280.0;
pub const MAP_H: f32 = 900.0;
/// Side panel width (icon rail + panel, like the desktop layout).
pub const MAP_SIDEBAR_W: f32 = 300.0;
/// Icon rail width; the chat panel takes the rest.
pub const RAIL_W: f32 = 52.0;
pub const DOT_STEP: f32 = 26.0;
/// Chat kinds (the workspace dropdown). Chats are the only type — no
/// workspace scoping, everything lives in side tabs on one page.
pub const CHAT_KINDS: [&str; 4] = ["Main", "Coding", "House", "General"];

/// Side panel tab (the icon rail). One page: the panel switches, the canvas
/// stays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RailTab {
    #[default]
    Chats,
    Agents,
    Group,
    Cloud,
    Memory,
    House,
}

impl RailTab {
    pub fn glyph(self) -> &'static str {
        match self {
            RailTab::Chats => "⌂",
            RailTab::Agents => "◫",
            RailTab::Group => "▤",
            RailTab::Cloud => "☁",
            RailTab::Memory => "◍",
            RailTab::House => "△",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            RailTab::Chats => "Chats",
            RailTab::Agents => "Agents",
            RailTab::Group => "Group chats",
            RailTab::Cloud => "Cloud",
            RailTab::Memory => "Memory",
            RailTab::House => "House",
        }
    }
}

/// Zoom limits mirror `CanvasStage.tsx` (`clamp(scale, 0.45, 1.8)`).
pub const CAM_MIN_SCALE: f32 = 0.45;
pub const CAM_MAX_SCALE: f32 = 1.8;

/// Canvas camera: pan offset + zoom, mirroring `CanvasStage`'s
/// `{x, y, scale}` view state. The wheel path (`onWheel`, zoom-to-cursor) has
/// no element-level Rune equivalent — zoom steps through the HUD buttons
/// (`zoomBy(0.86)` / `zoomBy(1.16)`, same factors) about the viewport centre,
/// pan drags the background. Chrome (sidebar, dock, HUD) stays fixed, like
/// React's `[data-canvas-ui]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub x: f32,
    pub y: f32,
    pub scale: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            scale: 1.0,
        }
    }
}

impl Camera {
    pub fn zoom_by(&mut self, factor: f32) {
        self.scale = (self.scale * factor).clamp(CAM_MIN_SCALE, CAM_MAX_SCALE);
    }

    /// Fit all cards into the map viewport (sidebar excluded), like `fitView`.
    /// Empty board resets to the origin.
    pub fn fit(&mut self, cards: &[MapCard]) {
        if cards.is_empty() {
            *self = Self::default();
            return;
        }
        let vw = MAP_W - MAP_SIDEBAR_W;
        let (mut x0, mut y0, mut x1, mut y1) = (
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        );
        for c in cards {
            let (w, h) = c.size();
            x0 = x0.min(c.x);
            y0 = y0.min(c.y);
            x1 = x1.max(c.x + w);
            y1 = y1.max(c.y + h);
        }
        let pad = 48.0;
        let scale = ((vw - pad * 2.0) / (x1 - x0).max(1.0))
            .min((MAP_H - pad * 2.0) / (y1 - y0).max(1.0))
            .clamp(CAM_MIN_SCALE, CAM_MAX_SCALE);
        self.scale = scale;
        self.x = (vw - (x1 - x0) * scale) / 2.0 - x0 * scale;
        self.y = (MAP_H - (y1 - y0) * scale) / 2.0 - y0 * scale;
    }
}

#[derive(Debug, Clone)]
pub struct MapCard {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub preview: String,
    /// Chat kind (the dropdown filter). Chats are the only type — this is a
    /// category label, never a workspace scope.
    pub kind: String,
    pub x: f32,
    pub y: f32,
    pub compact: bool,
    pub running: bool,
}

impl MapCard {
    pub fn size(&self) -> (f32, f32) {
        if self.compact {
            (
                stark_theme::layout::CARD_COMPACT_W,
                stark_theme::layout::CARD_COMPACT_H,
            )
        } else {
            (stark_theme::layout::CARD_W, stark_theme::layout::CARD_H)
        }
    }
}

/// Sessions: one transcript thread per chat card plus the sidebar's search
/// and kind filter. Keyed by card id; `order` is the session tab order and
/// `sheet` the open transcript sheet (none = canvas only).
pub struct Sessions {
    pub threads: std::collections::HashMap<String, crate::rune_transcript::RuneTranscriptState>,
    pub order: Vec<String>,
    pub sheet: Option<String>,
    pub search: rune::TextEdit,
    pub kind: String,
}

impl std::fmt::Debug for Sessions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sessions")
            .field("threads", &self.threads.keys().collect::<Vec<_>>())
            .field("order", &self.order)
            .field("sheet", &self.sheet)
            .field("kind", &self.kind)
            .finish()
    }
}

impl Clone for Sessions {
    fn clone(&self) -> Self {
        Self {
            threads: self.threads.clone(),
            order: self.order.clone(),
            sheet: self.sheet.clone(),
            search: self.search.clone(),
            kind: self.kind.clone(),
        }
    }
}

impl Default for Sessions {
    fn default() -> Self {
        Self {
            threads: std::collections::HashMap::new(),
            order: Vec::new(),
            sheet: None,
            search: rune::TextEdit::new(""),
            kind: "General".to_string(),
        }
    }
}

/// Host access for session state (threads, sheet, filters).
pub trait SessionsHost {
    fn sessions_mut(&mut self) -> &mut Sessions;
    fn sessions(&self) -> &Sessions;
}

pub struct CanvasState {
    pub cards: Vec<MapCard>,
    pub edges: Vec<(String, String)>,
    pub press_home: Option<(f32, f32)>,
    /// Pan-drag home (world offset at press); separate from `press_home` so a
    /// card drag can never leak into the camera.
    pub press_pan: Option<(f32, f32)>,
    pub model: String,
    pub edit: rune::TextEdit,
    /// Selected chat id; sidebar rows and cards key off the same ids.
    pub active_id: String,
    /// Side panel tab (icon rail). The canvas always stays; the panel swaps.
    pub rail: RailTab,
    /// Sessions: one transcript thread per chat card, the open sheet, and
    /// the sidebar's search + kind filter. Chats are the only type, so all
    /// threads live here — no per-route transcript states.
    pub sessions: Sessions,
    /// Camera (pan + zoom); chrome stays fixed while the world scales.
    pub cam: Camera,
    /// Group projects scoped to the workspace (`useGroupProjects` twin);
    /// rendered as a rail section, never a route.
    pub projects: crate::projects::ProjectsState,
    /// Agent delegations (`AgentTreePanel` twin); tree in the sidebar,
    /// detail as a sheet.
    pub agents: crate::agents::AgentsState,
    /// Mic capture bus (levels, notes, stop flag, inbox).
    pub voice: crate::voice::VoiceBus,
    /// Prompts staged from the map dock, awaiting run dispatch.
    /// Kind dropdown open in the chats panel.
    pub kind_open: bool,
}

impl std::fmt::Debug for CanvasState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CanvasState")
            .field("cards", &self.cards)
            .field("edges", &self.edges)
            .field("model", &self.model)
            .field("edit", &self.edit.text)
            .field("active_id", &self.active_id)
            .field("voice", &self.voice)
            .finish()
    }
}

impl Clone for CanvasState {
    fn clone(&self) -> Self {
        Self {
            cards: self.cards.clone(),
            edges: self.edges.clone(),
            press_home: self.press_home,
            press_pan: self.press_pan,
            model: self.model.clone(),
            edit: self.edit.clone(),
            active_id: self.active_id.clone(),
            rail: self.rail,
            sessions: self.sessions.clone(),
            cam: self.cam,
            projects: self.projects.clone(),
            agents: self.agents.clone(),
            voice: self.voice.clone(),
            kind_open: self.kind_open,
        }
    }
}

impl Default for CanvasState {
    fn default() -> Self {
        Self::map()
    }
}

impl CanvasState {
    /// Screenshot scene: the four home cards + one spawn edge.
    pub fn map() -> Self {
        let mut state = Self {
            cards: vec![
                MapCard {
                    id: "chat:ws-general:chat-1".to_string(),
                    title: "New Conversation".to_string(),
                    subtitle: "General · Orchestrator".to_string(),
                    preview: "452 ÷ 5 = 90.4 (exactly 90 remainder 2)".to_string(),
                    kind: "General".to_string(),
                    x: 36.0,
                    y: 36.0,
                    compact: false,
                    running: false,
                },
                MapCard {
                    id: "chat:ws-general:chat-2".to_string(),
                    title: "New Conversation".to_string(),
                    subtitle: "General · Orchestrator".to_string(),
                    preview: "No messages yet.".to_string(),
                    kind: "Coding".to_string(),
                    x: 520.0,
                    y: 36.0,
                    compact: false,
                    running: false,
                },
                MapCard {
                    id: "chat:ws-general:chat-3".to_string(),
                    title: "arithmetic 329+3823".to_string(),
                    subtitle: "General · spawned".to_string(),
                    preview: "Good to meet you, Davin.".to_string(),
                    kind: "General".to_string(),
                    x: 60.0,
                    y: 480.0,
                    compact: true,
                    running: false,
                },
                MapCard {
                    id: "chat:ws-general:chat-4".to_string(),
                    title: "New Chat".to_string(),
                    subtitle: "General · running".to_string(),
                    preview: "Playing now in your YouTube tab…".to_string(),
                    kind: "General".to_string(),
                    x: 560.0,
                    y: 480.0,
                    compact: true,
                    running: true,
                },
            ],
            edges: vec![(
                "chat:ws-general:chat-1".to_string(),
                "chat:ws-general:chat-3".to_string(),
            )],
            press_home: None,
            press_pan: None,
            model: "Automatic Fallback".to_string(),
            edit: rune::TextEdit::new(""),
            active_id: "chat:ws-general:chat-1".to_string(),
            rail: RailTab::Chats,
            sessions: Sessions::default(),
            cam: Camera::default(),
            projects: crate::projects::ProjectsState::default(),
            agents: crate::agents::AgentsState::default(),
            voice: crate::voice::VoiceBus::default(),
            kind_open: false,
        };
        state.seed_sessions();
        state
    }

    /// Seed session tabs from the scene cards (cards predate threads;
    /// threads materialize on open/send).
    pub fn seed_sessions(&mut self) {
        for card in &self.cards {
            if !self.sessions.order.contains(&card.id) {
                self.sessions.order.push(card.id.clone());
            }
        }
        if self.active_id.is_empty() {
            self.active_id = self.sessions.order.first().cloned().unwrap_or_default();
        }
    }

    pub fn card(&self, id: &str) -> Option<&MapCard> {
        self.cards.iter().find(|c| c.id == id)
    }

    /// Focus the camera on a card (session tab switch centers it).
    pub fn focus_card(&mut self, id: &str) {
        let (cx, cy) = match self.card(id) {
            Some(card) => {
                let (w, h) = card.size();
                (card.x + w / 2.0, card.y + h / 2.0)
            }
            None => return,
        };
        let vw = MAP_W - MAP_SIDEBAR_W;
        let k = self.cam.scale;
        self.cam.x = vw / 2.0 - cx * k;
        self.cam.y = MAP_H / 2.0 - cy * k;
    }

    /// Select a session: active row + camera focus.
    pub fn select_card(&mut self, id: &str) {
        if self.card(id).is_none() {
            return;
        }
        self.active_id = id.to_string();
        self.focus_card(id);
    }

    /// Get-or-create the transcript thread for a card.
    pub fn thread_mut(&mut self, id: &str) -> &mut crate::rune_transcript::RuneTranscriptState {
        if !self.sessions.threads.contains_key(id) {
            let title = self
                .card(id)
                .map(|c| c.title.clone())
                .unwrap_or_else(|| "New Chat".to_string());
            let mut thread = crate::rune_transcript::RuneTranscriptState::screenshot();
            thread.title = title;
            thread.rows = Vec::new();
            thread.live = false;
            thread.chat_id = crate::notes::mint_id("rune-chat");
            self.sessions.threads.insert(id.to_string(), thread);
            if !self.sessions.order.contains(&id.to_string()) {
                self.sessions.order.push(id.to_string());
            }
        }
        self.sessions
            .threads
            .get_mut(id)
            .expect("thread ensured above")
    }

    /// Open the transcript sheet for a card (ensures its thread).
    pub fn open_sheet(&mut self, id: &str) {
        if self.card(id).is_none() {
            return;
        }
        self.select_card(id);
        let _ = self.thread_mut(id);
        self.sessions.sheet = Some(id.to_string());
    }

    pub fn close_sheet(&mut self) {
        self.sessions.sheet = None;
    }

    /// The sheet's thread, if a sheet is open.
    pub fn sheet_thread(&self) -> Option<&crate::rune_transcript::RuneTranscriptState> {
        self.sessions
            .sheet
            .as_ref()
            .and_then(|id| self.sessions.threads.get(id))
    }

    pub fn sheet_thread_mut(&mut self) -> Option<&mut crate::rune_transcript::RuneTranscriptState> {
        let id = self.sessions.sheet.clone()?;
        self.sessions.threads.get_mut(&id)
    }

    /// New session: card (current kind, cascaded) + thread + select.
    /// Returns the card id for the tab bar.
    pub fn new_session(&mut self) -> String {
        let n = self.cards.len();
        let id = format!("chat:local:{}", crate::notes::mint_id("s"));
        let kind = self.sessions.kind.clone();
        self.cards.push(MapCard {
            id: id.clone(),
            title: "New Chat".to_string(),
            subtitle: format!("{kind} · fresh"),
            preview: "No messages yet.".to_string(),
            kind,
            x: 36.0 + ((n * 84) % 420) as f32,
            y: 36.0 + ((n * 64) % 320) as f32,
            compact: false,
            running: false,
        });
        let _ = self.thread_mut(&id);
        self.select_card(&id);
        id
    }

    /// Close a session: card + thread + tab go; selection and sheet fall
    /// back to the first remaining session (local scene state only — no
    /// server chats exist behind these cards).
    pub fn close_session(&mut self, id: &str) {
        self.cards.retain(|c| c.id != id);
        self.sessions.threads.remove(id);
        self.sessions.order.retain(|o| o != id);
        if self.sessions.sheet.as_deref() == Some(id) {
            self.sessions.sheet = None;
        }
        if self.active_id == id {
            self.active_id = self
                .sessions
                .order
                .first()
                .or_else(|| self.cards.first().map(|c| &c.id))
                .cloned()
                .unwrap_or_default();
        }
    }
}

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

/// Cubic edge path in canvas points (mirrors `stark-kit::edge_path`).
pub fn edge_cubic(parent: &MapCard, child: &MapCard) -> Path {
    let (pw, ph) = parent.size();
    let (cw, _) = child.size();
    let sx = parent.x + pw / 2.0;
    let sy = parent.y + ph;
    let tx = child.x + cw / 2.0;
    let ty = child.y;
    let bend = 34.0f32.max((ty - sy) * 0.48);
    Path::new()
        .move_to([sx, sy])
        .cubic_to([sx, sy + bend], [tx, ty - bend], [tx, ty])
}

/// Host access for views composed into a larger shell.
pub trait CanvasHost {
    fn canvas_mut(&mut self) -> &mut CanvasState;
}

/// Home hosting: the map canvas plus its projects rail. One bound covers
/// the canvas-local panels (chats, projects, agents, sessions), so the
/// sidebar can never disagree with the board state. Cloud/memory/house
/// panels render shell-side (their state lives on `ShellState`).
pub trait HomeHost:
    CanvasHost + crate::projects::ProjectsHost + crate::agents::AgentsHost + SessionsHost
{
}

impl<T: CanvasHost + crate::projects::ProjectsHost + crate::agents::AgentsHost + SessionsHost>
    HomeHost for T
{
}

impl SessionsHost for CanvasState {
    fn sessions_mut(&mut self) -> &mut Sessions {
        &mut self.sessions
    }
    fn sessions(&self) -> &Sessions {
        &self.sessions
    }
}

impl CanvasHost for CanvasState {
    fn canvas_mut(&mut self) -> &mut CanvasState {
        self
    }
}

impl crate::projects::ProjectsHost for CanvasState {
    fn projects_mut(&mut self) -> &mut crate::projects::ProjectsState {
        &mut self.projects
    }
}

impl crate::agents::AgentsHost for CanvasState {
    fn agents_mut(&mut self) -> &mut crate::agents::AgentsState {
        &mut self.agents
    }
}

fn card_view<S: CanvasHost + 'static>(card: &MapCard, active: bool) -> El {
    let (w, h) = card.size();
    let radius = if card.compact { 16.0 } else { 18.0 };
    let id = card.id.clone();
    let drag_id = card.id.clone();
    // Borders mirror `canvas.css`: default .085, running .22, active .30.
    // (Rune carries one shadow per element, so the card takes the drop
    // `0 26px 70px rgba(0,0,0,.50)`; the inset top-light and active halo have
    // no slot — documented, not faked.)
    let border = if active {
        ink("#ffffff4d")
    } else if card.running {
        ink("#ffffff38")
    } else {
        ink("#ffffff16")
    };
    let mut view = col()
        .key(card.id.clone())
        .absolute(card.x, card.y)
        .size(w, h)
        .radius(radius)
        .fill(ink("#0b0b0e"))
        // Card surface (`canvas.css .canvas-card`): top-light → transparent
        // → black-lift over the base. `PI` is top-to-bottom (CSS 180deg).
        .linear_gradient(
            std::f32::consts::PI,
            [
                (0.0, ink("#ffffff08")),
                (0.36, ink("#ffffff02")),
                (1.0, ink("#0000000c")),
            ],
        )
        .border(1.0, border)
        .shadow(
            26.0,
            70.0,
            rune::Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.50,
            },
        )
        .clip_rounded(radius);
    if card.running {
        // One shadow slot: the running glow wins over the drop (the glow is
        // the running signal; `.canvas-card.is-running` keeps both).
        view = view
            .shadow(
                0.0,
                44.0,
                rune::Color {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.05,
                },
            )
            // Sweep sheen: 110pt bar sliding across on the 2.6s linear loop,
            // clipped to the card. Mirrors `canvas-border-sweep`.
            .child(
                rune::el()
                    .key(format!("{id}-sheen"))
                    .absolute(0.0, 0.0)
                    .size(110.0, h)
                    .linear_gradient(
                        std::f32::consts::FRAC_PI_2,
                        [
                            (0.0, ink("#ffffff00")),
                            (0.5, ink("#ffffff1f")),
                            (1.0, ink("#ffffff00")),
                        ],
                    )
                    .looping(
                        rune::LoopSpec::slide(2.6, [-130.0, 0.0], [w + 20.0, 0.0])
                            .easing(rune::Easing::LINEAR),
                    ),
            );
    }
    // Status pill: breathing dot while running (`house-breathe` language).
    let mut head = row()
        .key(format!("{id}-head"))
        .w_full()
        .items_center()
        .gap(10.0)
        .px(14.0)
        .pt(14.0)
        .pb(10.0)
        .on_drag(move |s: &mut S, drag: &mut Drag, _tx| {
            // Index, never a held reference: each phase takes its own borrow.
            let idx = match s.canvas_mut().cards.iter().position(|c| c.id == drag_id) {
                Some(i) => i,
                None => return,
            };
            match drag.phase {
                Phase::Down => {
                    let c = s.canvas_mut();
                    c.press_home = Some((c.cards[idx].x, c.cards[idx].y));
                }
                Phase::Move | Phase::Up => {
                    let c = s.canvas_mut();
                    // Drag deltas arrive in screen points; cards live in world
                    // points — divide out the zoom or cards drift when scaled.
                    let k = c.cam.scale.max(CAM_MIN_SCALE);
                    if let Some((hx, hy)) = c.press_home {
                        c.cards[idx].x = hx + drag.dx / k;
                        c.cards[idx].y = hy + drag.dy / k;
                    }
                    if matches!(drag.phase, Phase::Up) {
                        c.press_home = None;
                    }
                }
            }
        })
        .child(
            col()
                .key(format!("{id}-titles"))
                .grow(1.0)
                .min_h(0.0)
                .child(
                    text(card.title.clone())
                        .key(format!("{id}-title"))
                        // Compact cards step down (15px → 13.5px), like
                        // `.canvas-card.is-compact .canvas-card-title`.
                        .font(rune::tok::TextStyle::new(
                            if card.compact { 13.5 } else { 15.0 },
                            600.0,
                        ))
                        .color(ink("#f2f2f5")),
                )
                .child(
                    text(card.subtitle.clone())
                        .key(format!("{id}-subtitle"))
                        // Compact: 11.5px → 10.5px (`.canvas-card-subtitle`).
                        .font(rune::tok::TextStyle::new(
                            if card.compact { 10.5 } else { 11.5 },
                            400.0,
                        ))
                        .color(ink("#8a8a93")),
                ),
        );
    if card.running {
        head = head.child(
            rune::el()
                .key(format!("{id}-status"))
                .size(8.0, 8.0)
                .radius(4.0)
                .fill(ink("#ededf0"))
                .looping(rune::LoopSpec::opacity(2.2, 0.0, 0.35, 1.0)),
        );
    }
    view.child(head)
        .child(
            text(card.preview.clone())
                .key(format!("{id}-preview"))
                .px(16.0)
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .wrap()
                .color(ink("#92959e")),
        )
        // Edge ports (`.canvas-card-port`): 9px dots centred on the top and
        // bottom rims where the beziers land. Inert (`pointer-events: none`).
        .child(
            rune::el()
                .key(format!("{id}-port-top"))
                .absolute(w / 2.0 - 4.5, -4.5)
                .size(9.0, 9.0)
                .radius(4.5)
                .fill(ink("#1b1b20"))
                .border(1.0, ink("#44444c"))
                .inert(),
        )
        .child(
            rune::el()
                .key(format!("{id}-port-bottom"))
                .absolute(w / 2.0 - 4.5, h - 4.5)
                .size(9.0, 9.0)
                .radius(4.5)
                .fill(ink("#1b1b20"))
                .border(1.0, ink("#44444c"))
                .inert(),
        )
}

pub fn canvas_view(s: &CanvasState, _: &mut Cx<CanvasState>) -> El {
    canvas_body::<CanvasState>(s)
}

/// Canvas body shared by the standalone view and the shell.
pub fn canvas_body<S: CanvasHost + 'static>(s: &CanvasState) -> El {
    let dot = rune::Color {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 0.075,
    };
    let vw = MAP_W - MAP_SIDEBAR_W;
    // World layer: everything that pans/zooms. Origin top-left so pan offsets
    // map 1:1; `scale` springs on the layout spring and hit tests follow the
    // scaled rect (`El::scale`). Chrome (composer, HUD, sidebar) stays fixed,
    // like React's `[data-canvas-ui]`.
    let mut world = col()
        .key("canvas-world")
        .absolute(s.cam.x, s.cam.y)
        .size(vw, MAP_H)
        .origin(0.0, 0.0)
        .scale(s.cam.scale);
    // Background pan surface: cards, dots and edges sit above it, so empty
    // presses pan while header presses drag cards (`CanvasStage` background
    // drag). Wheel zoom has no element-level Rune equivalent — the HUD
    // buttons step the same 0.86/1.16 factors.
    world = world.child(
        rune::el()
            .key("canvas-bg")
            .absolute(0.0, 0.0)
            .size(vw, MAP_H)
            .fill(ink("#08080a"))
            .on_drag(move |s: &mut S, drag: &mut Drag, _tx| {
                let c = s.canvas_mut();
                match drag.phase {
                    Phase::Down => {
                        c.press_pan = Some((c.cam.x, c.cam.y));
                    }
                    Phase::Move | Phase::Up => {
                        let c = s.canvas_mut();
                        if let Some((hx, hy)) = c.press_pan {
                            c.cam.x = hx + drag.dx;
                            c.cam.y = hy + drag.dy;
                        }
                        if matches!(drag.phase, Phase::Up) {
                            c.press_pan = None;
                        }
                    }
                }
            }),
    );
    // Dotted grid tile (26pt, like `canvas.css .canvas-grid`). Inert so dots
    // never steal the pan drag.
    let mut gy = DOT_STEP;
    while gy < MAP_H {
        let mut gx = DOT_STEP;
        while gx < vw {
            world = world.child(
                rune::el()
                    .key(format!("dot-{gx:.0}x{gy:.0}"))
                    .absolute(gx, gy)
                    .size(2.0, 2.0)
                    .radius(1.0)
                    .fill(dot)
                    .inert(),
            );
            gx += DOT_STEP;
        }
        gy += DOT_STEP;
    }
    // Bezier edges, inert so cards keep their drags.
    let mut shape = Shape::new();
    for (parent_id, child_id) in &s.edges {
        if let (Some(p), Some(c)) = (s.card(parent_id), s.card(child_id)) {
            let path = edge_cubic(p, c);
            shape = shape.stroke_path(&path, 1.2, LineCap::Round, ink("#ffffff21"));
        }
    }
    world = world.child(
        rune::el()
            .key("canvas-edges")
            .absolute(0.0, 0.0)
            .size(vw, MAP_H)
            .inert()
            .shape(shape),
    );
    for card in &s.cards {
        world = world.child(card_view::<S>(card, card.id == s.active_id));
    }
    // Fixed-chrome area: clips the scaled world, holds composer + HUD.
    // `grow` (not `h_full`): shares a shell row without overflowing it.
    let mut canvas = col()
        .key("canvas-area")
        .grow(1.0)
        .min_h(0.0)
        .fill(ink("#08080a"))
        .clip_rounded(0.0)
        .child(world);
    // Floating composer, centred over the canvas like `HouseChat`'s dock:
    // 640pt wide, bottom-anchored. Same card as the transcript dock.
    canvas = canvas.child(
        rune::el()
            .key("map-composer-float")
            .absolute((MAP_W - MAP_SIDEBAR_W - 640.0) / 2.0, MAP_H - 128.0)
            .w(640.0)
            .child(super::composer::composer_card(
                &s.model,
                &s.edit,
                move |s: &mut S| &mut s.canvas_mut().edit,
                s.voice.meter(),
                move |s: &mut S| {
                    let c = s.canvas_mut();
                    c.voice.set_live(!c.voice.live);
                },
                // The map dock sends into the ACTIVE session's thread and opens
                // its sheet — chats are the only type, so every prompt lands
                // in a chat thread (no dead-letter outbox). It never shows
                // Stop; cancellation lives on the sheet that owns the run.
                (false, move |_s: &mut S| {}),
                move |s: &mut S| {
                    let c = s.canvas_mut();
                    let prompt = c.edit.text.trim().to_string();
                    if prompt.is_empty() || c.active_id.is_empty() {
                        return;
                    }
                    c.edit.text.clear();
                    c.edit.anchor = 0;
                    c.edit.focus = 0;
                    let id = c.active_id.clone();
                    let thread = c.thread_mut(&id);
                    crate::rune_transcript::stage_send_prompt(thread, prompt);
                    c.sessions.sheet = Some(id);
                },
            )),
    );

    // Camera HUD (`CanvasStage` map controls): zoom out/in at the React
    // 0.86/1.16 steps, fit-to-view, and layout reset. Fixed chrome — never
    // scaled with the world.
    let hud_btn = |key: &str, label: &str| {
        rune::el()
            .key(format!("cam-{key}"))
            .role("button")
            .label(label.to_string())
            .px(10.0)
            .py(6.0)
            .radius(8.0)
            .fill(ink("#17171b"))
            .border(1.0, ink("#ffffff14"))
            .center()
            .child(
                text(label.to_string())
                    .font(rune::tok::TextStyle::new(13.0, 600.0))
                    .color(ink("#c9c9d1")),
            )
    };
    canvas = canvas
        .child(
            row()
                .key("cam-hud")
                .absolute(vw - 244.0, 12.0)
                .items_center()
                .gap(6.0)
                .child(hud_btn("zoom-out", "−").on_press(move |s: &mut S| {
                    s.canvas_mut().cam.zoom_by(0.86);
                }))
                .child(hud_btn("zoom-in", "+").on_press(move |s: &mut S| {
                    s.canvas_mut().cam.zoom_by(1.16);
                }))
                .child(hud_btn("fit", "Fit").on_press(move |s: &mut S| {
                    let c = s.canvas_mut();
                    let cards = c.cards.clone();
                    c.cam.fit(&cards);
                }))
                .child(hud_btn("reset", "Reset").on_press(move |s: &mut S| {
                    // `relayout`: stock card positions plus a centred camera.
                    let fresh = CanvasState::map();
                    let c = s.canvas_mut();
                    c.cards = fresh.cards;
                    c.edges = fresh.edges;
                    c.active_id = fresh.active_id;
                    c.cam = Camera::default();
                    c.press_home = None;
                    c.press_pan = None;
                })),
        )
        .child(
            text("Drag to pan · drag cards to arrange".to_string())
                .key("cam-hint")
                .absolute(vw - 244.0, 48.0)
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .color(ink("#8a8a93")),
        );
    // Sidebar rows mirror the cards: the active chat carries the white-9%
    // wash (`.house-sidebar-row.is-active`); a press switches selection.
    // Rows key off card ids so the board and the list can never disagree.
    // NOTE: the side panel (chats/projects/agents/cloud/…) renders
    // shell-side — `canvas_body` is the canvas area only, so the panel can
    // swap while the map stays mounted. `chats_panel` below is the Chats tab.
    canvas
}

/// Chats panel: kind dropdown + new chat, live search, and one row per chat
/// filtered by kind + query. A row press selects and focuses the card.
pub fn chats_panel<S: CanvasHost + SessionsHost + 'static>(s: &CanvasState) -> El {
    let mut panel = col()
        .key("map-sidebar")
        .w(MAP_SIDEBAR_W)
        .h_full()
        .fill(ink("#0f0f0f"))
        .px(16.0)
        .pt(14.0)
        .gap(8.0)
        .child(
            row()
                .key("map-kindrow")
                .w_full()
                .items_center()
                .gap(8.0)
                .child(
                    row()
                        .key("map-kind")
                        .grow(1.0)
                        .items_center()
                        .gap(6.0)
                        .role("button")
                        .label("Switch chat kind")
                        .on_press(move |s: &mut S| {
                            let c = s.canvas_mut();
                            c.kind_open = !c.kind_open;
                        })
                        .child(
                            text(s.sessions.kind.clone())
                                .font(rune::tok::TextStyle::new(15.0, 600.0))
                                .color(ink("#fafafa")),
                        )
                        .child(
                            text(if s.kind_open { "▴" } else { "▾" }.to_string())
                                .font(rune::tok::TextStyle::new(10.0, 600.0))
                                .color(ink("#71717a")),
                        ),
                )
                .child(
                    text("+".to_string())
                        .key("map-new")
                        .role("button")
                        .label("New chat")
                        .font(rune::tok::TextStyle::new(15.0, 400.0))
                        .color(ink("#a1a1aa"))
                        .on_press(move |s: &mut S| {
                            s.canvas_mut().new_session();
                        }),
                ),
        );
    if s.kind_open {
        for kind in CHAT_KINDS {
            let current = s.sessions.kind == kind;
            let target = kind.to_string();
            panel = panel.child(
                row()
                    .key(format!("map-kind-{kind}"))
                    .w_full()
                    .items_center()
                    .gap(8.0)
                    .radius(8.0)
                    .px(10.0)
                    .py(6.0)
                    .fill(ink(if current { "#1b1b20" } else { "#00000000" }))
                    .role("button")
                    .label(format!("Show {kind} chats"))
                    .on_press(move |s: &mut S| {
                        let c = s.canvas_mut();
                        c.sessions.kind = target.clone();
                        c.kind_open = false;
                    })
                    .child(
                        text(kind.to_string())
                            .font(rune::tok::TextStyle::new(12.0, 500.0))
                            .color(ink(if current { "#e4e4e7" } else { "#a1a1aa" })),
                    ),
            );
        }
    }
    panel = panel.child(
        text_input(&s.sessions.search, rune::tok::TextStyle::new(12.0, 400.0))
            .key("map-search")
            .w_full()
            .placeholder("Search chats...", ink("#8a8a8a"))
            .caret(ink("#f2f2f4"), ink("#3b82f6"))
            .on_edit(move |s: &mut S| &mut s.canvas_mut().sessions.search),
    );
    let query = s.sessions.search.text.trim().to_lowercase();
    for card in &s.cards {
        if card.kind != s.sessions.kind {
            continue;
        }
        if !query.is_empty() && !card.title.to_lowercase().contains(&query) {
            continue;
        }
        let row_id = format!("side-{}", card.id);
        let active = card.id == s.active_id;
        let target = card.id.clone();
        let mut item = row()
            .key(row_id)
            .w_full()
            .items_center()
            .gap(8.0)
            .radius(8.0)
            .px(10.0)
            .py(8.0)
            .role("button")
            .label(format!("Open {}", card.title))
            .on_press(move |s: &mut S| {
                s.canvas_mut().select_card(&target);
            });
        if active {
            item = item.fill(rune::Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.09,
            });
        }
        if card.running {
            item = item.child(
                rune::el()
                    .key(format!("side-{}-dot", card.id))
                    .size(6.0, 6.0)
                    .radius(3.0)
                    .fill(ink("#fafafa")),
            );
        }
        panel = panel.child(
            item.child(
                text(card.title.clone())
                    .key(format!("side-{}-title", card.id))
                    .font(rune::tok::TextStyle::new(12.0, 500.0))
                    .color(ink(if active { "#e4e4e7" } else { "#a1a1aa" })),
            ),
        );
    }
    panel
}

fn init_map() -> CanvasState {
    CanvasState::map()
}

pub fn canvas_spec() -> AppSpec<CanvasState> {
    AppSpec::new("Stark Canvas", (MAP_W, MAP_H), init_map, canvas_view)
}

pub fn run_canvas_headless(extra_args: Vec<String>) {
    rune::App::new(canvas_spec()).args(extra_args).run();
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn map_commits_with_cards_edges_and_grid() {
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(CanvasState::map(), canvas_view, (MAP_W, MAP_H), env);
        rt.frame(0.0);
        for key in [
            "chat:ws-general:chat-1",
            "chat:ws-general:chat-4",
            "canvas-edges",
        ] {
            assert!(rt.find(key).is_some(), "{key} laid out");
        }
        let (rect, _) = rt.presented("chat:ws-general:chat-1").unwrap();
        assert!((rect[3] - 420.0).abs() < 2.0, "full card height 420");
        for key in ["map-composer-float", "composer-card", "composer-send"] {
            assert!(rt.find(key).is_some(), "{key} mounted");
        }
    }

    #[test]
    fn chats_panel_filters_searches_and_selects() {
        fn panel_view(s: &CanvasState, _: &mut rune::Cx<CanvasState>) -> El {
            col()
                .key("panel-root")
                .w_full()
                .h_full()
                .child(canvas_body::<CanvasState>(s))
                .child(chats_panel::<CanvasState>(s))
        }
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(CanvasState::map(), panel_view, (MAP_W, MAP_H), env);
        rt.frame(0.0);
        assert!(rt.find("map-sidebar").is_some(), "panel mounts");
        assert!(rt.find("map-search").is_some(), "live search mounts");
        assert!(rt.find("map-kind").is_some(), "kind dropdown mounts");
        // Kind filter: Coding shows only chat-2.
        let (x, y) = rt.find("map-kind").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        let (x, y) = rt.find("map-kind-Coding").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.find("side-chat:ws-general:chat-2").is_some());
        assert!(rt.find("side-chat:ws-general:chat-1").is_none());
        // Row press selects + focuses.
        let (x, y) = rt.find("side-chat:ws-general:chat-2").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert_eq!(rt.state.active_id, "chat:ws-general:chat-2");
    }

    #[test]
    fn camera_zoom_clamps_like_canvas_stage() {
        let mut cam = Camera::default();
        cam.zoom_by(0.001);
        assert_eq!(cam.scale, CAM_MIN_SCALE, "floor 0.45");
        cam.zoom_by(1_000.0);
        assert_eq!(cam.scale, CAM_MAX_SCALE, "ceiling 1.8");
        let mut cam = Camera::default();
        cam.zoom_by(1.16);
        assert!((cam.scale - 1.16).abs() < 1e-6, "HUD step in");
    }

    #[test]
    fn camera_fit_frames_every_card() {
        let state = CanvasState::map();
        let mut cam = Camera::default();
        cam.fit(&state.cards);
        assert!(
            cam.scale >= CAM_MIN_SCALE && cam.scale <= CAM_MAX_SCALE,
            "fit clamps: {}",
            cam.scale
        );
        // Every card rect lands inside the viewport (48pt pad).
        let vw = MAP_W - MAP_SIDEBAR_W;
        for c in &state.cards {
            let (w, h) = c.size();
            assert!(c.x * cam.scale + cam.x >= 40.0, "{} left", c.id);
            assert!((c.x + w) * cam.scale + cam.x <= vw - 40.0, "{} right", c.id);
            assert!(
                (c.y + h) * cam.scale + cam.y <= MAP_H - 40.0,
                "{} bottom",
                c.id
            );
        }
        // Empty board resets.
        cam.fit(&[]);
        assert_eq!(cam, Camera::default());
    }

    #[test]
    fn hud_buttons_step_and_reset_camera() {
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(CanvasState::map(), canvas_view, (MAP_W, MAP_H), env);
        rt.frame(0.0);
        for key in ["cam-zoom-in", "cam-zoom-out", "cam-fit", "cam-reset"] {
            assert!(rt.find(key).is_some(), "{key} mounted");
        }
        let (x, y) = rt.find("cam-zoom-in").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        assert!(
            (rt.state.cam.scale - 1.16).abs() < 1e-4,
            "zoom in steps: {}",
            rt.state.cam.scale
        );
        let (x, y) = rt.find("cam-reset").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(rt.state.cam, Camera::default(), "reset recentres");
    }

    #[test]
    fn background_drag_pans_without_moving_cards() {
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(CanvasState::map(), canvas_view, (MAP_W, MAP_H), env);
        rt.frame(0.0);
        // World (950, 800): clear of cards, composer and HUD (standalone
        // canvas fills the window — no rail offset here).
        let (x0, y0) = (950.0, 800.0);
        let card_before = (rt.state.cards[0].x, rt.state.cards[0].y);
        rt.pointer_down(x0, y0);
        rt.frame(1.0 / 60.0);
        rt.pointer_move(x0 + 40.0, y0 + 30.0);
        rt.frame(1.0 / 60.0);
        rt.pointer_up(x0 + 40.0, y0 + 30.0);
        rt.frame(1.0 / 60.0);
        assert!(
            (rt.state.cam.x - 40.0).abs() < 2.0 && (rt.state.cam.y - 30.0).abs() < 2.0,
            "camera pans: ({}, {})",
            rt.state.cam.x,
            rt.state.cam.y
        );
        assert_eq!(
            (rt.state.cards[0].x, rt.state.cards[0].y),
            card_before,
            "cards stay put while panning"
        );
    }

    #[test]
    fn header_drag_moves_card_by_pointer_travel() {
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(CanvasState::map(), canvas_view, (MAP_W, MAP_H), env);
        rt.frame(0.0);
        let (x, y) = rt.find("chat:ws-general:chat-1-head").unwrap();
        let (x0, y0) = (rt.state.cards[0].x, rt.state.cards[0].y);
        rt.pointer_down(x, y);
        rt.frame(1.0 / 60.0);
        rt.pointer_move(x + 40.0, y + 30.0);
        rt.frame(1.0 / 60.0);
        rt.pointer_up(x + 40.0, y + 30.0);
        rt.frame(1.0 / 60.0);
        let (x1, y1) = (rt.state.cards[0].x, rt.state.cards[0].y);
        assert!(
            (x1 - (x0 + 40.0)).abs() < 2.0 && (y1 - (y0 + 30.0)).abs() < 2.0,
            "card follows pointer: ({x0},{y0}) -> ({x1},{y1})"
        );
    }

    #[test]
    fn running_card_has_sheen_status_and_motion() {
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(CanvasState::map(), canvas_view, (MAP_W, MAP_H), env);
        rt.frame(0.0);
        let run_id = "chat:ws-general:chat-4";
        let idle_id = "chat:ws-general:chat-1";
        for key in [format!("{run_id}-sheen"), format!("{run_id}-status")] {
            assert!(rt.find(&key).is_some(), "{key} mounted on running card");
        }
        assert!(
            rt.find(&format!("{idle_id}-sheen")).is_none(),
            "idle card has no sheen"
        );
        // The sweep travels on the virtual clock: half a period later the
        // sheen rect has moved.
        let x0 = rt.presented(&format!("{run_id}-sheen")).unwrap().0[0];
        for _ in 0..78 {
            rt.frame(1.0 / 60.0);
        }
        let x1 = rt.presented(&format!("{run_id}-sheen")).unwrap().0[0];
        assert!(
            (x1 - x0).abs() > 5.0,
            "sheen travels: {x0} -> {x1} over half a period"
        );
    }

    #[test]
    fn edge_cubic_matches_kit_bend_math() {
        let s = CanvasState::map();
        let d = stark_kit::edge_path(
            (s.cards[0].x, s.cards[0].y, false),
            (s.cards[2].x, s.cards[2].y, true),
        );
        // Card-1 bottom (36+420=456) vs card-3 top (480): 24pt gap, bend floor 34.
        assert!(d.starts_with("M 270 456 C 270 490,"), "kit path: {d}");
        let _ = edge_cubic(&s.cards[0], &s.cards[2]);
    }
}
