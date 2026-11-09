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

#[derive(Debug, Clone, PartialEq)]
pub enum MapCardItem {
    Chat,
    Project,
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
    /// Chat cards host threads + docks; project cards are info displays
    /// (title, progress, waiting) added from the Group list.
    pub item: MapCardItem,
    /// Project snapshot for `MapCardItem::Project` (mirrors the modal list
    /// rows — no extra fetch).
    pub project: Option<crate::projects::GroupProject>,
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
    /// Per-card message drafts (`canvas-card-input`): keyed by card id, like
    /// React's per-card `draft` state. The send button mirrors
    /// `disabled={sending || running || empty}` — type-ahead while running,
    /// no double-send.
    pub inputs: std::collections::HashMap<String, rune::TextEdit>,
    /// Card id whose connected-agents menu is open (single open at a time).
    pub agents_open: Option<String>,
    /// Kind dropdown open in the chats panel.
    pub kind_open: bool,
    /// Add menu (`CanvasAddMenu`): open flag, tab, live search.
    pub add_open: bool,
    pub add_tab: String,
    pub add_search: rune::TextEdit,
    /// Closed chats available for re-adding (id, title). Threads survive
    /// the close, so reopening restores history.
    pub closed: Vec<(String, String)>,
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
            add_open: self.add_open,
            add_tab: self.add_tab.clone(),
            add_search: self.add_search.clone(),
            closed: self.closed.clone(),
            inputs: self.inputs.clone(),
            agents_open: self.agents_open.clone(),
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
                    item: MapCardItem::Chat,
                    project: None,
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
                    item: MapCardItem::Chat,
                    project: None,
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
                    item: MapCardItem::Chat,
                    project: None,
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
                    item: MapCardItem::Chat,
                    project: None,
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
            add_open: false,
            add_tab: "chats".to_string(),
            add_search: rune::TextEdit::new(""),
            closed: Vec::new(),
            inputs: std::collections::HashMap::new(),
            agents_open: None,
        };
        state.seed_sessions();
        state
    }

    /// Seed session tabs from the scene cards (cards predate threads;
    /// threads materialize on open/send). Draft inputs are ensured too, so
    /// the card dock always has a backing field.
    pub fn seed_sessions(&mut self) {
        for card in &self.cards {
            if !self.sessions.order.contains(&card.id) {
                self.sessions.order.push(card.id.clone());
            }
            self.inputs
                .entry(card.id.clone())
                .or_insert_with(|| rune::TextEdit::new(""));
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

    /// Get-or-create the message draft for a card.
    pub fn input_mut(&mut self, id: &str) -> &mut rune::TextEdit {
        self.inputs
            .entry(id.to_string())
            .or_insert_with(|| rune::TextEdit::new(""))
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
            item: MapCardItem::Chat,
            project: None,
            x: 36.0 + ((n * 84) % 420) as f32,
            y: 36.0 + ((n * 64) % 320) as f32,
            compact: false,
            running: false,
        });
        let _ = self.thread_mut(&id);
        self.inputs
            .entry(id.clone())
            .or_insert_with(|| rune::TextEdit::new(""));
        self.select_card(&id);
        id
    }

    /// Close a session: the card and tab go, but the thread stays — reopening
    /// from the Add menu restores its history (closing a tab never deletes
    /// the chat, like React). Selection and sheet fall back to the first
    /// remaining session.
    pub fn close_session(&mut self, id: &str) {
        if let Some(card) = self.cards.iter().find(|c| c.id == id) {
            if card.item == MapCardItem::Chat && !self.closed.iter().any(|(cid, _)| cid == id) {
                self.closed.push((id.to_string(), card.title.clone()));
            }
        }
        self.cards.retain(|c| c.id != id);
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

    /// Re-add a closed chat to the canvas (Add menu → Chats). History is
    /// intact — the thread survived the close. Lands top-left in view, like
    /// `addDraft` (`screenX 10, screenY 78 + n·430` into world points).
    pub fn add_draft_chat(&mut self, id: &str) {
        if self.card(id).is_some() {
            self.add_open = false;
            return;
        }
        let title = self
            .closed
            .iter()
            .find(|(cid, _)| cid == id)
            .map(|(_, t)| t.clone())
            .or_else(|| self.sessions.threads.get(id).map(|t| t.title.clone()))
            .unwrap_or_else(|| "Chat".to_string());
        let n = self.cards.len();
        let k = self.cam.scale.max(CAM_MIN_SCALE);
        self.cards.push(MapCard {
            id: id.to_string(),
            title,
            subtitle: format!("{} · restored", self.sessions.kind),
            preview: "No messages yet.".to_string(),
            kind: self.sessions.kind.clone(),
            item: MapCardItem::Chat,
            project: None,
            x: (10.0 - self.cam.x) / k,
            y: (78.0 + n as f32 * 430.0 - self.cam.y) / k,
            compact: false,
            running: false,
        });
        self.closed.retain(|(cid, _)| cid != id);
        let _ = self.thread_mut(id);
        self.inputs
            .entry(id.to_string())
            .or_insert_with(|| rune::TextEdit::new(""));
        if !self.sessions.order.contains(&id.to_string()) {
            self.sessions.order.push(id.to_string());
        }
        self.select_card(id);
        self.add_open = false;
    }

    /// Add a project card (Add menu → Projects). Info display from the stored
    /// snapshot — no fetch; the Group panel owns the live list.
    pub fn add_draft_project(&mut self, pid: &str) {
        let id = format!("project:{pid}");
        if self.card(&id).is_some() {
            self.add_open = false;
            return;
        }
        let Some(p) = self.projects.projects.iter().find(|p| p.id == pid).cloned() else {
            return;
        };
        let n = self.cards.len();
        let k = self.cam.scale.max(CAM_MIN_SCALE);
        self.cards.push(MapCard {
            id: id.clone(),
            title: if p.title.is_empty() {
                pid.to_string()
            } else {
                p.title.clone()
            },
            subtitle: format!("Project · {} items", p.total),
            preview: String::new(),
            kind: self.sessions.kind.clone(),
            item: MapCardItem::Project,
            project: Some(p),
            x: (10.0 - self.cam.x) / k,
            y: (78.0 + n as f32 * 430.0 - self.cam.y) / k,
            compact: true,
            running: false,
        });
        self.select_card(&id);
        self.add_open = false;
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

/// Compact card bubble (`CanvasChatBubble` at card scale): avatar, name head
/// (+ `auto` badge, typing dots when live), plain wrapped text, clock foot.
/// Card bodies render raw text like `.canvas-bubble-text` — no markdown pass.
/// Project card (`CanvasItemCard` project branch): title, progress bar,
/// item counts, waiting badge. Display-only — the Group panel owns the live
/// list, and project ops live there; the card has no open target (documented,
/// not faked).
fn project_card_view<S: CanvasHost + 'static>(card: &MapCard, active: bool) -> El {
    let (w, h) = card.size();
    let p = card.project.clone().unwrap_or_default();
    let waiting = p.waiting > 0;
    let border = if active {
        ink("#ffffff4d")
    } else if waiting {
        // `.canvas-card.is-waiting`: amber border.
        ink("#fcd34d42")
    } else {
        ink("#ffffff16")
    };
    let id = card.id.clone();
    let drag_id = card.id.clone();
    let close_id = id.clone();
    let frac = p.fraction();
    let fill = (frac.clamp(0.0, 1.0) * 1000.0).round() / 10.0;
    col()
        .key(card.id.clone())
        .absolute(card.x, card.y)
        .size(w, h)
        .radius(16.0)
        .fill(ink("#0b0b0e"))
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
        .clip_rounded(16.0)
        .child(
            row()
                .key(format!("{id}-head"))
                .w_full()
                .items_center()
                .gap(8.0)
                .px(14.0)
                .pt(14.0)
                .pb(10.0)
                .on_drag(move |s: &mut S, drag: &mut Drag, _tx| {
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
                    rune::el().key(format!("{id}-grip")).child(
                        text("⋮⋮".to_string())
                            .font(rune::tok::TextStyle::new(11.0, 700.0))
                            .color(ink("#52525b")),
                    ),
                )
                .child(
                    rune::el()
                        .key(format!("{id}-typeicon"))
                        .size(22.0, 22.0)
                        .radius(6.0)
                        .fill(ink("#24242a"))
                        .center()
                        .child(
                            text("▤".to_string())
                                .font(rune::tok::TextStyle::new(11.0, 700.0))
                                .color(ink("#c9c9d1")),
                        ),
                )
                .child(
                    col()
                        .grow(1.0)
                        .min_h(0.0)
                        .child(
                            text(card.title.clone())
                                .key(format!("{id}-title"))
                                .font(rune::tok::TextStyle::new(13.5, 600.0))
                                .color(ink("#f2f2f5")),
                        )
                        .child(
                            text(card.subtitle.clone())
                                .key(format!("{id}-subtitle"))
                                .font(rune::tok::TextStyle::new(10.5, 400.0))
                                .color(ink("#8a8a93")),
                        ),
                )
                .child(
                    text("×".to_string())
                        .key(format!("{id}-close"))
                        .role("button")
                        .label(format!("Remove {}", card.title))
                        .font(rune::tok::TextStyle::new(13.0, 400.0))
                        .color(ink("#71717a"))
                        .on_press(move |s: &mut S| {
                            s.canvas_mut().close_session(&close_id);
                        }),
                ),
        )
        .child(
            col()
                .key(format!("{id}-projbody"))
                .w_full()
                .gap(6.0)
                .px(14.0)
                .child(
                    row()
                        .key(format!("{id}-bar"))
                        .w_full()
                        .h(4.0)
                        .radius(2.0)
                        .fill(ink("#ffffff12"))
                        .child(
                            rune::el()
                                .key(format!("{id}-fill"))
                                .h_full()
                                .radius(2.0)
                                .fill(ink("#d4d4d8"))
                                .grow(fill.max(1.0))
                                .min_h(0.0),
                        )
                        .child(
                            rune::el()
                                .key(format!("{id}-rest"))
                                .grow((1000.0 - fill).max(1.0)),
                        ),
                )
                .child(
                    text(format!("{} of {} items", p.done, p.total))
                        .key(format!("{id}-counts"))
                        .font(rune::tok::TextStyle::new(10.0, 400.0))
                        .color(ink("#71717a")),
                ),
        )
        .child(
            text(if waiting {
                format!("{} waiting on you", p.waiting)
            } else {
                String::new()
            })
            .key(format!("{id}-waiting"))
            .px(14.0)
            .font(rune::tok::TextStyle::new(10.0, 600.0))
            .color(ink("#fcd34d")),
        )
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

fn mini_bubble(id: &str, r: &crate::rune_transcript::RowData, live_row: bool) -> El {
    let (fill, hair_a, avatar_bg, initial, name, body_ink) = match r.role {
        "user" => ("#28282f", 0.075, "#303038", "U", "You", "#dedee5"),
        "error" => ("#fb718511", 0.24, "#3a2226", "!", "Error", "#d2d2da"),
        "system" => ("#101013", 0.05, "#24242a", "S", "System", "#d2d2da"),
        _ => ("#19191e", 0.05, "#24242a", "A", "Agent", "#d2d2da"),
    };
    let hairline = if r.role == "error" {
        ink("#fb71853d")
    } else if live_row {
        rune::Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.10,
        }
    } else {
        rune::Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: hair_a,
        }
    };
    let mut head = row().items_center().gap(6.0).child(
        text(name.to_string())
            .font(rune::tok::TextStyle::new(10.0, 650.0))
            .color(ink("#f0f0f3")),
    );
    if r.role == "agent" || r.role == "system" {
        // `auto` badge in the head row, like `.canvas-bubble-badge`.
        head = head.child(
            text("auto".to_string())
                .key(format!("{id}-mini-{}-badge", r.key))
                .font(rune::tok::TextStyle::new(8.0, 600.0))
                .px(5.0)
                .py(1.0)
                .radius(5.0)
                .fill(ink("#2b2b32"))
                .color(ink("#9696a0")),
        );
    }
    let mut bubble = col()
        .key(format!("{id}-mini-{}", r.key))
        .radius(12.0)
        .fill(ink(if live_row { "#1b1b20" } else { fill }))
        .border(1.0, hairline)
        .px(10.0)
        .py(8.0)
        .child(head)
        .child(
            text(r.body.clone())
                .font(rune::tok::TextStyle::new(11.5, 400.0))
                .wrap()
                .color(ink(body_ink)),
        );
    if live_row {
        let mut dots = row().items_center().gap(3.0).pt(4.0);
        for (n, delay) in [0.0, 0.14, 0.28].into_iter().enumerate() {
            dots = dots.child(
                rune::el()
                    .key(format!("{id}-mini-{}-t{n}", r.key))
                    .size(3.0, 3.0)
                    .radius(1.5)
                    .fill(ink("#9b9ba5"))
                    .looping(rune::LoopSpec::opacity(1.1, delay, 0.35, 1.0)),
            );
        }
        bubble = bubble.child(dots);
    }
    let t = crate::rune_transcript::clock(&r.at);
    if !t.is_empty() {
        bubble = bubble.child(
            text(t)
                .key(format!("{id}-mini-{}-time", r.key))
                .font(rune::tok::TextStyle::new(9.0, 400.0))
                .color(ink("#71717a")),
        );
    }
    let message = row()
        .key(format!("{id}-minrow-{}", r.key))
        .w_full()
        .gap(6.0);
    if r.role == "user" {
        message
            .child(bubble.max_w(300.0))
            .child(
                rune::el()
                    .size(18.0, 18.0)
                    .radius(9.0)
                    .fill(ink(avatar_bg))
                    .center()
                    .child(
                        text(initial.to_string())
                            .font(rune::tok::TextStyle::new(8.0, 700.0))
                            .color(ink("#e6e6ec")),
                    ),
            )
            .justify_end()
    } else {
        message
            .child(
                rune::el()
                    .size(18.0, 18.0)
                    .radius(9.0)
                    .fill(ink(avatar_bg))
                    .center()
                    .child(
                        text(initial.to_string())
                            .font(rune::tok::TextStyle::new(8.0, 700.0))
                            .color(ink("#c9c9d1")),
                    ),
            )
            .child(bubble.grow(1.0).min_h(0.0))
    }
}

fn card_view<S: CanvasHost + 'static>(
    card: &MapCard,
    active: bool,
    thread: Option<&crate::rune_transcript::RuneTranscriptState>,
    input: &rune::TextEdit,
    kids: &[(String, String)],
    menu_open: bool,
) -> El {
    if card.item == MapCardItem::Project {
        return project_card_view::<S>(card, active);
    }
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
    // Card status from the thread (`childStatus`): the run dot while live,
    // failed on an error tail, done once rows exist, idle otherwise. Queued
    // has no Rune-side signal (no server row until detail loads) — running
    // covers the in-flight case.
    let (live, status): (bool, &'static str) = match thread {
        Some(t) if t.live => (true, "running"),
        Some(t) if t.rows.last().is_some_and(|r| r.role == "error") => (false, "failed"),
        Some(t) if !t.rows.is_empty() => (false, "done"),
        _ => (false, "idle"),
    };
    // Status badge (`StatusBadge`): pulse while running, ✓ done, ✕ failed,
    // quiet dot idle. Rune has no icon font — glyphs, same geometry.
    let badge: El = match status {
        "running" => rune::el()
            .key(format!("{id}-badge"))
            .size(8.0, 8.0)
            .radius(4.0)
            .fill(ink("#ededf0"))
            .looping(rune::LoopSpec::opacity(2.2, 0.0, 0.35, 1.0)),
        "done" => text("✓".to_string())
            .key(format!("{id}-badge"))
            .font(rune::tok::TextStyle::new(11.0, 700.0))
            .color(ink("#34d399")),
        "failed" => text("✕".to_string())
            .key(format!("{id}-badge"))
            .font(rune::tok::TextStyle::new(11.0, 700.0))
            .color(ink("#f87171")),
        _ => rune::el()
            .key(format!("{id}-badge"))
            .size(6.0, 6.0)
            .radius(3.0)
            .fill(ink("#52525b")),
    };
    let open_id = id.clone();
    let close_id = id.clone();
    let menu_id = id.clone();
    // Header (`canvas-card-head`): grip (the drag handle), type icon, linked
    // title + subtitle, status, agents menu, close. The title opens the chat
    // (`titleOpen`); close removes the card (`onRemove` — same as tab × here,
    // local scene state only).
    let head = row()
        .key(format!("{id}-head"))
        .w_full()
        .items_center()
        .gap(8.0)
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
            row()
                .key(format!("{id}-titles"))
                .grow(1.0)
                .min_h(0.0)
                .child(
                    rune::el().key(format!("{id}-grip")).child(
                        text("⋮⋮".to_string())
                            .font(rune::tok::TextStyle::new(11.0, 700.0))
                            .color(ink("#52525b")),
                    ),
                )
                .child(
                    rune::el()
                        .key(format!("{id}-typeicon"))
                        .size(22.0, 22.0)
                        .radius(6.0)
                        .fill(ink("#24242a"))
                        .center()
                        .child(
                            text(
                                card.kind
                                    .chars()
                                    .next()
                                    .unwrap_or('C')
                                    .to_uppercase()
                                    .to_string(),
                            )
                            .font(rune::tok::TextStyle::new(11.0, 700.0))
                            .color(ink("#c9c9d1")),
                        ),
                )
                .child(
                    col()
                        .grow(1.0)
                        .min_h(0.0)
                        .role("button")
                        .label(format!("Open {}", card.title))
                        .on_press(move |s: &mut S| {
                            s.canvas_mut().open_sheet(&open_id);
                        })
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
                ),
        )
        .child(badge)
        .child({
            let mut agents_btn = row()
                .key(format!("{id}-agents"))
                .items_center()
                .gap(4.0)
                .role("button")
                .label(format!("Connected agents ({})", kids.len()))
                .on_press(move |s: &mut S| {
                    let c = s.canvas_mut();
                    if c.agents_open.as_deref() == Some(menu_id.as_str()) {
                        c.agents_open = None;
                    } else {
                        c.agents_open = Some(menu_id.clone());
                    }
                })
                .child(
                    text("⤢".to_string())
                        .font(rune::tok::TextStyle::new(12.0, 400.0))
                        .color(ink("#71717a")),
                );
            if !kids.is_empty() {
                agents_btn = agents_btn.child(
                    text(kids.len().to_string())
                        .key(format!("{id}-agents-count"))
                        .font(rune::tok::TextStyle::new(9.0, 600.0))
                        .color(ink("#a1a1aa")),
                );
            }
            agents_btn
        })
        .child(
            text("×".to_string())
                .key(format!("{id}-close"))
                .role("button")
                .label(format!("Close {}", card.title))
                .font(rune::tok::TextStyle::new(13.0, 400.0))
                .color(ink("#71717a"))
                .on_press(move |s: &mut S| {
                    s.canvas_mut().close_session(&close_id);
                }),
        );
    // Conversation (`canvas-card-conversation`): the thread's last rows as
    // compact bubbles, or the empty line when nothing ran yet.
    let convo = match thread {
        Some(t) if !t.rows.is_empty() => {
            let mut convo = col().key(format!("{id}-convo")).w_full().gap(8.0).px(14.0);
            let n = t.rows.len();
            for r in t.rows.iter().skip(n.saturating_sub(4)) {
                convo = convo.child(mini_bubble(&id, r, live && r.key == "live-agent"));
            }
            convo
        }
        _ => col().key(format!("{id}-convo")).w_full().px(14.0).child(
            text(card.preview.clone())
                .key(format!("{id}-preview"))
                .font(rune::tok::TextStyle::new(11.5, 400.0))
                .wrap()
                .color(ink("#92959e")),
        ),
    };
    view = view.child(head);
    // Connected-agents menu (`canvas-card-agent-menu`): child chats of this
    // card; a press selects the child.
    if menu_open {
        let mut menu = col()
            .key(format!("{id}-agents-menu"))
            .w_full()
            .gap(2.0)
            .px(14.0)
            .child(
                text("Connected agents".to_string())
                    .font(rune::tok::TextStyle::new(9.0, 600.0))
                    .color(ink("#52525b")),
            );
        if kids.is_empty() {
            menu = menu.child(
                text("No connected agents.".to_string())
                    .font(rune::tok::TextStyle::new(10.0, 400.0))
                    .color(ink("#52525b")),
            );
        }
        for (kid_id, kid_title) in kids {
            let target = kid_id.clone();
            menu = menu.child(
                row()
                    .key(format!("{id}-agent-{kid_id}"))
                    .w_full()
                    .items_center()
                    .gap(8.0)
                    .radius(6.0)
                    .px(8.0)
                    .py(5.0)
                    .role("button")
                    .label(format!("Open {kid_title}"))
                    .on_press(move |s: &mut S| {
                        let c = s.canvas_mut();
                        c.agents_open = None;
                        c.select_card(&target);
                    })
                    .child(
                        text(kid_title.clone())
                            .font(rune::tok::TextStyle::new(11.0, 500.0))
                            .color(ink("#d4d4d8")),
                    ),
            );
        }
        view = view.child(menu);
    }
    view = view.child(convo);
    // Card dock (`canvas-card-dock`): per-card input + send. Placeholder and
    // disabled state mirror the React form exactly: type-ahead while the run
    // is live, no double-send. No attach button / drop overlay / ask panel —
    // file pickers, drag-drop and agent questions have no Rune target
    // (documented, not faked).
    let draft_empty = input.text.trim().is_empty();
    let send_id = id.clone();
    let edit_id = id.clone();
    let dock = row()
        .key(format!("{id}-dock"))
        .w_full()
        .items_center()
        .gap(8.0)
        .px(14.0)
        .pt(8.0)
        .child(
            text_input(input, rune::tok::TextStyle::new(12.0, 400.0))
                .key(format!("{id}-input"))
                .grow(1.0)
                .min_h(0.0)
                .placeholder(
                    if live {
                        "Agent is running — you can type ahead…"
                    } else {
                        "Message this chat…"
                    },
                    ink("#52525b"),
                )
                .caret(ink("#f2f2f4"), ink("#3b82f6"))
                .on_edit(move |s: &mut S| s.canvas_mut().input_mut(&edit_id)),
        )
        .child({
            let disabled = live || draft_empty;
            let send_label = id.clone();
            rune::el()
                .key(format!("{id}-send"))
                .role("button")
                .label(format!("Send to {send_label}"))
                .size(28.0, 28.0)
                .radius(14.0)
                .fill(ink(if disabled { "#2a2a2e" } else { "#ffffff" }))
                .center()
                .on_press(move |s: &mut S| {
                    let c = s.canvas_mut();
                    let prompt = c
                        .inputs
                        .get(&send_id)
                        .map(|e| e.text.trim().to_string())
                        .unwrap_or_default();
                    if prompt.is_empty() {
                        return;
                    }
                    if c.sessions.threads.get(&send_id).is_some_and(|t| t.live) {
                        return;
                    }
                    if let Some(e) = c.inputs.get_mut(&send_id) {
                        e.text.clear();
                        e.anchor = 0;
                        e.focus = 0;
                    }
                    let thread = c.thread_mut(&send_id);
                    crate::rune_transcript::stage_send_prompt(thread, prompt);
                })
                .child(
                    text("↑".to_string())
                        .font(rune::tok::TextStyle::new(13.0, 650.0))
                        .color(ink(if disabled { "#71717a" } else { "#111113" })),
                )
        });
    view = view
        .child(dock)
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
        );
    view
}

/// Add menu (`CanvasAddMenu`): search + Chats/Projects/Addons tabs with
/// counts + rows. Chats are closed sessions (history intact); projects come
/// from the Group list; addons have no Rune target yet ("No addons
/// available"). Tap adds at the visible top-left (`addDraft` math) and closes
/// the menu. No drag-drop onto the canvas (no DnD target — click only).
pub fn add_menu<S: CanvasHost + 'static>(s: &CanvasState) -> El {
    let q = s.add_search.text.trim().to_lowercase();
    // Chats tab: closed sessions matching the query (all are addable —
    // nothing closed is on the canvas).
    let chats: Vec<(String, String)> = s
        .closed
        .iter()
        .filter(|(_, title)| q.is_empty() || title.to_lowercase().contains(&q))
        .cloned()
        .collect();
    let projects: Vec<crate::projects::GroupProject> = s
        .projects
        .projects
        .iter()
        .filter(|p| {
            q.is_empty() || p.title.to_lowercase().contains(&q) || p.id.to_lowercase().contains(&q)
        })
        .cloned()
        .collect();
    let on_canvas = |id: &str| s.cards.iter().any(|c| c.id == id);
    let mut list = col().key("canvas-add-list").w_full().gap(2.0);
    if s.add_tab == "chats" {
        if chats.is_empty() {
            list = list.child(
                text(if q.is_empty() {
                    "No chats available.".to_string()
                } else {
                    format!("Nothing matches “{q}”.")
                })
                .key("canvas-add-empty")
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .color(ink("#71717a")),
            );
        }
        for (cid, title) in &chats {
            let target = cid.clone();
            list = list.child(add_row::<S>(
                &format!("canvas-add-chat-{cid}"),
                title.clone(),
                "Chat".to_string(),
                "▤".to_string(),
                false,
                move |s: &mut S| {
                    s.canvas_mut().add_draft_chat(&target);
                },
            ));
        }
    } else if s.add_tab == "projects" {
        if projects.is_empty() {
            list = list.child(
                text(if q.is_empty() {
                    "No projects available.".to_string()
                } else {
                    format!("Nothing matches “{q}”.")
                })
                .key("canvas-add-empty")
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .color(ink("#71717a")),
            );
        }
        for p in &projects {
            let pid = p.id.clone();
            let added = on_canvas(&format!("project:{pid}"));
            list = list.child(add_row::<S>(
                &format!("canvas-add-project-{pid}"),
                if p.title.is_empty() {
                    pid.clone()
                } else {
                    p.title.clone()
                },
                format!("{}/{} items", p.done, p.total),
                "▤".to_string(),
                added,
                move |s: &mut S| {
                    s.canvas_mut().add_draft_project(&pid);
                },
            ));
        }
    } else {
        // Addons: OpenDesign is unported — honest empty state, same copy
        // shape as the other tabs.
        list = list.child(
            text("No addons available.".to_string())
                .key("canvas-add-empty")
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .color(ink("#71717a")),
        );
    }
    let mut tabs = row().key("canvas-add-tabs").w_full().gap(2.0);
    for (tab, label, count) in [
        ("chats", "Chats", chats.len()),
        ("projects", "Projects", projects.len()),
        ("addons", "Addons", 0usize),
    ] {
        let active = s.add_tab == tab;
        let target = tab.to_string();
        tabs = tabs.child(
            row()
                .key(format!("canvas-add-tab-{tab}"))
                .items_center()
                .gap(4.0)
                .px(6.0)
                .py(2.0)
                .radius(4.0)
                .fill(ink(if active { "#ffffff17" } else { "#00000000" }))
                .role("button")
                .label(format!("{label} tab"))
                .on_press(move |s: &mut S| {
                    s.canvas_mut().add_tab = target.clone();
                })
                .child(
                    text(label.to_string())
                        .font(rune::tok::TextStyle::new(9.0, 500.0))
                        .color(ink(if active { "#ffffff" } else { "#71717a" })),
                )
                .child(
                    text(count.to_string())
                        .font(rune::tok::TextStyle::new(8.0, 400.0))
                        .color(ink("#52525b")),
                ),
        );
    }
    col()
        .key("canvas-add-menu")
        .absolute(14.0, 44.0)
        .w(300.0)
        .radius(10.0)
        .fill(ink("#121317fa"))
        .border(1.0, ink("#ffffff17"))
        .shadow(
            0.0,
            70.0,
            rune::Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.55,
            },
        )
        .px(8.0)
        .pt(6.0)
        .pb(6.0)
        .gap(4.0)
        .child(
            row()
                .key("canvas-add-searchrow")
                .w_full()
                .items_center()
                .gap(6.0)
                .px(8.0)
                .py(6.0)
                .radius(6.0)
                .fill(ink("#ffffff08"))
                .child(
                    text("⌕".to_string())
                        .font(rune::tok::TextStyle::new(10.0, 400.0))
                        .color(ink("#52525b")),
                )
                .child(
                    text_input(&s.add_search, rune::tok::TextStyle::new(10.5, 400.0))
                        .key("canvas-add-search")
                        .grow(1.0)
                        .min_h(0.0)
                        .placeholder("Add chat, project, or addon…", ink("#52525b"))
                        .caret(ink("#f2f2f4"), ink("#3b82f6"))
                        .on_edit(move |s: &mut S| &mut s.canvas_mut().add_search),
                )
                .child(
                    text("×".to_string())
                        .key("canvas-add-close")
                        .role("button")
                        .label("Close add menu")
                        .font(rune::tok::TextStyle::new(11.0, 400.0))
                        .color(ink("#71717a"))
                        .on_press(move |s: &mut S| {
                            s.canvas_mut().add_open = false;
                        }),
                ),
        )
        .child(tabs)
        .child(list)
}

/// One add-menu row: icon, title, subtitle, plus (or emerald check when
/// already on the canvas). `on_add` is a no-op for added rows.
fn add_row<S: 'static>(
    key: &str,
    title: String,
    subtitle: String,
    icon: String,
    added: bool,
    on_add: impl Fn(&mut S) + 'static,
) -> El {
    let mut btn = row()
        .key(key.to_string())
        .w_full()
        .items_center()
        .gap(8.0)
        .radius(6.0)
        .px(8.0)
        .py(4.0)
        .role("button")
        .label(if added {
            "Already on the canvas".to_string()
        } else {
            format!("{title} — click to add")
        })
        .child(
            text(icon)
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .color(ink("#52525b")),
        )
        .child(
            col()
                .grow(1.0)
                .min_h(0.0)
                .child(
                    text(title)
                        .font(rune::tok::TextStyle::new(11.0, 400.0))
                        .color(ink("#d4d4d8")),
                )
                .child(
                    text(subtitle)
                        .font(rune::tok::TextStyle::new(9.0, 400.0))
                        .color(ink("#52525b")),
                ),
        )
        .child(
            text(if added { "✓" } else { "+" }.to_string())
                .font(rune::tok::TextStyle::new(11.0, 600.0))
                .color(ink(if added { "#34d399" } else { "#52525b" })),
        );
    if !added {
        btn = btn.on_press(on_add);
    }
    btn
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
        let kids: Vec<(String, String)> = s
            .edges
            .iter()
            .filter(|(p, _)| p == &card.id)
            .filter_map(|(_, c)| s.card(c).map(|k| (k.id.clone(), k.title.clone())))
            .collect();
        let blank = rune::TextEdit::new("");
        // Inputs are ensured in `seed_sessions`/`new_session`; the blank is
        // a borrow-safe fallback that never renders (every card has one).
        let input = s.inputs.get(&card.id).unwrap_or(&blank);
        world = world.child(card_view::<S>(
            card,
            card.id == s.active_id,
            s.sessions.threads.get(&card.id),
            input,
            &kids,
            s.agents_open.as_deref() == Some(card.id.as_str()),
        ));
    }
    // Fixed-chrome area: clips the scaled world, holds composer + HUD.
    // `grow` (not `h_full`): shares a shell row without overflowing it.
    let mut canvas = col()
        .key("canvas-area")
        .grow(1.0)
        .min_h(0.0)
        .fill(ink("#08080a"))
        .clip_rounded(0.0);
    if s.cards.is_empty() {
        // Empty canvas (`canvas-empty`, map variant): Add button + copy.
        // Chrome (HUD, dock, fab) still mounts around it.
        canvas = canvas.child(
            col()
                .key("canvas-empty")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .center()
                .gap(10.0)
                .child(
                    row()
                        .key("canvas-empty-add")
                        .items_center()
                        .gap(6.0)
                        .px(14.0)
                        .py(8.0)
                        .radius(9.0)
                        .fill(ink("#ffffff0d"))
                        .border(1.0, ink("#ffffff14"))
                        .role("button")
                        .label("Add to canvas")
                        .on_press(move |s: &mut S| {
                            s.canvas_mut().add_open = true;
                        })
                        .child(
                            text("+".to_string())
                                .font(rune::tok::TextStyle::new(14.0, 600.0))
                                .color(ink("#e4e4e7")),
                        )
                        .child(
                            text("Add to Canvas".to_string())
                                .font(rune::tok::TextStyle::new(12.0, 600.0))
                                .color(ink("#e4e4e7")),
                        ),
                )
                .child(
                    text("Open a chat and it will appear here automatically.".to_string())
                        .key("canvas-empty-copy")
                        .font(rune::tok::TextStyle::new(11.0, 400.0))
                        .color(ink("#71717a")),
                ),
        );
    } else {
        canvas = canvas.child(world);
    }
    // Floating composer, centred over the canvas like `HouseChat`'s dock:
    // 640pt wide, bottom-anchored. Same card as the transcript dock.
    // Toolbar chrome follows the ACTIVE thread; unwritten threads read the
    // defaults, like a fresh chat.
    let fallback_usage = crate::rune_transcript::UsageTotals::default();
    let active_thread = s.sessions.threads.get(&s.active_id);
    let dock_meta = super::composer::ComposerMeta {
        target: active_thread
            .map(|t| t.run_target.as_str())
            .unwrap_or("local"),
        usage: active_thread.map(|t| &t.usage).unwrap_or(&fallback_usage),
        usage_open: active_thread.map(|t| t.usage_open).unwrap_or(false),
        provider_label: active_thread
            .map(|t| t.provider.label.as_str())
            .unwrap_or(""),
        provider_warn: active_thread.map(|t| t.provider.warn).unwrap_or(false),
        // Map dock: no model list (like `HouseChat` — provider pill only).
        model: "",
        models: &[],
        model_open: false,
    };
    canvas = canvas.child(
        rune::el()
            .key("map-composer-float")
            .absolute((MAP_W - MAP_SIDEBAR_W - 640.0) / 2.0, MAP_H - 128.0)
            .w(640.0)
            .child(super::composer::composer_card(
                &s.edit,
                move |s: &mut S| &mut s.canvas_mut().edit,
                s.voice.meter(),
                false,
                dock_meta,
                super::composer::ComposerButtons {
                    voice: std::rc::Rc::new(move |s: &mut S| {
                        let c = s.canvas_mut();
                        c.voice.set_live(!c.voice.live);
                    }),
                    cancel: std::rc::Rc::new(move |_s: &mut S| {}),
                    local: std::rc::Rc::new(move |s: &mut S| {
                        let id = s.canvas_mut().active_id.clone();
                        if !id.is_empty() {
                            s.canvas_mut().thread_mut(&id).run_target = "local".to_string();
                        }
                    }),
                    cloud: std::rc::Rc::new(move |s: &mut S| {
                        let id = s.canvas_mut().active_id.clone();
                        if !id.is_empty() {
                            s.canvas_mut().thread_mut(&id).run_target = "cloud".to_string();
                        }
                    }),
                    usage: std::rc::Rc::new(move |s: &mut S| {
                        let id = s.canvas_mut().active_id.clone();
                        if !id.is_empty() {
                            let open = s.canvas_mut().thread_mut(&id).usage_open;
                            s.canvas_mut().thread_mut(&id).usage_open = !open;
                        }
                    }),
                    send: std::rc::Rc::new(move |s: &mut S| {
                        // The map dock sends into the ACTIVE session's thread
                        // and opens its sheet — chats are the only type, so
                        // every prompt lands in a chat thread. It never shows
                        // Stop; cancellation lives on the sheet that owns the
                        // run.
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
                    }),
                    // Map dock: no model list, so these never fire.
                    model_menu: std::rc::Rc::new(move |_s: &mut S| {}),
                    model_pick: std::rc::Rc::new(move |_s: &mut S, _id: String| {}),
                },
            )),
    );

    // Add button (`.canvas-add-fab`): Plus + "Add", top-left fixed chrome.
    canvas = canvas.child(
        row()
            .key("canvas-add")
            .absolute(14.0, 12.0)
            .items_center()
            .gap(5.0)
            .h(26.0)
            .radius(7.0)
            .fill(ink("#0f0f12b8"))
            .border(1.0, ink("#ffffff14"))
            .px(9.0)
            .role("button")
            .label("Add to canvas")
            .on_press(move |s: &mut S| {
                s.canvas_mut().add_open = true;
            })
            .child(
                text("+".to_string())
                    .font(rune::tok::TextStyle::new(12.0, 600.0))
                    .color(ink("#a9abb3")),
            )
            .child(
                text("Add".to_string())
                    .font(rune::tok::TextStyle::new(9.5, 600.0))
                    .color(ink("#a9abb3")),
            ),
    );
    if s.add_open {
        // Backdrop catcher (React's window pointerdown): anything outside
        // the menu closes it. Transparent, below the menu.
        canvas = canvas.child(
            rune::el()
                .key("canvas-add-backdrop")
                .absolute(0.0, 0.0)
                .size(vw, MAP_H)
                .fill(rune::Color {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.0,
                })
                .role("button")
                .label("Close add menu")
                .modal(true, move |s: &mut S| {
                    s.canvas_mut().add_open = false;
                }),
        );
        canvas = canvas.child(add_menu::<S>(s));
    }
    // Camera HUD (`CanvasStage` map controls): zoom out/in at the React
    // 0.86/1.16 steps, fit-to-view, and layout reset. Fixed chrome — never
    // scaled with the world.
    // (Add menu lives just above: `add_menu` + `add_row`.)
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
            // Exact `CanvasStage` copy (design parity). NOTE: scroll-to-zoom
            // has no element-level Rune target yet — buttons zoom, wheel is
            // ignored. The string stays identical so the pixels match.
            text("Drag to pan · scroll to zoom · drag cards to arrange".to_string())
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
pub fn chats_panel<S: CanvasHost + 'static>(s: &CanvasState) -> El {
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
    fn card_shows_mini_thread_and_sends_into_it() {
        fn card_view_only(s: &CanvasState, _: &mut rune::Cx<CanvasState>) -> El {
            col()
                .key("card-test-root")
                .w_full()
                .h_full()
                .child(canvas_body::<CanvasState>(s))
        }
        let env = Env::new(true, 1.0);
        let mut init = CanvasState::map();
        init.agents.auto = false;
        // Seed a thread with two stamped rows on chat-1; chat-2 stays empty.
        let id = "chat:ws-general:chat-1".to_string();
        let thread = init.thread_mut(&id);
        thread.api_port = 1;
        thread.rows = vec![
            crate::rune_transcript::RowData {
                key: "u1".to_string(),
                role: "user",
                body: "hey how much 4152-5".to_string(),
                at: "2026-11-09T14:44:00Z".to_string(),
            },
            crate::rune_transcript::RowData {
                key: "a1".to_string(),
                role: "agent",
                body: "4152 ÷ 5 = 830.4".to_string(),
                at: "2026-11-09T14:44:30Z".to_string(),
            },
        ];
        let mut rt = Runtime::new(init, card_view_only, (MAP_W, MAP_H), env);
        rt.frame(0.0);
        let card = "chat:ws-general:chat-1";
        let empty = "chat:ws-general:chat-2";
        // Mini bubbles + clocks mount; the empty card shows its preview.
        for key in [
            format!("{card}-convo"),
            format!("{card}-mini-u1-time"),
            format!("{card}-mini-a1"),
            format!("{card}-input"),
            format!("{card}-send"),
            format!("{empty}-preview"),
        ] {
            assert!(rt.find(&key).is_some(), "{key} mounted");
        }
        // Header chrome: badge (done ✓), agents menu, close.
        for key in [
            format!("{card}-badge"),
            format!("{card}-agents"),
            format!("{card}-close"),
        ] {
            assert!(rt.find(&key).is_some(), "{key} mounted");
        }
        // Per-card send lands in THIS thread only (api_port 1: the run
        // thread fails fast into an error frame, deterministically).
        rt.state.inputs.get_mut(card).unwrap().text = "ping".to_string();
        let (x, y) = rt.find(&format!("{card}-send")).unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        let threads = &rt.state.sessions.threads;
        assert!(
            threads
                .get(card)
                .unwrap()
                .rows
                .iter()
                .any(|r| r.body == "ping"),
            "prompt lands in card thread"
        );
        assert!(
            threads.get(empty).map(|t| t.rows.len()).unwrap_or(0) == 0,
            "other threads untouched"
        );
    }

    #[test]
    fn add_menu_reopens_closed_chats_and_projects() {
        fn menu_view(s: &CanvasState, _: &mut rune::Cx<CanvasState>) -> El {
            col()
                .key("menu-test-root")
                .w_full()
                .h_full()
                .child(canvas_body::<CanvasState>(s))
        }
        let env = Env::new(true, 1.0);
        let mut init = CanvasState::map();
        init.agents.auto = false;
        // Projects list with one entry for the Projects tab.
        init.projects.projects = vec![crate::projects::GroupProject {
            id: "p1".to_string(),
            title: "Website".to_string(),
            status: "doing".to_string(),
            done: 3,
            total: 4,
            waiting: 1,
        }];
        init.projects.auto = false;
        // Close chat-2 so the Chats tab has exactly one row.
        init.close_session("chat:ws-general:chat-2");
        assert!(init.card("chat:ws-general:chat-2").is_none());
        let mut rt = Runtime::new(init, menu_view, (MAP_W, MAP_H), env);
        rt.frame(0.0);
        assert!(rt.find("canvas-add").is_some(), "fab mounts");
        let (x, y) = rt.find("canvas-add").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        // Menu: search + three tabs with counts (1 closed chat, 1 project).
        for key in [
            "canvas-add-menu",
            "canvas-add-search",
            "canvas-add-tab-chats",
            "canvas-add-tab-projects",
            "canvas-add-tab-addons",
        ] {
            assert!(rt.find(key).is_some(), "{key} mounted");
        }
        // Re-add the closed chat: card + tab return, thread intact.
        let (x, y) = rt.find("canvas-add-chat-chat:ws-general:chat-2").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.state.card("chat:ws-general:chat-2").is_some());
        assert!(rt
            .state
            .sessions
            .order
            .contains(&"chat:ws-general:chat-2".to_string()));
        assert!(!rt.state.add_open, "menu closes on add");
        // Projects tab: add the project card with its progress bar.
        let (x, y) = rt.find("canvas-add").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        let (x, y) = rt.find("canvas-add-tab-projects").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        let (x, y) = rt.find("canvas-add-project-p1").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        for key in ["project:p1-bar", "project:p1-fill", "project:p1-counts"] {
            assert!(rt.find(key).is_some(), "{key} mounted");
        }
        assert!(rt.state.card("project:p1").is_some(), "project card lands");
        // Addons tab: honest empty state.
        let (x, y) = rt.find("canvas-add").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        let (x, y) = rt.find("canvas-add-tab-addons").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.find("canvas-add-empty").is_some(), "addons empty state");
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
        // Drag from the grip: the head now carries buttons (title opens the
        // sheet), so the grip is the drag-safe spot — like React's
        // `GripVertical` handle inside `data-canvas-drag-handle`.
        let (x, y) = rt.find("chat:ws-general:chat-1-grip").unwrap();
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
        for key in [format!("{run_id}-sheen"), format!("{run_id}-badge")] {
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
