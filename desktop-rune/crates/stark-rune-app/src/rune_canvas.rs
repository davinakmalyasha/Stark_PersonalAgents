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

use crate::keys::EnterPress;
use rune::{col, row, text, text_input, AppSpec, Cx, Drag, El, LineCap, Path, Phase, Shape};

pub const MAP_W: f32 = 1280.0;
pub const MAP_H: f32 = 900.0;
/// Side panel width (icon rail + panel, like the desktop layout).
pub const MAP_SIDEBAR_W: f32 = 300.0;
/// Icon rail width (the reference UI `HouseRail`: 36pt button + 6pt gutters + hairline).
pub const RAIL_W: f32 = 49.0;
pub const DOT_STEP: f32 = 26.0;
/// Chat kinds (the workspace dropdown). Chats are the only type — no
/// workspace scoping, everything lives in side tabs on one page.
pub const CHAT_KINDS: [&str; 4] = ["Main", "Coding", "House", "General"];

/// Visible top-level rows in the Main filter before Load more
/// (`HouseSidebar.GENERAL_PAGE`).
pub const GENERAL_PAGE: usize = 15;
/// Visible top-level rows per Coding project before Load more
/// (`HouseSidebar.PROJECT_PAGE`).
pub const PROJECT_PAGE: usize = 7;

/// Native mode label → the reference UI `ChatsFilter` (persisted in `/api/ui/state`).
pub fn filter_for_kind(kind: &str) -> &'static str {
    match kind {
        "Coding" => "coding",
        "House" => "house",
        "General" => "personal",
        _ => "general",
    }
}

/// The reference UI `ChatsFilter` → native mode label. Unknown/empty values keep the
/// current mode (the server may predate a filter name).
pub fn kind_for_filter(filter: &str) -> Option<&'static str> {
    match filter {
        "general" => Some("Main"),
        "coding" => Some("Coding"),
        "house" => Some("House"),
        "personal" => Some("General"),
        _ => None,
    }
}

/// Split `workspace:chat` into the pair the history fetch expects.
fn split_chat_key(key: &str) -> Option<(String, String)> {
    let (ws, chat) = key.split_once(':')?;
    if ws.is_empty() || chat.is_empty() {
        return None;
    }
    Some((ws.to_string(), chat.to_string()))
}

/// Side panel tab (the icon rail). One page: the panel switches, the canvas
/// stays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(dead_code)] // Agents/Memory/House remain panel states, not rail tabs
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
/// The reference UI's `[data-canvas-ui]`.
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
    #[cfg(test)]
    pub fn fit(&mut self, cards: &[MapCard]) {
        self.fit_in(cards, MAP_W - MAP_SIDEBAR_W, MAP_H);
    }

    /// `fitView` against a live stage size: the reference UI's paddings and clamps
    /// (`paddingX` 36 under 720pt, else 90; top 120, bottom 70; scale
    /// 0.45–1.15 for a fit).
    pub fn fit_in(&mut self, cards: &[MapCard], vw: f32, vh: f32) {
        if cards.is_empty() {
            *self = Self::default();
            return;
        }
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
        let pad_x = if vw < 720.0 { 36.0 } else { 90.0 };
        let pad_top = 120.0;
        let pad_bottom = 70.0;
        let avail_w = (vw - pad_x * 2.0).max(1.0);
        let avail_h = (vh - pad_top - pad_bottom).max(1.0);
        let scale = (avail_w / (x1 - x0).max(1.0))
            .min(avail_h / (y1 - y0).max(1.0))
            .clamp(CAM_MIN_SCALE, 1.15);
        let cx = (x0 + x1) / 2.0;
        let cy = (y0 + y1) / 2.0;
        self.scale = scale;
        self.x = vw / 2.0 - cx * scale;
        self.y = pad_top + avail_h / 2.0 - cy * scale;
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

/// Structural identity of an auto-synced map card (`CanvasItem.depth`,
/// `parentItemId`, `isTab` in `CanvasStage`). A session poll keeps the card's
/// current x/y while this is unchanged, so the map never jumps, and snaps it
/// back to the fresh tidy-tree slot when the tree shape changes.
#[derive(Debug, Clone, PartialEq)]
pub struct CardLayoutMeta {
    pub is_tab: bool,
    pub depth: usize,
    pub parent_item_id: Option<String>,
}

/// Server message → transcript row (`assistant` already normalized to
/// `agent` on parse; anything not user/error renders as agent).
pub(crate) fn row_from_chat(r: &crate::chats::ChatMsgRow) -> crate::rune_transcript::RowData {
    crate::rune_transcript::RowData {
        key: r.key.clone(),
        role: match r.role.as_str() {
            "user" => "user",
            "error" => "error",
            _ => "agent",
        },
        body: r.body.clone(),
        raw_body: r.raw_body.clone(),
        at: r.at.clone(),
        duration: r.duration.clone(),
        show_trace: r.show_trace,
        hide_body: r.hide_body,
        event: r.event.clone(),
        event_tokens_before: r.event_tokens_before,
        event_input: r.event_input,
        event_output: r.event_output,
        has_usage: r.has_usage,
        tokens_in: r.tokens_in,
        tokens_out: r.tokens_out,
        agent_name: r.agent_name.clone(),
        timeline: r.timeline.clone(),
        activity: r.activity.clone(),
        final_text: r.final_text.clone(),
        thinking: r.thinking.clone(),
        steps: r.steps.clone(),
        stopped: r.stopped,
        retry: r.retry.clone(),
        live_status: r.live_status.clone(),
        run_usage: r.run_usage.clone(),
        artifacts: r.artifacts.clone(),
        attachments: r.attachments.clone(),
        ..crate::rune_transcript::RowData::default()
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
    /// Pinned chats (`workspace:chat`), persisted via `/api/ui/state` and
    /// sorted first inside the active sidebar filter (the reference UI `pinnedChats`).
    pub pinned: Vec<String>,
    /// Inputs for inline rename: session id + draft. `None` renders rows.
    pub editing: Option<(String, rune::TextEdit)>,
    /// Fallback model for a rename input whose draft closed earlier in the
    /// frame: Rune syncs focus into the old node before the tree drops it,
    /// and the accessor must still return a field.
    pub rename_spare: rune::TextEdit,
    /// First click on Delete: session id + expiry (the reference UI 2.5s confirm).
    pub confirm_delete: Option<(String, u64)>,
    /// Confirmed delete in its 220ms fade-out: session id, start ms, remove-at
    /// ms (the reference UI `.is-deleting`).
    pub deleting: Option<(String, u64, u64)>,
    /// Collapsed spawn nests (`HouseSidebar.collapsedSpawns`): parents with
    /// spawned children start collapsed, so lower agents live inside their
    /// parent instead of reading as flat siblings.
    pub spawn_collapsed: std::collections::HashSet<String>,
    /// Nests the user explicitly expanded — auto-collapse seeding never
    /// re-collapses them (`HouseSidebar.userExpanded`).
    pub spawn_expanded: std::collections::HashSet<String>,
    /// Collapsed Coding project groups (`HouseSidebar.collapsedWs`).
    /// Groups start expanded; the toggle only ever adds here.
    pub collapsed_ws: std::collections::HashSet<String>,
    /// Visible top-level rows per Coding project (`projectLimits`),
    /// defaulting to `PROJECT_PAGE` (7). Grows by 7 per Load more press.
    pub project_limits: std::collections::HashMap<String, usize>,
    /// Visible top-level rows in the Main filter (`generalLimit`),
    /// defaulting to `GENERAL_PAGE` (15). Grows by 15 per Load more press.
    pub general_limit: usize,
}

impl std::fmt::Debug for Sessions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sessions")
            .field("threads", &self.threads.keys().collect::<Vec<_>>())
            .field("order", &self.order)
            .field("sheet", &self.sheet)
            .field("kind", &self.kind)
            .field("pinned", &self.pinned)
            .field("editing", &self.editing.as_ref().map(|(id, _)| id))
            .field("confirm_delete", &self.confirm_delete)
            .field("deleting", &self.deleting)
            .field("spawn_collapsed", &self.spawn_collapsed)
            .field("spawn_expanded", &self.spawn_expanded)
            .field("collapsed_ws", &self.collapsed_ws)
            .field("general_limit", &self.general_limit)
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
            pinned: self.pinned.clone(),
            editing: self.editing.clone(),
            rename_spare: self.rename_spare.clone(),
            confirm_delete: self.confirm_delete.clone(),
            deleting: self.deleting.clone(),
            spawn_collapsed: self.spawn_collapsed.clone(),
            spawn_expanded: self.spawn_expanded.clone(),
            collapsed_ws: self.collapsed_ws.clone(),
            project_limits: self.project_limits.clone(),
            general_limit: self.general_limit,
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
            kind: "Main".to_string(),
            pinned: Vec::new(),
            editing: None,
            rename_spare: rune::TextEdit::new(""),
            confirm_delete: None,
            deleting: None,
            spawn_collapsed: std::collections::HashSet::new(),
            spawn_expanded: std::collections::HashSet::new(),
            collapsed_ws: std::collections::HashSet::new(),
            project_limits: std::collections::HashMap::new(),
            general_limit: GENERAL_PAGE,
        }
    }
}

impl Sessions {
    /// Earliest wall-clock deadline that needs a repaint (delete confirm
    /// timeout or the fade-out removal).
    pub fn next_deadline(&self) -> Option<u64> {
        [
            self.confirm_delete.as_ref().map(|(_, at)| *at),
            self.deleting.as_ref().map(|(_, _, at)| *at),
        ]
        .into_iter()
        .flatten()
        .min()
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
    /// The sidebar's search + kind filter. Chats are the only type, so all
    /// threads live here — no per-route transcript states.
    pub sessions: Sessions,
    /// Camera (pan + zoom); chrome stays fixed while the world scales.
    pub cam: Camera,
    /// Presentation copy of the camera. Pan and zoom publish here and say
    /// `tx.live_only()`, so dragging the map never rebuilds the view (Rune
    /// binds offsets/scales at draw time; hit tests follow them).
    pub cam_live: rune::Live<[f32; 2]>,
    pub cam_scale_live: rune::Live<f32>,
    /// Dot grid offset: camera pan modulo the 26pt tile. The field is one
    /// tile larger than the viewport, so wrapping by a tile is invisible.
    pub grid_live: rune::Live<[f32; 2]>,
    /// Hover tracks for sidebar rows, project headers and canvas cards.
    pub hover: crate::motion::HoverSet,
    /// Camera follows the content (the reference UI `fitView`) until the first manual
    /// pan/zoom; the shell refits when the window resizes.
    pub cam_auto: bool,
    /// `fittedRef`: the map fits once when the first cards arrive, never
    /// again on a session poll (the reference UI preserves the user's pan/zoom).
    pub cam_fitted: bool,
    /// Auto-card structure from the last graph sync, mirroring the fields
    /// `CanvasStage` compares before preserving a card's position.
    pub layout_meta: std::collections::HashMap<String, CardLayoutMeta>,
    /// Canvas viewport size (the stage area, sidebar and tab bar excluded).
    /// The shell syncs this from the live window each poll; the standalone
    /// canvas spec keeps the 980×900 default.
    pub view_w: f32,
    pub view_h: f32,
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
    /// The reference UI's per-card `draft` state. The send button mirrors
    /// `disabled={sending || running || empty}` — type-ahead while running,
    /// no double-send.
    pub inputs: std::collections::HashMap<String, rune::TextEdit>,
    /// Card dock attachment drafts (the reference UI's per-card `files` state). Kept
    /// separate from the thread's composer deck: opening the full chat must
    /// not inherit files staged on the map.
    pub card_attachments: std::collections::HashMap<String, Vec<crate::chats::ChatAttachment>>,
    /// Amber error line under a card dock (`canvas-card-error`).
    pub card_attachment_errors: std::collections::HashMap<String, String>,
    /// Card currently showing the compact file-drop ring.
    pub card_drop: Option<String>,
    /// Card id whose connected-agents menu is open (single open at a time).
    pub agents_open: Option<String>,
    /// Kind dropdown open in the chats panel.
    pub kind_open: bool,
    /// Add menu (`CanvasAddMenu`): open flag, tab, live search.
    pub add_open: bool,
    pub add_tab: String,
    pub add_search: rune::TextEdit,
    /// Closed chats available for re-adding (id, title). Threads survive
    /// The close, so reopening restores history.
    pub closed: Vec<(String, String)>,
    /// Every chat on the server (sidebar rows), newest first. Empty until the
    /// first real snapshot lands (or when the server is unreachable).
    pub chat_rows: Vec<crate::chats::ChatSummary>,
    /// Every workspace on the server (paths + flags; sidebar filters and the
    /// active-run → tab mapping).
    pub workspaces: Vec<crate::chats::ChatWorkspace>,
    /// Open tab keys (`workspace:chat`, the reference UI `open_tabs`) and the active one.
    pub open_tabs: Vec<String>,
    pub active_tab: String,
    /// Keys the server knew at the previous snapshot (`knownChatIdsRef`):
    /// chats added since then auto-open as project tabs.
    pub known_chats: std::collections::HashSet<String>,
    /// Tabs whose history the shell still has to fetch (boot fallback +
    /// auto-opened project chats).
    pub pending_chat_fetches: Vec<(String, String)>,
    /// Chats whose history already landed on the map, so spawned child cards
    /// fetch once instead of on every snapshot.
    pub hydrated_chats: std::collections::HashSet<String>,
    /// The chat with a live backend run (`workspace:chat`, the reference UI
    /// `runningTabId`); drives the dot on sidebar rows and tabs.
    pub running_tab: Option<String>,
    /// True once a real snapshot replaced the seeded scene.
    pub real: bool,
    /// Cards created locally (a rekeyed `chat:local:*` draft) that server
    /// `open_tabs` may not list yet. Snapshots keep them open and local UI
    /// state stays the source of truth until the tab is closed.
    pub manual_cards: std::collections::HashSet<String>,
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
            cam_live: self.cam_live.clone(),
            cam_scale_live: self.cam_scale_live.clone(),
            grid_live: self.grid_live.clone(),
            hover: crate::motion::HoverSet::default(),
            cam_auto: self.cam_auto,
            cam_fitted: self.cam_fitted,
            layout_meta: self.layout_meta.clone(),
            view_w: self.view_w,
            view_h: self.view_h,
            projects: self.projects.clone(),
            agents: self.agents.clone(),
            voice: self.voice.clone(),
            kind_open: self.kind_open,
            add_open: self.add_open,
            add_tab: self.add_tab.clone(),
            add_search: self.add_search.clone(),
            closed: self.closed.clone(),
            chat_rows: self.chat_rows.clone(),
            workspaces: self.workspaces.clone(),
            open_tabs: self.open_tabs.clone(),
            active_tab: self.active_tab.clone(),
            known_chats: self.known_chats.clone(),
            pending_chat_fetches: self.pending_chat_fetches.clone(),
            hydrated_chats: self.hydrated_chats.clone(),
            running_tab: self.running_tab.clone(),
            real: self.real,
            manual_cards: self.manual_cards.clone(),
            inputs: self.inputs.clone(),
            card_attachments: self.card_attachments.clone(),
            card_attachment_errors: self.card_attachment_errors.clone(),
            card_drop: self.card_drop.clone(),
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
                    kind: "Main".to_string(),
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
                    kind: "Main".to_string(),
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
                    kind: "Main".to_string(),
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
            cam_live: rune::Live::new([0.0, 0.0]),
            cam_scale_live: rune::Live::new(1.0),
            grid_live: rune::Live::new([0.0, 0.0]),
            hover: crate::motion::HoverSet::default(),
            cam_auto: true,
            cam_fitted: false,
            layout_meta: std::collections::HashMap::new(),
            view_w: MAP_W - MAP_SIDEBAR_W,
            view_h: MAP_H,
            projects: crate::projects::ProjectsState::default(),
            agents: crate::agents::AgentsState::default(),
            voice: crate::voice::VoiceBus::default(),
            kind_open: false,
            add_open: false,
            add_tab: "chats".to_string(),
            add_search: rune::TextEdit::new(""),
            closed: Vec::new(),
            chat_rows: Vec::new(),
            workspaces: Vec::new(),
            open_tabs: Vec::new(),
            active_tab: String::new(),
            known_chats: std::collections::HashSet::new(),
            pending_chat_fetches: Vec::new(),
            hydrated_chats: std::collections::HashSet::new(),
            running_tab: None,
            real: false,
            manual_cards: std::collections::HashSet::new(),
            inputs: std::collections::HashMap::new(),
            card_attachments: std::collections::HashMap::new(),
            card_attachment_errors: std::collections::HashMap::new(),
            card_drop: None,
            agents_open: None,
        };
        state.seed_sessions();
        state
    }

    /// Seed session tabs from the scene cards (cards predate threads;
    /// threads materialize on open/send). Draft inputs and backing threads
    /// are ensured too, so the always-on home chat card has a live thread for
    /// whichever card is active.
    pub fn seed_sessions(&mut self) {
        let ids: Vec<String> = self.cards.iter().map(|c| c.id.clone()).collect();
        for id in &ids {
            if !self.sessions.order.contains(id) {
                self.sessions.order.push(id.clone());
            }
            self.inputs
                .entry(id.clone())
                .or_insert_with(|| rune::TextEdit::new(""));
            if !self.sessions.threads.contains_key(id) {
                let _ = self.thread_mut(id);
            }
        }
        if self.active_id.is_empty() {
            self.active_id = self.sessions.order.first().cloned().unwrap_or_default();
        }
        // The home chat overlay always shows the active session (the reference UI's
        // bottom `HouseChat` card); keep it in sync with the selection.
        if !self.active_id.is_empty() {
            self.sessions.sheet = Some(self.active_id.clone());
        }
    }

    /// Kind label for a real chat (`CHAT_KINDS` family).
    fn kind_for(workspace_kind: &str) -> &'static str {
        match workspace_kind {
            "project" | "dfd" => "Coding",
            "house" => "House",
            "personal" => "General",
            _ => "Main",
        }
    }

    /// Rebuild the map cards from `open_tabs` + `chat_rows` using the reference UI
    /// tidy-tree layout. Local draft cards survive the rebuild.
    fn sync_graph_cards(&mut self) {
        let snapshot = crate::chats::ChatSnapshot {
            open_tabs: self.open_tabs.clone(),
            active_tab: self.active_tab.clone(),
            pinned_chats: self.sessions.pinned.clone(),
            chats_filter: filter_for_kind(&self.sessions.kind).to_string(),
            workspaces: self.workspaces.clone(),
            messages: std::collections::HashMap::new(),
            chats: self.chat_rows.clone(),
        };
        let (graph, edges) = crate::chats::layout(&snapshot);
        if graph.is_empty() {
            return;
        }
        let old: std::collections::HashMap<String, MapCard> =
            self.cards.drain(..).map(|c| (c.id.clone(), c)).collect();
        let previous_meta = std::mem::take(&mut self.layout_meta);
        let mut layout_meta = std::collections::HashMap::new();
        let mut cards = Vec::new();
        for g in graph {
            let running = old.get(&g.id).map(|c| c.running).unwrap_or(false);
            // `CanvasStage` auto-sync: preserve the user's arrangement unless
            // The card's structural slot (depth / parent / tab) changed.
            let (x, y) = match (old.get(&g.id), previous_meta.get(&g.id)) {
                (Some(prev), Some(meta))
                    if meta.is_tab == g.is_tab
                        && meta.depth == g.depth
                        && meta.parent_item_id == g.parent_item_id =>
                {
                    (prev.x, prev.y)
                }
                _ => (g.x, g.y),
            };
            let kind = self
                .chat_rows
                .iter()
                .find(|c| c.card_id() == g.id)
                .map(|c| Self::kind_for(&c.workspace_kind))
                .unwrap_or("Main");
            layout_meta.insert(
                g.id.clone(),
                CardLayoutMeta {
                    is_tab: g.is_tab,
                    depth: g.depth,
                    parent_item_id: g.parent_item_id.clone(),
                },
            );
            cards.push(MapCard {
                id: g.id,
                title: g.title,
                subtitle: g.subtitle,
                kind: kind.to_string(),
                item: MapCardItem::Chat,
                project: None,
                x,
                y,
                compact: g.compact,
                running,
            });
        }
        // Locally-created drafts are not server tabs; keep them on the map.
        // A rekeyed draft is a real chat id but `open_tabs` still has to catch
        // up, so `manual_cards` keeps it mounted across snapshots.
        for (id, card) in old {
            let keep = id.starts_with("chat:local:") || self.manual_cards.contains(&id);
            if keep && !cards.iter().any(|c| c.id == id) {
                cards.push(card);
            }
        }
        self.cards = cards;
        self.layout_meta = layout_meta;
        self.edges = edges;
        self.sessions.order = self.cards.iter().map(|c| c.id.clone()).collect();
        // Spawned child cards are on the map without being tabs, so the boot
        // hydrate never touches them. Fetch each real chat's history once.
        let mut fetches = std::mem::take(&mut self.pending_chat_fetches);
        for card in &self.cards {
            let Some(key) = card.id.strip_prefix("chat:") else {
                continue;
            };
            if key.starts_with("local:") || self.hydrated_chats.contains(key) {
                continue;
            }
            let Some(pair) = split_chat_key(key) else {
                continue;
            };
            if !fetches.contains(&pair) {
                fetches.push(pair);
            }
        }
        self.pending_chat_fetches = fetches;
    }

    /// Adopt the boot snapshot: server layout wins once (open tabs, active
    /// tab, pins, filter), like `useSessions.boot`. Later polls keep local UI
    /// state and only reconcile the data.
    pub fn adopt_boot_snapshot(&mut self, snapshot: &crate::chats::ChatSnapshot) {
        if snapshot.open_tabs.is_empty() && snapshot.chats.is_empty() {
            // A successful empty boot is authoritative. Keeping the demo map
            // here left a fresh account showing four fake cards (and the add
            // FAB) instead of `canvas-empty`.
            self.real = true;
            self.workspaces = snapshot.workspaces.clone();
            self.chat_rows.clear();
            self.open_tabs.clear();
            self.active_tab.clear();
            self.active_id.clear();
            self.cards.clear();
            self.edges.clear();
            self.layout_meta.clear();
            self.sessions.order.clear();
            self.sessions.threads.clear();
            self.sessions.sheet = None;
            self.inputs.clear();
            self.card_attachments.clear();
            self.card_attachment_errors.clear();
            self.card_drop = None;
            self.known_chats.clear();
            self.hydrated_chats.clear();
            self.running_tab = None;
            return;
        }
        self.real = true;
        self.workspaces = snapshot.workspaces.clone();
        self.chat_rows = snapshot.chats.clone();
        self.seed_spawn_collapsed();
        let known: std::collections::HashSet<String> =
            self.chat_rows.iter().map(|c| c.key()).collect();
        self.known_chats = known.clone();
        // Restore open tabs — drop any that no longer resolve to a real chat.
        let mut restored: Vec<String> = snapshot
            .open_tabs
            .iter()
            .filter(|key| known.contains(*key))
            .cloned()
            .collect();
        if restored.is_empty() {
            if let Some(ws) = self.workspaces.iter().find(|w| w.is_general()) {
                if let Some(chat) = self.chat_rows.iter().find(|c| c.workspace_id == ws.id) {
                    restored.push(chat.key());
                }
            }
        }
        for id in &self.manual_cards {
            if let Some(key) = id.strip_prefix("chat:") {
                if !restored.iter().any(|open| open == key) {
                    restored.push(key.to_string());
                }
            }
        }
        self.open_tabs = restored;
        self.active_tab = if self.open_tabs.iter().any(|key| key == &snapshot.active_tab) {
            snapshot.active_tab.clone()
        } else {
            self.open_tabs.first().cloned().unwrap_or_default()
        };
        self.sessions.pinned = snapshot.pinned_chats.clone();
        if let Some(kind) = kind_for_filter(&snapshot.chats_filter) {
            self.sessions.kind = kind.to_string();
        }
        self.sync_graph_cards();
        self.hydrate_open_threads(snapshot);
        // A restored tab with no payload (first-general fallback, failed GET)
        // needs its history fetched once the snapshot channel is free.
        for key in &self.open_tabs {
            if !snapshot.messages.contains_key(key) {
                if let Some(pair) = split_chat_key(key) {
                    if !self.pending_chat_fetches.contains(&pair) {
                        self.pending_chat_fetches.push(pair);
                    }
                }
            }
        }
        self.sync_active_card(false);
        self.fit_once();
    }

    /// Adopt a later sessions poll: server chats are authoritative, local UI
    /// layout is not. Prunes dead tabs/pins, auto-opens new project chats,
    /// and keeps the selected local draft (the reference UI's 30s poll semantics).
    pub fn apply_chat_snapshot(&mut self, snapshot: &crate::chats::ChatSnapshot) {
        if snapshot.open_tabs.is_empty() && snapshot.chats.is_empty() {
            return;
        }
        self.real = true;
        self.workspaces = snapshot.workspaces.clone();
        self.chat_rows = snapshot.chats.clone();
        self.seed_spawn_collapsed();
        let known: std::collections::HashSet<String> =
            self.chat_rows.iter().map(|c| c.key()).collect();
        // Auto-open new project chats (coding tasks); General/House/Personal
        // chats only open when the user creates them explicitly. Spawned
        // children never auto-open as flat tabs (the reference UI `useSessions` poll).
        let mut fresh = Vec::new();
        for chat in &self.chat_rows {
            let key = chat.key();
            if self.known_chats.contains(&key) {
                continue;
            }
            let special = self
                .workspaces
                .iter()
                .find(|w| w.id == chat.workspace_id)
                .map(|w| w.is_general() || w.is_house() || w.is_personal())
                .unwrap_or(false);
            if !special && chat.parent_chat_id.is_none() {
                fresh.push(key);
            }
        }
        self.known_chats = known.clone();
        // Prune tabs pointing at chats deleted elsewhere; manual cards (a
        // locally rekeyed draft the server list has not caught up with yet)
        // stay mounted.
        self.open_tabs.retain(|key| {
            known.contains(key) || self.manual_cards.contains(&format!("chat:{key}"))
        });
        for id in &self.manual_cards {
            if let Some(key) = id.strip_prefix("chat:") {
                if !self.open_tabs.iter().any(|open| open == key) {
                    self.open_tabs.push(key.to_string());
                }
            }
        }
        for key in fresh {
            if !self.open_tabs.contains(&key) {
                self.open_tabs.push(key.clone());
                if let Some(pair) = split_chat_key(&key) {
                    self.pending_chat_fetches.push(pair);
                }
            }
        }
        // Drop pins for deleted chats too.
        self.sessions.pinned.retain(|key| known.contains(key));
        self.sync_graph_cards();
        if !self.open_tabs.iter().any(|key| key == &self.active_tab) {
            self.active_tab = self.open_tabs.first().cloned().unwrap_or_default();
        }
        self.hydrate_open_threads(snapshot);
        self.sync_active_card(true);
        self.fit_once();
    }

    /// Seed every parent with spawned children as collapsed on first sight
    /// (the reference UI's default-collapse effect). User expansions are never touched.
    fn seed_spawn_collapsed(&mut self) {
        let known: std::collections::HashSet<String> =
            self.chat_rows.iter().map(|chat| chat.key()).collect();
        let mut parents = std::collections::HashSet::new();
        for chat in &self.chat_rows {
            let Some(parent) = &chat.parent_chat_id else {
                continue;
            };
            let workspace = chat
                .parent_workspace_id
                .as_deref()
                .unwrap_or(&chat.workspace_id);
            let key = format!("{workspace}:{parent}");
            if key != chat.key() && known.contains(&key) {
                parents.insert(key);
            }
        }
        for key in parents {
            if !self.sessions.spawn_collapsed.contains(&key)
                && !self.sessions.spawn_expanded.contains(&key)
            {
                self.sessions.spawn_collapsed.insert(key);
            }
        }
    }

    /// Titles + persisted message payloads for every open tab. A live or
    /// aborting thread keeps its local rows until the run settles.
    fn hydrate_open_threads(&mut self, snapshot: &crate::chats::ChatSnapshot) {
        for key in self.open_tabs.clone() {
            let card_id = format!("chat:{key}");
            let Some(card) = self.card(&card_id) else {
                continue;
            };
            let title = card.title.clone();
            let chat_id = key
                .split_once(':')
                .map(|(_, c)| c.to_string())
                .unwrap_or_default();
            let thread = self.thread_mut(&card_id);
            thread.title = title;
            thread.chat_id = chat_id;
            if !thread.live && !thread.aborting {
                if let Some(rows) = snapshot.messages.get(&key) {
                    thread.rows = rows.iter().map(row_from_chat).collect();
                    thread.sync_usage_from_rows();
                }
            }
        }
    }

    /// Keep the selected card coherent with `active_tab`. `preserve_local`
    /// keeps a locally opened draft selected across polls; boot always takes
    /// The persisted layout.
    fn sync_active_card(&mut self, preserve_local: bool) {
        let want = format!("chat:{}", self.active_tab);
        let keep = preserve_local && self.card(&self.active_id).is_some();
        self.active_id = if keep {
            self.active_id.clone()
        } else if self.card(&want).is_some() {
            want
        } else {
            self.sessions.order.first().cloned().unwrap_or_default()
        };
        self.sessions.sheet = if self.active_id.is_empty() {
            None
        } else {
            Some(self.active_id.clone())
        };
    }

    fn fit_once(&mut self) {
        if self.cam_auto && !self.cam_fitted {
            let (vw, vh) = (self.view_w, self.view_h);
            self.cam.fit_in(&self.cards, vw, vh);
            self.sync_cam_live();
            self.cam_fitted = true;
        }
    }

    /// The persisted layout payload (`PUT /api/ui/state`), filtered to chats
    /// The server knows (the reference UI prunes before saving). Never writes local
    /// draft keys.
    pub fn ui_state_json(&self) -> serde_json::Value {
        let known: std::collections::HashSet<String> =
            self.chat_rows.iter().map(|c| c.key()).collect();
        let open_tabs: Vec<&String> = self
            .open_tabs
            .iter()
            .filter(|key| known.contains(*key))
            .collect();
        let active = if open_tabs.iter().any(|key| **key == self.active_tab) {
            serde_json::Value::String(self.active_tab.clone())
        } else {
            serde_json::Value::Null
        };
        let pinned: Vec<&String> = self
            .sessions
            .pinned
            .iter()
            .filter(|key| known.contains(*key))
            .collect();
        serde_json::json!({
            "open_tabs": open_tabs,
            "active_tab": active,
            "pinned_chats": pinned,
            "chats_filter": filter_for_kind(&self.sessions.kind),
        })
    }

    /// The reference UI `runningTabId`: map the backend's active run to its open tab.
    /// A path-less run belongs to the general workspace.
    pub fn running_tab_for(&self, run: Option<&crate::cloud::CloudActive>) -> Option<String> {
        let run = run.filter(|run| run.running)?;
        let ws = if run.workspace_path.trim().is_empty() {
            self.workspaces.iter().find(|w| w.is_general())
        } else {
            self.workspaces.iter().find(|w| {
                !w.path.is_empty() && w.path.eq_ignore_ascii_case(run.workspace_path.trim())
            })
        }?;
        Some(format!("{}:{}", ws.id, run.chat_id))
    }

    /// Fill one card's transcript from a per-chat history fetch.
    pub fn apply_chat_messages(&mut self, key: &str, rows: &[crate::chats::ChatMsgRow]) -> bool {
        let card_id = format!("chat:{key}");
        if !self.sessions.threads.contains_key(&card_id) {
            // Spawned child cards have no tab and no thread until they render;
            // create it here so their fetched history has somewhere to land.
            if self.card(&card_id).is_none() {
                return false;
            }
            self.thread_mut(&card_id);
        }
        self.hydrated_chats.insert(key.to_string());
        let Some(thread) = self.sessions.threads.get_mut(&card_id) else {
            return false;
        };
        // History refresh replaces the rows wholesale. Keep the seen set for
        // keys that survive, and drop the rest so a rewound/recreated row is
        // treated as a fresh mount again.
        let live: std::collections::HashSet<String> =
            rows.iter().map(|row| row.key.clone()).collect();
        thread.seen_rows.retain(|key| live.contains(key));
        thread.rows = rows.iter().map(row_from_chat).collect();
        thread.sync_usage_from_rows();
        thread.live = false;
        thread.aborting = false;
        if let Some((ws, chat)) = key.split_once(':') {
            thread.workspace_id = ws.to_string();
            thread.chat_id = chat.to_string();
            thread.chat_ready = true;
        }
        true
    }

    /// Open a real chat from the sidebar: add it as a tab, place it with the
    /// graph layout, and ask the shell to load its history.
    pub fn open_chat_row(&mut self, workspace_id: &str, chat_id: &str) {
        let key = format!("{workspace_id}:{chat_id}");
        let card_id = format!("chat:{key}");
        if self.card(&card_id).is_some() {
            self.select_card(&card_id);
            return;
        }
        if !self.open_tabs.contains(&key) {
            self.open_tabs.push(key.clone());
        }
        self.active_tab = key.clone();
        self.sync_graph_cards();
        if let Some(card) = self.card(&card_id) {
            let title = card.title.clone();
            let thread = self.thread_mut(&card_id);
            thread.title = title;
            thread.workspace_id = workspace_id.to_string();
            thread.chat_id = chat_id.to_string();
            thread.chat_ready = true;
            thread.rows.clear();
            thread.live = false;
        }
        self.pending_chat_fetches
            .push((workspace_id.to_string(), chat_id.to_string()));
        self.select_card(&card_id);
    }

    /// Open the workspace + first chat a finished `NewProjectDialog` created.
    /// The chat is not in a server snapshot yet, so register its workspace and
    /// row locally before laying the map out; the next chats poll reconciles.
    pub fn open_created_chat(&mut self, created: &crate::projects::NewProjectCreated) {
        let workspace = created.workspace.clone();
        if !self.workspaces.iter().any(|w| w.id == workspace.id) {
            self.workspaces.push(workspace.clone());
        }
        let key = format!("{}:{}", workspace.id, created.chat_id);
        if !self.chat_rows.iter().any(|chat| chat.key() == key) {
            let at = crate::rune_transcript::now_iso();
            self.chat_rows.push(crate::chats::ChatSummary {
                id: created.chat_id.clone(),
                workspace_id: workspace.id.clone(),
                workspace_name: workspace.name.clone(),
                workspace_kind: workspace.kind.clone(),
                title: created.chat_title.clone(),
                updated_at: at.clone(),
                created_at: at,
                ..crate::chats::ChatSummary::default()
            });
        }
        if !self.open_tabs.contains(&key) {
            self.open_tabs.push(key.clone());
        }
        self.active_tab = key.clone();
        self.sync_graph_cards();
        self.open_chat_row(&workspace.id, &created.chat_id);
        let card_id = format!("chat:{key}");
        // The new card is already on the map, so `open_chat_row` selects it
        // without touching its thread; get-or-create the thread here.
        let thread = self.thread_mut(&card_id);
        thread.title = created.chat_title.clone();
        thread.workspace_id = workspace.id.clone();
        thread.chat_id = created.chat_id.clone();
        thread.chat_ready = true;
        if !created.prompt.is_empty() {
            thread.edit.text = created.prompt.clone();
            thread.edit.anchor = thread.edit.text.len();
            thread.edit.focus = thread.edit.text.len();
        }
        self.manual_cards.insert(card_id.clone());
        self.select_card(&card_id);
    }

    pub fn card(&self, id: &str) -> Option<&MapCard> {
        self.cards.iter().find(|c| c.id == id)
    }

    /// Resize the canvas stage (shell poll). The camera keeps its view across
    /// resizes, like the reference UI (only the first card set triggers `fitView`).
    pub fn set_viewport(&mut self, w: f32, h: f32) -> bool {
        let w = w.max(320.0);
        let h = h.max(240.0);
        if (w - self.view_w).abs() < 0.5 && (h - self.view_h).abs() < 0.5 {
            return false;
        }
        self.view_w = w;
        self.view_h = h;
        true
    }

    /// Publish the camera to its live presentation values. Call after any
    /// mutation a view rebuild might not accompany.
    pub fn sync_cam_live(&self) {
        self.cam_live.set([self.cam.x, self.cam.y]);
        self.cam_scale_live.set(self.cam.scale);
        self.grid_live.set([
            (self.cam.x + DOT_STEP / 2.0).rem_euclid(DOT_STEP),
            (self.cam.y + DOT_STEP / 2.0).rem_euclid(DOT_STEP),
        ]);
    }

    /// The reference UI `relayout` (Reset layout): auto cards snap back to their tidy-tree
    /// slots, manual drafts stay put, then the map refits.
    pub fn relayout(&mut self) {
        let snapshot = crate::chats::ChatSnapshot {
            open_tabs: self.open_tabs.clone(),
            active_tab: self.active_tab.clone(),
            messages: std::collections::HashMap::new(),
            chats: self.chat_rows.clone(),
            ..Default::default()
        };
        let (graph, _) = crate::chats::layout(&snapshot);
        let positions: std::collections::HashMap<&str, (f32, f32)> =
            graph.iter().map(|g| (g.id.as_str(), (g.x, g.y))).collect();
        for card in &mut self.cards {
            if self.layout_meta.contains_key(&card.id) {
                if let Some(&(x, y)) = positions.get(card.id.as_str()) {
                    card.x = x;
                    card.y = y;
                }
            }
        }
        let cards = self.cards.clone();
        let (vw, vh) = (self.view_w, self.view_h);
        self.cam.fit_in(&cards, vw, vh);
        self.sync_cam_live();
        self.cam_auto = true;
        self.cam_fitted = true;
        self.press_home = None;
        self.press_pan = None;
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
        let vw = self.view_w;
        let vh = self.view_h;
        let k = self.cam.scale.clamp(0.65, 1.15);
        self.cam.scale = k;
        self.cam.x = vw / 2.0 - cx * k;
        self.cam.y = vh / 2.0 - cy * k;
        self.sync_cam_live();
        self.cam_auto = false;
    }

    /// Select a session: active row + camera focus + home chat overlay.
    pub fn select_card(&mut self, id: &str) {
        if self.card(id).is_none() {
            return;
        }
        self.active_id = id.to_string();
        if self.real {
            if let Some(key) = id.strip_prefix("chat:") {
                if key.contains(':') && !key.starts_with("local:") {
                    self.active_tab = key.to_string();
                }
            }
        }
        self.sessions.sheet = Some(id.to_string());
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
            if let Some(key) = id.strip_prefix("chat:") {
                if key.starts_with("local:") {
                    // `chat:local:*` is a draft card, not workspace "local".
                    // It lives in the general workspace and gets a real chat
                    // id on its first send (`HouseChat`'s activeChatId).
                    thread.workspace_id = "_general".to_string();
                    thread.chat_id.clear();
                    thread.chat_ready = false;
                } else if let Some((ws, chat)) = key.split_once(':') {
                    thread.workspace_id = ws.to_string();
                    thread.chat_id = chat.to_string();
                    thread.chat_ready =
                        !(chat.starts_with("rune-chat") || chat.starts_with("rune"));
                }
            }
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

    /// Rename a local draft card/tab/thread to the server's real chat key
    /// immediately after the internal `chat` frame. The thread keeps its
    /// rows and inbox, so the stream continues on the renamed card.
    pub fn rekey_local_chat(&mut self, old_id: &str, workspace_id: &str, chat_id: &str) -> bool {
        if !old_id.starts_with("chat:local:")
            || workspace_id.trim().is_empty()
            || chat_id.trim().is_empty()
        {
            return false;
        }
        let new_id = format!("chat:{workspace_id}:{chat_id}");
        let key = format!("{workspace_id}:{chat_id}");
        if old_id == new_id {
            return false;
        }
        let Some(mut thread) = self.sessions.threads.remove(old_id) else {
            return false;
        };
        thread.workspace_id = workspace_id.to_string();
        thread.chat_id = chat_id.to_string();
        thread.chat_ready = true;
        thread.pending_rekey = None;
        self.sessions.threads.insert(new_id.clone(), thread);
        for id in &mut self.sessions.order {
            if id == old_id {
                *id = new_id.clone();
            }
        }
        if self.sessions.sheet.as_deref() == Some(old_id) {
            self.sessions.sheet = Some(new_id.clone());
        }
        if self.active_id == old_id {
            self.active_id = new_id.clone();
        }
        if let Some(edit) = self.inputs.remove(old_id) {
            self.inputs.insert(new_id.clone(), edit);
        }
        if let Some(files) = self.card_attachments.remove(old_id) {
            self.card_attachments.insert(new_id.clone(), files);
        }
        if let Some(error) = self.card_attachment_errors.remove(old_id) {
            self.card_attachment_errors.insert(new_id.clone(), error);
        }
        if self.card_drop.as_deref() == Some(old_id) {
            self.card_drop = Some(new_id.clone());
        }
        if let Some(card) = self.cards.iter_mut().find(|c| c.id == old_id) {
            card.id = new_id.clone();
        }
        if let Some(meta) = self.layout_meta.remove(old_id) {
            self.layout_meta.insert(new_id.clone(), meta);
        }
        if let Some(closed) = self.closed.iter_mut().find(|(id, _)| id == old_id) {
            closed.0 = new_id.clone();
        }
        if !self.open_tabs.iter().any(|open| open == &key) {
            self.open_tabs.push(key.clone());
        }
        self.active_tab = key;
        self.manual_cards.insert(new_id);
        true
    }

    /// Get-or-create the message draft for a card.
    pub fn input_mut(&mut self, id: &str) -> &mut rune::TextEdit {
        self.inputs
            .entry(id.to_string())
            .or_insert_with(|| rune::TextEdit::new(""))
    }

    /// Files staged on a canvas card dock (the reference UI `CanvasItemCard.files`).
    pub fn card_attachments(&self, id: &str) -> &[crate::chats::ChatAttachment] {
        self.card_attachments
            .get(id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn add_card_attachments(&mut self, id: &str, paths: &[std::path::PathBuf]) {
        let files = self.card_attachments.entry(id.to_string()).or_default();
        let error = self
            .card_attachment_errors
            .entry(id.to_string())
            .or_default();
        let mut message = (!error.is_empty()).then(|| error.clone());
        crate::rune_transcript::add_attachment_paths_to(files, &mut message, paths);
        match message {
            Some(text) => {
                *error = text;
            }
            None => {
                self.card_attachment_errors.remove(id);
            }
        }
    }

    pub fn remove_card_attachment(&mut self, id: &str, target: &str) {
        let Some(files) = self.card_attachments.get_mut(id) else {
            return;
        };
        let error = self
            .card_attachment_errors
            .entry(id.to_string())
            .or_default();
        let mut message = (!error.is_empty()).then(|| error.clone());
        crate::rune_transcript::remove_attachment_from(files, &mut message, target);
        if let Some(text) = message {
            *error = text;
        } else {
            self.card_attachment_errors.remove(id);
        }
        if files.is_empty() {
            self.card_attachments.remove(id);
        }
    }

    pub fn clear_card_draft(&mut self, id: &str) {
        self.card_attachments.remove(id);
        self.card_attachment_errors.remove(id);
        if self.card_drop.as_deref() == Some(id) {
            self.card_drop = None;
        }
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
        let kind = self.sessions.kind.clone();
        let workspace_id = self.draft_workspace_id(&kind);
        self.new_session_in(&workspace_id)
    }

    /// Start a chat inside a specific workspace (the per-project `+`
    /// buttons in the Coding filter call this; the header `+` uses the
    /// mode workspace via `new_session`).
    pub fn new_session_in(&mut self, workspace_id: &str) -> String {
        let n = self.cards.len();
        let id = format!("chat:local:{}", crate::notes::mint_id("s"));
        let kind = self.sessions.kind.clone();
        let workspace_id = workspace_id.to_string();
        self.cards.push(MapCard {
            id: id.clone(),
            title: "New Chat".to_string(),
            subtitle: format!("{kind} · fresh"),
            kind,
            item: MapCardItem::Chat,
            project: None,
            x: 36.0 + ((n * 84) % 420) as f32,
            y: 36.0 + ((n * 64) % 320) as f32,
            compact: false,
            running: false,
        });
        {
            let thread = self.thread_mut(&id);
            thread.workspace_id = workspace_id;
        }
        self.inputs
            .entry(id.clone())
            .or_insert_with(|| rune::TextEdit::new(""));
        self.select_card(&id);
        id
    }

    /// Workspace a "+ New chat" lands in for the active mode (the reference UI's
    /// adaptive +): projects for Coding, the personal room for General.
    fn draft_workspace_id(&self, kind: &str) -> String {
        let by_kind = |pick: &dyn Fn(&crate::chats::ChatWorkspace) -> bool| {
            self.workspaces
                .iter()
                .find(|w| pick(w))
                .map(|w| w.id.clone())
        };
        match kind {
            "Coding" => self
                .workspaces
                .iter()
                .filter(|w| w.is_project())
                .min_by(|a, b| a.name.cmp(&b.name))
                .map(|w| w.id.clone())
                .unwrap_or_else(|| "_general".to_string()),
            "House" => by_kind(&|w| w.is_house()).unwrap_or_else(|| "_house".to_string()),
            "General" => by_kind(&|w| w.is_personal()).unwrap_or_else(|| "_personal".to_string()),
            _ => by_kind(&|w| w.is_general()).unwrap_or_else(|| "_general".to_string()),
        }
    }

    /// Close a session: the card and tab go, but the thread stays — reopening
    /// from the Add menu restores its history (closing a tab never deletes
    /// The chat, like the reference UI). Selection and sheet fall back to the first
    /// remaining session.
    pub fn close_session(&mut self, id: &str) {
        if let Some(card) = self.cards.iter().find(|c| c.id == id) {
            if card.item == MapCardItem::Chat && !self.closed.iter().any(|(cid, _)| cid == id) {
                self.closed.push((id.to_string(), card.title.clone()));
            }
        }
        // Closing a real tab drops it from the open set; the graph relayout
        // also removes children that were only visible under it.
        if let Some(key) = id.strip_prefix("chat:") {
            if self.open_tabs.iter().any(|k| k == key) {
                self.open_tabs.retain(|k| k != key);
                self.sync_graph_cards();
            }
        }
        self.cards.retain(|c| c.id != id);
        self.manual_cards.remove(id);
        self.clear_card_draft(id);
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
        // Closing a tab never leaves the map without its home chat card:
        // selection falls through to the first remaining session.
        self.sessions.sheet = if self.active_id.is_empty() {
            None
        } else {
            Some(self.active_id.clone())
        };
        let fallback = self.active_id.clone();
        if !fallback.is_empty() {
            let _ = self.thread_mut(&fallback);
        }
        // Keep the persisted active tab in step with the selection the reference UI's
        // `closeTab` would make (`next[min(index, len-1)]`).
        if let Some(key) = self
            .active_id
            .strip_prefix("chat:")
            .filter(|key| !key.starts_with("local:"))
        {
            self.active_tab = key.to_string();
        } else if !self.open_tabs.contains(&self.active_tab) {
            self.active_tab = self.open_tabs.first().cloned().unwrap_or_default();
        }
    }

    /// Is this `workspace:chat` pinned in the active filter's list?
    pub fn is_pinned(&self, key: &str) -> bool {
        self.sessions.pinned.iter().any(|pin| pin == key)
    }

    /// The reference UI `togglePin`: append new pins, remove existing ones.
    pub fn toggle_pin(&mut self, key: &str) {
        if let Some(index) = self.sessions.pinned.iter().position(|pin| pin == key) {
            self.sessions.pinned.remove(index);
        } else {
            self.sessions.pinned.push(key.to_string());
        }
    }

    /// Start inline rename (the reference UI `startRename`): seed the field with the
    /// current title and clear any armed delete.
    pub fn start_rename(&mut self, sid: &str) {
        let title = self
            .chat_rows
            .iter()
            .find(|chat| chat.key() == sid)
            .map(|chat| chat.title.clone())
            .or_else(|| {
                self.card(&format!("chat:{sid}"))
                    .map(|card| card.title.clone())
            })
            .unwrap_or_default();
        self.sessions.confirm_delete = None;
        // The reference UI focuses the input and selects the title. Rune has no
        // imperative focus call, but a model field marked focused takes focus
        // when it first mounts; `anchor..focus` selects the whole title
        // (blur writes `focused = false` back here, which the shell tick
        // commits).
        let mut edit = rune::TextEdit::new(title);
        edit.anchor = 0;
        edit.focus = edit.text.len();
        edit.focused = true;
        self.sessions.editing = Some((sid.to_string(), edit));
    }

    /// Commit the inline rename: a trimmed non-empty title that differs from
    /// The row's current one becomes an optimistic rename. Returns the write
    /// The shell still has to send (`PUT …/title`).
    pub fn commit_editing(&mut self) -> Option<(String, String, String)> {
        let (sid, edit) = self.sessions.editing.take()?;
        let title = edit.text.trim().to_string();
        if title.is_empty() {
            return None;
        }
        let (workspace_id, chat_id) = sid.split_once(':')?;
        if workspace_id.is_empty() || chat_id.is_empty() {
            return None;
        }
        let previous = self
            .chat_rows
            .iter()
            .find(|chat| chat.key() == sid)
            .map(|chat| chat.title.clone())
            .or_else(|| {
                self.card(&format!("chat:{sid}"))
                    .map(|card| card.title.clone())
            });
        if previous.as_deref() == Some(title.as_str()) {
            return None;
        }
        self.rename_chat_local(workspace_id, chat_id, &title);
        Some((workspace_id.to_string(), chat_id.to_string(), title))
    }

    /// First/second Delete press (the reference UI `handleDelete`): arm the 2.5s confirm,
    /// then start the 220ms fade-out.
    pub fn press_delete(&mut self, sid: &str) {
        if self.sessions.deleting.is_some() {
            return;
        }
        let now = crate::clock::now_millis();
        let confirming = self
            .sessions
            .confirm_delete
            .as_ref()
            .is_some_and(|(id, at)| id == sid && now < *at);
        if confirming {
            self.sessions.confirm_delete = None;
            self.sessions.deleting = Some((sid.to_string(), now, now + 220));
        } else {
            self.sessions.confirm_delete = Some((sid.to_string(), now + 2500));
        }
    }

    /// Is this row showing the red confirm state (`Click again to confirm`)?
    pub fn delete_confirming(&self, sid: &str) -> bool {
        self.sessions
            .confirm_delete
            .as_ref()
            .is_some_and(|(id, at)| id == sid && crate::clock::now_millis() < *at)
    }

    /// Eased progress (0 at the first frame, 1 at removal) of a row in its
    /// 220 ms `house-row-out` animation. The shell wakes every frame while
    /// this is `Some`, and the view drives opacity, slide and slot height
    /// from this one number so they cannot drift apart.
    pub fn delete_progress(&self, sid: &str) -> Option<f32> {
        let (id, start, end) = self.sessions.deleting.as_ref()?;
        if id != sid {
            return None;
        }
        let span = end.saturating_sub(*start).max(1) as f32;
        let elapsed = crate::clock::now_millis().saturating_sub(*start) as f32;
        let t = (elapsed / span).clamp(0.0, 1.0);
        // `house-row-out` and `sidebar-row-out` both use
        // `cubic-bezier(0.4, 0, 0.2, 1)` for every animated property.
        Some(rune::Easing::cubic(0.4, 0.0, 0.2, 1.0).eval(t))
    }

    /// Opacity of a row in its 220ms delete fade, `None` when it is not
    /// deleting. Used by the mounted-row test; the view drives both opacity
    /// and height from [`CanvasState::delete_progress`].
    #[cfg(test)]
    pub fn delete_fade(&self, sid: &str) -> Option<f32> {
        self.delete_progress(sid).map(|t| 1.0 - t)
    }

    /// Optimistic rename (the reference UI `renameChatLocal`): title + timestamp update
    /// everywhere before the `PUT …/title` lands.
    pub fn rename_chat_local(&mut self, workspace_id: &str, chat_id: &str, title: &str) {
        let now = crate::rune_transcript::now_iso();
        let key = format!("{workspace_id}:{chat_id}");
        if let Some(chat) = self.chat_rows.iter_mut().find(|c| c.key() == key) {
            chat.title = title.to_string();
            chat.updated_at = now;
        }
        let card_id = format!("chat:{key}");
        if let Some(card) = self.cards.iter_mut().find(|c| c.id == card_id) {
            card.title = title.to_string();
        }
        if let Some(thread) = self.sessions.threads.get_mut(&card_id) {
            thread.title = title.to_string();
        }
    }

    /// Optimistic delete (the reference UI `removeChatLocal` + `closeTab`): drop the row,
    /// pin, tab and card; the `DELETE` runs in the background.
    pub fn remove_chat_local(&mut self, workspace_id: &str, chat_id: &str) {
        let key = format!("{workspace_id}:{chat_id}");
        let card_id = format!("chat:{key}");
        self.chat_rows.retain(|chat| chat.key() != key);
        self.sessions.pinned.retain(|pin| pin != &key);
        self.known_chats.remove(&key);
        self.close_session(&card_id);
        self.closed.retain(|(id, _)| id != &card_id);
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

fn rgba(r: f32, g: f32, b: f32, a: f32) -> rune::Color {
    rune::Color {
        r: r / 255.0,
        g: g / 255.0,
        b: b / 255.0,
        a,
    }
}

/// `.canvas-card`'s full `box-shadow` stack: inset, optional ring/halo, drop.
fn canvas_card_shadows(active: bool, running: bool) -> Vec<rune::Shadow> {
    let top = if active {
        0.08
    } else if running {
        0.07
    } else {
        0.045
    };
    let drop = |y: f32, blur: f32, a: f32| rune::Shadow {
        x: 0.0,
        y,
        blur,
        spread: 0.0,
        color: rune::Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a,
        },
        inset: false,
    };
    let halo = |blur: f32, a: f32| rune::Shadow {
        x: 0.0,
        y: 0.0,
        blur,
        spread: 0.0,
        color: rune::Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a,
        },
        inset: false,
    };
    let mut shadows = vec![crate::motion::inset_top_light(top)];
    if active {
        shadows.push(rune::Shadow {
            x: 0.0,
            y: 0.0,
            blur: 0.0,
            spread: 1.0,
            color: rune::Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.05,
            },
            inset: false,
        });
        shadows.push(drop(28.0, 74.0, 0.58));
        shadows.push(halo(52.0, 0.06));
    } else if running {
        shadows.push(drop(24.0, 66.0, 0.56));
        shadows.push(halo(44.0, 0.05));
    } else {
        shadows.push(drop(26.0, 70.0, 0.50));
    }
    shadows
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
    /// Live chat state for sidebar mutations (rename/delete). Standalone
    /// canvas scenes have none.
    fn chats_mut(&mut self) -> Option<&mut crate::chats::ChatsState> {
        None
    }
}

impl CanvasHost for CanvasState {
    fn canvas_mut(&mut self) -> &mut CanvasState {
        self
    }
}

impl crate::rune_transcript::TranscriptHost for CanvasState {
    fn thread(
        &self,
        which: crate::rune_transcript::Thread,
    ) -> &crate::rune_transcript::RuneTranscriptState {
        use crate::rune_transcript::Thread;
        let id = match which {
            Thread::Card(index) => self.sessions.order.get(index).cloned(),
            Thread::Main | Thread::Open => self
                .sessions
                .sheet
                .clone()
                .or_else(|| Some(self.active_id.clone())),
        };
        id.and_then(|id| self.sessions.threads.get(&id))
            .or_else(|| self.sessions.threads.values().next())
            .expect("canvas card views always have a thread")
    }

    fn thread_mut(
        &mut self,
        which: crate::rune_transcript::Thread,
    ) -> &mut crate::rune_transcript::RuneTranscriptState {
        use crate::rune_transcript::Thread;
        let id = match which {
            Thread::Card(index) => self.sessions.order.get(index).cloned(),
            Thread::Main | Thread::Open => self
                .sessions
                .sheet
                .clone()
                .or_else(|| Some(self.active_id.clone())),
        };
        match id {
            Some(id) => self.thread_mut(&id),
            None => {
                let fallback = self.active_id.clone();
                self.thread_mut(&fallback)
            }
        }
    }
}

/// Commit (or drop) an inline rename once the field lost focus, the reference UI's
/// `onBlur`. The shell calls this from its poll, so the input's old node is
/// gone before the draft is dropped.
pub fn settle_sidebar_editing<S: CanvasHost + 'static>(s: &mut S) -> bool {
    let Some((_, edit)) = s.canvas_mut().sessions.editing.as_ref() else {
        return false;
    };
    if edit.focused {
        return false;
    }
    commit_sidebar_rename(s);
    true
}

/// Commit an open inline rename and fire its `PUT …/title` (`HouseSidebar`'s
/// `onBlur`/Enter). Standalone canvas scenes have no chat state to write to;
/// they still commit the optimistic local title.
pub fn commit_sidebar_rename<S: CanvasHost + 'static>(s: &mut S) {
    let Some((workspace_id, chat_id, title)) = s.canvas_mut().commit_editing() else {
        return;
    };
    if let Some(chats) = s.chats_mut() {
        crate::chats::rename_chat(chats, &workspace_id, &chat_id, &title);
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
        ink("#a1a1aa42")
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
        // `.canvas-card`: inset top-light over the drop, both layers.
        .box_shadow(crate::motion::inset_top_light(0.045))
        .box_shadow(rune::Shadow {
            x: 0.0,
            y: 26.0,
            blur: 70.0,
            spread: 0.0,
            color: rgba(0.0, 0.0, 0.0, 0.50),
            inset: false,
        })
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
                    rune::el()
                        .key(format!("{id}-close"))
                        .size(14.0, 14.0)
                        .role("button")
                        .focusable(true)
                        .label(format!("Remove {}", card.title))
                        .shape(crate::lucide_x(14.0, 2.0, ink("#71717a")))
                        .on_press_enter(move |s: &mut S| {
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
            .color(ink("#a1a1aa")),
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

/// Canvas dock picker. Native opens the real file dialog; the browser build
/// opens a real `<input type="file">` and uploads through the shared
/// multipart endpoint.
fn pick_card_attachments<S: CanvasHost + 'static>(_s: &mut S, id: &str, tx: &mut rune::Tx) {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = tx;
        crate::transport::pick_files(crate::transport::WebTarget::Card(id.to_string()));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let options = rune::FileDialogOptions {
            title: "Attach files".to_string(),
            ..Default::default()
        };
        let target = id.to_string();
        tx.open_file(options, move |s: &mut S, result, _tx| match result {
            Ok(Some(path)) => s.canvas_mut().add_card_attachments(&target, &[path]),
            Ok(None) => {}
            Err(error) => {
                s.canvas_mut()
                    .card_attachment_errors
                    .insert(target.clone(), error.to_string());
            }
        });
    }
}

/// Lucide `Paperclip` approximation at a 24-unit box, matching the reference UI's card
/// dock attach control.
fn paperclip_icon(size: f32, color: rune::Color) -> Shape {
    let p = Path::new()
        .move_to([16.0, 6.0])
        .line_to([7.6, 14.4])
        .cubic_to([6.4, 15.6], [6.4, 17.4], [7.6, 18.6])
        .cubic_to([8.8, 19.8], [10.6, 19.8], [11.8, 18.6])
        .line_to([19.0, 11.4])
        .cubic_to([21.0, 9.4], [21.0, 6.0], [19.0, 4.0])
        .cubic_to([17.0, 2.0], [13.6, 2.0], [11.6, 4.0])
        .line_to([4.0, 11.6])
        .cubic_to([1.4, 14.2], [1.4, 18.4], [4.0, 21.0])
        .cubic_to([6.6, 23.6], [10.8, 23.6], [13.4, 21.0])
        .line_to([21.4, 13.0]);
    Shape::new()
        .stroke_path(&p, crate::lucide_stroke(size), LineCap::Round, color)
        .view_box(24.0, 24.0)
}

fn mini_bubble<S: CanvasHost + 'static>(
    id: &str,
    r: &crate::rune_transcript::RowData,
    live_row: bool,
    copied: bool,
    armed: bool,
    can_rewind: bool,
) -> El {
    // `CanvasChatBubble` + `.canvas-bubble*`: agent rows put the 22pt avatar
    // first, user rows put it after the bubble.
    let (fill, hair_a, avatar_bg, avatar_ink, initial, name, body_ink) = match r.role {
        "user" => (
            "#28282f", 0.075, "#303038", "#e6e6ec", "U", "You", "#dedee5",
        ),
        "error" => (
            "#fb718511",
            0.24,
            "#24242a",
            "#c9c9d1",
            "O",
            "Error",
            "#d2d2da",
        ),
        _ => (
            "#19191e", 0.05, "#24242a", "#c9c9d1", "O", "Agent", "#d2d2da",
        ),
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
    let mut head = row().items_center().gap(6.0).mb(5.0).child(
        text(name.to_string())
            .font(rune::tok::TextStyle::new(11.0, 650.0))
            .color(ink("#f0f0f3")),
    );
    if r.role == "agent" {
        // `auto` badge (`.canvas-bubble-badge`).
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
        .radius(14.0)
        .fill(ink(if live_row { "#1b1b20" } else { fill }))
        .border(1.0, hairline)
        .border_layout()
        .px(13.0)
        .py(10.0)
        .max_w(390.0)
        .child(head)
        .child(crate::rune_transcript::render_markdown_standalone(
            &format!("{id}-mini-{}-md", r.key),
            &r.body,
            body_ink,
            {
                let mut style = rune::tok::TextStyle::new(12.5, 400.0);
                style.line_height = 12.5 * 1.55;
                style
            },
        ));
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
    // `.canvas-bubble-foot`: 22pt tall. Actions sit left and stay invisible
    // at rest (but occupy their slot); the clock is pushed right.
    let t = crate::rune_transcript::clock(&r.at);
    let copy_target = id.to_string();
    let copy_key = r.key.clone();
    let copy_text = if r.raw_body.is_empty() {
        r.body.clone()
    } else {
        r.raw_body.clone()
    };
    let rewind_target = id.to_string();
    let rewind_key = r.key.clone();
    let rewind_attachments = r.attachments.clone();
    let mut actions = row()
        .key(format!("{id}-mini-{}-actions", r.key))
        .items_center()
        .gap(6.0)
        .group_opacity(0.0, 1.0)
        .child(
            rune::el()
                .key(format!("{id}-mini-{}-copy", r.key))
                .role("button")
                .focusable(true)
                .label(if copied { "Copied" } else { "Copy message" })
                .help(if copied { "Copied" } else { "Copy message" })
                .size(22.0, 22.0)
                .radius(6.0)
                .center()
                .hover(ink("#ffffff13"))
                .hover_group()
                .on_press_tx_enter(move |s: &mut S, tx: &mut rune::Tx| {
                    tx.copy(copy_text.clone());
                    let now = crate::clock::now_millis();
                    if let Some(thread) = s.canvas_mut().sessions.threads.get_mut(&copy_target) {
                        thread.copied_messages.insert(copy_key.clone(), now);
                    }
                    tx.wake_after(std::time::Duration::from_millis(50));
                })
                .child(if copied {
                    rune::el()
                        .size(12.0, 12.0)
                        .inert()
                        .shape(crate::lucide_check(12.0, 2.0, ink("#34d399")))
                } else {
                    rune::el()
                        .size(12.0, 12.0)
                        .inert()
                        .shape(crate::rune_transcript::home_icon_copy(12.0, ink("#6f6f79")))
                }),
        );
    if r.role == "user" {
        if armed {
            actions = actions.child(
                row()
                    .key(format!("{id}-mini-{}-rewind", r.key))
                    .role("button")
                    .focusable(true)
                    .label("Confirm rewind")
                    .items_center()
                    .gap(4.0)
                    .h(22.0)
                    .px(7.0)
                    .radius(6.0)
                    .fill(ink("#fb718524"))
                    .on_press_tx_enter(move |s: &mut S, _tx: &mut rune::Tx| {
                        let c = s.canvas_mut();
                        if let Some(thread) = c.sessions.threads.get_mut(&rewind_target) {
                            crate::rune_transcript::rewind_row(
                                thread,
                                &rewind_key,
                                rewind_attachments.clone(),
                            );
                        }
                    })
                    .child(rune::el().size(11.0, 11.0).inert().shape(
                        crate::rune_transcript::home_icon_rotate(11.0, ink("#fda4af")),
                    ))
                    .child(
                        text("Rewind?".to_string())
                            .font(rune::tok::TextStyle::new(9.0, 650.0))
                            .color(ink("#fda4af")),
                    ),
            );
        } else {
            actions = actions.child(
                rune::el()
                    .key(format!("{id}-mini-{}-rewind", r.key))
                    .role("button")
                    .focusable(true)
                    .label("Rewind to this message")
                    .help(if can_rewind {
                        "Rewind to before this message"
                    } else {
                        "Wait for this run to finish"
                    })
                    .size(22.0, 22.0)
                    .radius(6.0)
                    .center()
                    .hover(ink("#fb71851a"))
                    .hover_group()
                    .disabled(!can_rewind)
                    .opacity(if can_rewind { 1.0 } else { 0.4 })
                    .on_press_tx_enter(move |s: &mut S, tx: &mut rune::Tx| {
                        if !can_rewind {
                            return;
                        }
                        if let Some(thread) =
                            s.canvas_mut().sessions.threads.get_mut(&rewind_target)
                        {
                            thread
                                .rewind_armed
                                .insert(rewind_key.clone(), crate::clock::now_millis());
                        }
                        tx.wake_after(std::time::Duration::from_millis(50));
                    })
                    .child(rune::el().size(12.0, 12.0).inert().shape(
                        crate::rune_transcript::home_icon_rotate(12.0, ink("#6f6f79")),
                    )),
            );
        }
    }
    bubble = bubble.child(
        row()
            .w_full()
            .items_center()
            .min_h(22.0)
            .mt(8.0)
            .child(actions)
            .child(rune::el().grow(1.0))
            .child(
                text(t)
                    .key(format!("{id}-mini-{}-time", r.key))
                    .font(crate::motion::tabular(rune::tok::TextStyle::new(
                        9.5, 400.0,
                    )))
                    .color(ink("#6d6d77")),
            ),
    );
    let avatar = || {
        rune::el()
            .size(22.0, 22.0)
            .mt(2.0)
            .radius(11.0)
            .border(1.0, ink("#ffffff12"))
            .fill(ink(avatar_bg))
            .center()
            .child(
                text(initial.to_string())
                    .font(rune::tok::TextStyle::new(9.5, 700.0))
                    .color(ink(avatar_ink)),
            )
    };
    if r.role == "user" {
        row()
            .key(format!("{id}-minrow-{}", r.key))
            .w_full()
            .items_start()
            .justify_end()
            .gap(8.0)
            .hover_group()
            .child(bubble)
            .child(avatar())
    } else {
        row()
            .key(format!("{id}-minrow-{}", r.key))
            .w_full()
            .items_start()
            .gap(8.0)
            .hover_group()
            .child(avatar())
            .child(bubble)
    }
}

#[allow(clippy::too_many_arguments)]
fn card_view<S: CanvasHost + crate::rune_transcript::TranscriptHost + 'static>(
    card: &MapCard,
    active: bool,
    card_index: usize,
    thread: Option<&crate::rune_transcript::RuneTranscriptState>,
    input: &rune::TextEdit,
    files: &[crate::chats::ChatAttachment],
    files_error: Option<&str>,
    drop_active: bool,
    kids: &[(String, String)],
    menu_open: bool,
    hover: &crate::motion::HoverSet,
) -> El {
    if card.item == MapCardItem::Project {
        return project_card_view::<S>(card, active);
    }
    let (w, h) = card.size();
    let radius = if card.compact { 16.0 } else { 18.0 };
    let id = card.id.clone();
    let drag_id = card.id.clone();
    // Borders mirror `canvas.css`: default .085, running .22, active .30.
    let border = if active {
        ink("#ffffff4d")
    } else if card.running {
        ink("#ffffff38")
    } else {
        ink("#ffffff16")
    };
    let hover_border = hover.border(format!("{id}-border"), border);
    let hover_offset = hover.offset(format!("{id}-offset"), [0.0, 0.0]);
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
        .border(1.0, border);
    // `box-shadow` is a layer list, not one slot: the inset top-light, the
    // drop, and the active/running ring + halo all draw.
    for shadow in canvas_card_shadows(active, card.running) {
        view = view.box_shadow(shadow);
    }
    view = view.clip_rounded(radius).hover_group();
    // `.canvas-card:hover`: a 1 pt lift on every card; the white/.16 border
    // only on the plain card because `.is-active`/`.is-running` keep their
    // stronger border colours. `Live` tracks avoid a rebuild per pointer move.
    let set_border = hover_border.clone();
    let set_offset = hover_offset.clone();
    view = view
        .live_offset(hover_offset.smooth(crate::motion::HOVER_SPRING))
        .on_hover_change(move |_s: &mut S, inside, tx| {
            set_border.set(if inside {
                ink("#ffffff29")
            } else {
                ink("#ffffff16")
            });
            set_offset.set(if inside { [0.0, -1.0] } else { [0.0, 0.0] });
            tx.live_only();
        });
    if !active && !card.running {
        view = view.live_border(hover_border.smooth(crate::motion::HOVER_SPRING));
    }
    if card.running {
        // `canvas.css .canvas-card.is-running::before`: a 110° gradient over a
        // 220% background, swept from 140% to -80% over 2.6 s and masked to the
        // 1 pt ring. Rune paints it fragment-by-fragment, so corners and AA are
        // exact and no child overlay can cover the card's content.
        view = view.border_sweep(rune::BorderSweep {
            width: 1.0,
            angle: 110.0f32.to_radians(),
            color: rune::Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.48,
            },
            stops: [0.22, 0.47, 0.72],
            size: [2.2, 1.0],
            from: 140.0,
            to: -80.0,
            period: 2.6,
        });
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
    // Status badge (`StatusBadge`): static running dot (the reference UI's
    // `.canvas-card-status-dot` has no animation), ✓ done, ✕ failed, quiet
    // dot idle. Rune has no icon font — glyphs, same geometry.
    let badge: El = {
        let (border, fill, tone) = match status {
            "running" => ("#ffffff21", "#ffffff0e", "#ededf0"),
            "done" => ("#3ddc9738", "#3ddc9713", "#3ddc97"),
            "failed" => ("#fb71853d", "#fb718513", "#fb7185"),
            _ => ("#ffffff0e", "#ffffff09", "#6f6f79"),
        };
        let size = if card.compact { 24.0 } else { 26.0 };
        let mut chip = rune::el()
            .key(format!("{id}-badge"))
            .size(size, size)
            .radius(if card.compact { 7.0 } else { 8.0 })
            .border(1.0, ink(border))
            .fill(ink(fill))
            .center();
        chip = chip.child(match status {
            "running" => rune::el()
                .key(format!("{id}-badge-dot"))
                .size(5.0, 5.0)
                .radius(2.5)
                .fill(ink(tone)),
            "done" => rune::el()
                .key(format!("{id}-badge-mark"))
                .size(12.0, 12.0)
                .inert()
                .shape(crate::lucide_check(12.0, 2.6, ink(tone))),
            "failed" => rune::el()
                .key(format!("{id}-badge-mark"))
                .size(12.0, 12.0)
                .inert()
                .shape(crate::lucide_x(12.0, 2.6, ink(tone))),
            _ => rune::el()
                .key(format!("{id}-badge-dot"))
                .size(5.0, 5.0)
                .radius(2.5)
                .fill(ink(tone)),
        });
        chip
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
            rune::el()
                .key(format!("{id}-grip"))
                .size(13.0, 13.0)
                .inert()
                .shape(grip_icon(ink("#ffffff33"))),
        )
        .child(
            rune::el()
                .key(format!("{id}-typeicon"))
                .size(
                    if card.compact { 30.0 } else { 34.0 },
                    if card.compact { 30.0 } else { 34.0 },
                )
                .radius(if card.compact { 10.0 } else { 11.0 })
                .border(1.0, ink("#ffffff13"))
                .fill(ink("#1c1c22"))
                .center()
                .child(
                    rune::el()
                        .key(format!("{id}-typeicon-glyph"))
                        .size(16.0, 16.0)
                        .inert()
                        .shape(kind_icon(&card.kind, ink("#d8d8de"))),
                ),
        )
        .child(
            col()
                .grow(1.0)
                .min_h(0.0)
                .role("button")
                .focusable(true)
                .label(format!("Open {}", card.title))
                .on_press_enter(move |s: &mut S| {
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
                        .truncate()
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
                        .truncate()
                        .color(ink("#8a8a93")),
                ),
        )
        .child(badge)
        .child({
            let mut agents_btn = row()
                .key(format!("{id}-agents"))
                .size(26.0, 26.0)
                .radius(8.0)
                .center()
                .gap(3.0)
                .role("button")
                .focusable(true)
                .label(format!("Connected agents ({})", kids.len()))
                .on_press_enter(move |s: &mut S| {
                    let c = s.canvas_mut();
                    if c.agents_open.as_deref() == Some(menu_id.as_str()) {
                        c.agents_open = None;
                    } else {
                        c.agents_open = Some(menu_id.clone());
                    }
                })
                .child(
                    rune::el()
                        .key(format!("{id}-agents-icon"))
                        .size(14.0, 14.0)
                        .inert()
                        .shape(network_icon(ink("#7c7c85"))),
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
            rune::el()
                .key(format!("{id}-close"))
                .size(26.0, 26.0)
                .center()
                .role("button")
                .focusable(true)
                .label(format!("Close {}", card.title))
                .child(rune::el().size(14.0, 14.0).inert().shape(crate::lucide_x(
                    14.0,
                    2.0,
                    ink("#71717a"),
                )))
                .on_press_enter(move |s: &mut S| {
                    s.canvas_mut().close_session(&close_id);
                }),
        );
    // Conversation (`canvas-card-conversation`): the reference UI shows the latest user
    // and agent rows, top-aligned in a flex-1 scroll area (2/16/14 padding,
    // 14pt gap). It must grow, or the dock rides up under the content.
    let convo = {
        let base = col()
            .key(format!("{id}-convo"))
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .gap(14.0)
            .px(16.0)
            .pt(2.0)
            .pb(14.0)
            .scroll(rune::ScrollSpec {
                stick_bottom: false,
            });
        match thread {
            Some(t) if !t.rows.is_empty() => {
                let mut idxs: Vec<usize> = Vec::new();
                if let Some(i) = t.rows.iter().rposition(|r| r.role == "user") {
                    idxs.push(i);
                }
                if let Some(i) = t.rows.iter().rposition(|r| r.role != "user") {
                    idxs.push(i);
                }
                idxs.sort_unstable();
                idxs.dedup();
                if idxs.is_empty() {
                    idxs.push(t.rows.len() - 1);
                }
                let mut convo = base;
                for i in idxs {
                    let row = &t.rows[i];
                    convo = convo.child(mini_bubble::<S>(
                        &id,
                        row,
                        live && row.key == "live-agent",
                        t.copied_messages.contains_key(&row.key),
                        t.rewind_armed.contains_key(&row.key),
                        !live && t.rewind_inbox.is_none(),
                    ));
                }
                convo
            }
            _ => base.child(
                rune::el()
                    .key(format!("{id}-convo-empty"))
                    .w_full()
                    .min_h(150.0)
                    .center()
                    .child(
                        text("No messages yet.")
                            .font(rune::tok::TextStyle::new(11.0, 400.0))
                            .color(ink("#5f5f69")),
                    ),
            ),
        }
    };
    view = view.child(head);
    // Connected-agents menu (`canvas-card-agent-menu`): child chats of this
    // card; a press selects the child.
    if menu_open {
        let mut menu = col()
            .key(format!("{id}-agents-menu"))
            .w_full()
            .gap(2.0)
            .transition(crate::motion::menu_enter())
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
                    .focusable(true)
                    .label(format!("Open {kid_title}"))
                    .hover(ink("#ffffff0a"))
                    .hover_group()
                    .on_press_enter(move |s: &mut S| {
                        let c = s.canvas_mut();
                        c.agents_open = None;
                        c.select_card(&target);
                    })
                    .child(
                        text(kid_title.clone())
                            .font(rune::tok::TextStyle::new(11.0, 500.0))
                            .color(ink("#d4d4d8"))
                            .group_color(ink("#ffffff")),
                    ),
            );
        }
        view = view.child(menu);
    }
    view = view.child(convo);
    // Canvas card dock. Pending questions replace the prompt dock with the
    // compact Ask panel on every card except the active one (whose full
    // transcript owns the question). The compact panel caps at 250pt, the
    // same `canvas.css` cap as the reference UI.
    let ask_batch = if active {
        None
    } else {
        thread
            .and_then(|t| t.ask.visible_batch(&t.chat_id).cloned())
            .zip(thread)
    };
    let dock = if let Some((batch, t)) = ask_batch {
        col()
            .key(format!("{id}-dock"))
            .w_full()
            .px(14.0)
            .pt(4.0)
            .pb(14.0)
            .child(crate::ask::ask_panel::<S>(
                &batch,
                &t.ask,
                crate::rune_transcript::Thread::Card(card_index),
                true,
                h,
            ))
    } else {
        // Card dock (`canvas-card-dock`): 4/14/14 padding around the bordered
        // input wrap; attach, input, and the 25pt send button.
        let draft_empty = input.text.trim().is_empty();
        let send_id = id.clone();
        let edit_id = id.clone();
        let send_files = files.to_vec();
        let send_disabled = live || (draft_empty && files.is_empty());
        let send_offset = hover.offset(format!("{id}-send-offset"), [0.0, 0.0]);
        let set_send_offset = send_offset.clone();
        let pick_id = id.clone();
        let attach = rune::el()
            .key(format!("{id}-attach"))
            .role("button")
            .focusable(true)
            .label("Attach files")
            .help("Attach files")
            .size(22.0, 22.0)
            .radius(6.0)
            .center()
            .hover(ink("#ffffff12"))
            .hover_group()
            .on_press_tx_enter(move |s: &mut S, tx: &mut rune::Tx| {
                pick_card_attachments::<S>(s, &pick_id, tx);
            })
            .child(
                rune::el()
                    .size(12.0, 12.0)
                    .inert()
                    .shape(paperclip_icon(12.0, ink("#676b74"))),
            );
        let mut files_row = row()
            .key(format!("{id}-files"))
            .w_full()
            .wrap()
            .gap(4.0)
            .mb(6.0);
        for (index, file) in files.iter().enumerate() {
            let remove_id = id.clone();
            let target = if file.id.is_empty() {
                format!("local:{index}")
            } else {
                file.id.clone()
            };
            let icon = if file.is_image {
                crate::lucide_image(9.0, ink("#b9b9c1"))
            } else if file.file_type.starts_with("audio/") {
                crate::lucide_audio_lines(9.0, ink("#b9b9c1"))
            } else {
                crate::lucide_file_text(9.0, ink("#b9b9c1"))
            };
            files_row = files_row.child(
                row()
                    .key(format!("{id}-file-{index}"))
                    .items_center()
                    .gap(4.0)
                    .px(5.0)
                    .py(3.0)
                    .radius(6.0)
                    .border(1.0, ink("#ffffff14"))
                    .fill(ink("#ffffff09"))
                    .child(rune::el().size(9.0, 9.0).inert().shape(icon))
                    .child(
                        text(file.name.clone())
                            .font(rune::tok::TextStyle::new(8.5, 400.0))
                            .max_w(150.0)
                            .truncate()
                            .color(ink("#b9b9c1")),
                    )
                    .child(
                        rune::el()
                            .key(format!("{id}-file-{index}-remove"))
                            .role("button")
                            .focusable(true)
                            .label(format!("Remove {}", file.name))
                            .size(13.0, 13.0)
                            .radius(4.0)
                            .center()
                            .hover(ink("#ffffff1a"))
                            .hover_group()
                            .on_press_enter(move |s: &mut S| {
                                s.canvas_mut().remove_card_attachment(&remove_id, &target);
                            })
                            .child(rune::el().size(8.0, 8.0).inert().shape(crate::lucide_x(
                                8.0,
                                2.0,
                                ink("#74747d"),
                            ))),
                    ),
            );
        }
        let wrap_border = if drop_active {
            rune::Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.3,
            }
        } else {
            ink("#ffffff16")
        };
        let wrap = row()
            .key(format!("{id}-input-wrap"))
            .w_full()
            .min_h(38.0)
            .items_center()
            .gap(5.0)
            .px(5.0)
            .py(4.0)
            .radius(10.0)
            .border(1.0, wrap_border)
            .border_layout()
            .fill(rune::Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.22,
            })
            .focus_within_ring(1.0, -1.0, ink("#ffffff33"))
            .child(attach)
            .child(
                text_input(input, rune::tok::TextStyle::new(10.5, 400.0))
                    .key(format!("{id}-input"))
                    .grow(1.0)
                    .min_w(0.0)
                    .placeholder(
                        if live {
                            "Agent is running — you can type ahead…"
                        } else {
                            "Message this chat…"
                        },
                        ink("#5e626b"),
                    )
                    .caret(ink("#f2f2f4"), crate::motion::TEXT_SELECTION)
                    .on_edit(move |s: &mut S| s.canvas_mut().input_mut(&edit_id)),
            )
            .child(
                rune::el()
                    .key(format!("{id}-send"))
                    .role("button")
                    .focusable(true)
                    .label(format!("Send to {send_id}"))
                    .help(if live {
                        "Wait for this run to finish"
                    } else {
                        "Send message"
                    })
                    .size(25.0, 25.0)
                    .radius(8.0)
                    .fill(if send_disabled {
                        ink("#ffffff14")
                    } else {
                        ink("#f2f2f4")
                    })
                    .center()
                    // `.gp-composer-send:hover`: white face and a 1 pt lift.
                    .hover(if send_disabled {
                        ink("#00000000")
                    } else {
                        ink("#ffffff")
                    })
                    .live_offset(send_offset.smooth(crate::motion::HOVER_SPRING))
                    .on_hover_change(move |_s: &mut S, inside, tx| {
                        if send_disabled {
                            return;
                        }
                        set_send_offset.set(if inside { [0.0, -1.0] } else { [0.0, 0.0] });
                        tx.live_only();
                    })
                    .on_press_enter(move |s: &mut S| {
                        let c = s.canvas_mut();
                        let prompt = c
                            .inputs
                            .get(&send_id)
                            .map(|e| e.text.trim().to_string())
                            .unwrap_or_default();
                        if prompt.is_empty() && send_files.is_empty() {
                            return;
                        }
                        if c.sessions.threads.get(&send_id).is_some_and(|t| t.live) {
                            return;
                        }
                        let thread = c.thread_mut(&send_id);
                        let staged = crate::rune_transcript::stage_send_prompt_with_attachments(
                            thread,
                            prompt,
                            send_files.clone(),
                        );
                        if staged {
                            if let Some(e) = c.inputs.get_mut(&send_id) {
                                e.text.clear();
                                e.anchor = 0;
                                e.focus = 0;
                            }
                            c.card_attachments.remove(&send_id);
                            c.card_attachment_errors.remove(&send_id);
                        } else {
                            let error = c
                                .sessions
                                .threads
                                .get(&send_id)
                                .and_then(|t| t.attachments_error.clone());
                            if let Some(error) = error {
                                c.card_attachment_errors.insert(send_id.clone(), error);
                            }
                        }
                    })
                    .child(
                        rune::el()
                            .size(12.0, 12.0)
                            .inert()
                            .shape(crate::lucide_arrow_up(
                                12.0,
                                2.0,
                                ink(if send_disabled { "#5e626b" } else { "#0a0a0d" }),
                            )),
                    ),
            );
        let drop_id = id.clone();
        let hover_id = id.clone();
        let mut body = col()
            .key(format!("{id}-dock"))
            .w_full()
            .px(14.0)
            .pt(4.0)
            .pb(14.0)
            .accepts_files(|_| true)
            .on_drop_files(move |s: &mut S, dropped: &[std::path::PathBuf], _tx| {
                s.canvas_mut().add_card_attachments(&drop_id, dropped);
                s.canvas_mut().card_drop = None;
            })
            .on_drop_hover(move |s: &mut S, over, _tx| {
                s.canvas_mut().card_drop = over.then(|| hover_id.clone());
            });
        if !files.is_empty() {
            body = body.child(files_row);
        }
        body = body.child(wrap);
        if let Some(error) = files_error {
            body = body.child(
                text(error.to_string())
                    .key(format!("{id}-files-error"))
                    .mt(4.0)
                    .font(rune::tok::TextStyle::new(8.5, 400.0))
                    .color(ink("#fda4af")),
            );
        }
        body
    };
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
/// The menu. No drag-drop onto the canvas (no DnD target — click only).
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
                .focusable(true)
                .label(format!("{label} tab"))
                .on_press_enter(move |s: &mut S| {
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
        .transition(crate::motion::canvas_menu_enter())
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
                        .caret(ink("#f2f2f4"), crate::motion::TEXT_SELECTION)
                        .on_edit(move |s: &mut S| &mut s.canvas_mut().add_search),
                )
                .child(
                    rune::el()
                        .key("canvas-add-close")
                        .size(12.0, 12.0)
                        .role("button")
                        .focusable(true)
                        .label("Close add menu")
                        .shape(crate::lucide_x(12.0, 2.0, ink("#71717a")))
                        .on_press_enter(move |s: &mut S| {
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
        .focusable(true)
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
        btn = btn.on_press_enter(on_add);
    }
    btn
}

pub fn canvas_view(s: &CanvasState, _: &mut Cx<CanvasState>) -> El {
    canvas_body::<CanvasState>(s)
}

/// Canvas body shared by the standalone view and the shell.
pub fn canvas_body<S: CanvasHost + crate::rune_transcript::TranscriptHost + 'static>(
    s: &CanvasState,
) -> El {
    let dot = rune::Color {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 0.075,
    };
    let vw = s.view_w.max(1.0);
    let vh = s.view_h.max(1.0);
    // World layer: everything that pans/zooms. Origin top-left so pan offsets
    // map 1:1; `scale` springs on the layout spring and hit tests follow the
    // scaled rect (`El::scale`). Chrome (HUD, chat overlay) stays fixed, like
    // The reference UI's `[data-canvas-ui]`.
    let mut world = col()
        .key("canvas-world")
        .absolute(0.0, 0.0)
        .live_offset(&s.cam_live)
        .size(vw, vh)
        .origin(0.0, 0.0)
        .live_scale(&s.cam_scale_live);
    // Background pan surface: fixed, unscaled (the reference UI `CanvasStage` drag on
    // The stage). Cards sit above it, so empty presses pan while header
    // presses drag cards.
    let pan = rune::el()
        .key("canvas-pan")
        .absolute(0.0, 0.0)
        .size(vw, vh)
        .fill(ink("#08080a"))
        .on_drag(move |s: &mut S, drag: &mut Drag, tx| {
            match drag.phase {
                Phase::Down => {
                    let c = s.canvas_mut();
                    c.press_pan = Some((c.cam.x, c.cam.y));
                }
                Phase::Move | Phase::Up => {
                    let c = s.canvas_mut();
                    if let Some((hx, hy)) = c.press_pan {
                        c.cam.x = hx + drag.dx;
                        c.cam.y = hy + drag.dy;
                        c.cam_auto = false;
                        c.sync_cam_live();
                        // The world and grid follow at draw time; no view run.
                        tx.live_only();
                    }
                    if matches!(drag.phase, Phase::Up) {
                        s.canvas_mut().press_pan = None;
                    }
                }
            }
        });
    // Fixed dotted grid (the reference UI `.canvas-grid`, map variant): 26pt tile,
    // 1pt dots, background position follows the camera pan (never its zoom).
    let mut grid = col()
        .key("canvas-grid")
        .absolute(0.0, 0.0)
        .live_offset(&s.grid_live)
        .size(vw + DOT_STEP, vh + DOT_STEP)
        .inert();
    // The field is anchored at 0 and one tile larger than the viewport; the
    // live grid offset supplies the camera remainder, so a pan never rebuilds.
    let mut gy = 0.0;
    while gy < vh + DOT_STEP {
        let mut gx = 0.0;
        while gx < vw + DOT_STEP {
            grid = grid.child(
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
            .size(vw, vh)
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
        let card_index = s
            .sessions
            .order
            .iter()
            .position(|id| id == &card.id)
            .unwrap_or(0);
        world = world.child(card_view::<S>(
            card,
            card.id == s.active_id,
            card_index,
            s.sessions.threads.get(&card.id),
            input,
            s.card_attachments(&card.id),
            s.card_attachment_errors.get(&card.id).map(String::as_str),
            s.card_drop.as_deref() == Some(card.id.as_str()),
            &kids,
            s.agents_open.as_deref() == Some(card.id.as_str()),
            &s.hover,
        ));
    }
    // Fixed-chrome area: clips the scaled world, holds composer + HUD.
    // `grow` (not `h_full`): shares a shell row without overflowing it.
    let mut canvas = col()
        .key("canvas-area")
        .grow(1.0)
        .min_h(0.0)
        .fill(ink("#08080a"))
        .clip_rounded(0.0)
        // The reference UI: dotted grid + stage background are fixed under the
        // transformed world (dots slide with pan, not zoom).
        .child(pan)
        .child(grid);
    if s.cards.is_empty() {
        // Empty canvas (`canvas-empty`, map variant): a transparent button
        // with a 27pt white circle and the copy under it, pinned top-left at
        // `left:24; top:16` (`.canvas-stage.is-map .canvas-empty`). The add
        // FAB is not mounted in this state (the reference UI `items.length > 0`).
        let scale = s.hover.scale("canvas-empty-add-scale", 1.0);
        let fill = s.hover.fill("canvas-empty-add-fill", ink("#ffffff"));
        let set_scale = scale.clone();
        let set_fill = fill.clone();
        canvas = canvas.child(
            col()
                .key("canvas-empty")
                .absolute(24.0, 16.0)
                .items_start()
                .gap(9.0)
                .max_w(320.0)
                .child(
                    row()
                        .key("canvas-empty-add")
                        .items_center()
                        .gap(8.0)
                        .role("button")
                        .focusable(true)
                        .label("Add to canvas")
                        .on_hover_change(move |_s: &mut S, inside, tx| {
                            set_scale.set(if inside { 1.04 } else { 1.0 });
                            set_fill.set(if inside {
                                ink("#e8e8ea")
                            } else {
                                ink("#ffffff")
                            });
                            tx.live_only();
                        })
                        .on_press_enter(move |s: &mut S| {
                            s.canvas_mut().add_open = true;
                        })
                        .child(
                            rune::el()
                                .key("canvas-empty-add-circle")
                                .size(27.0, 27.0)
                                .radius(13.5)
                                .fill(ink("#ffffff"))
                                .live_fill(fill.smooth(crate::motion::HOVER_SPRING))
                                .live_scale(scale.smooth(crate::motion::HOVER_SPRING))
                                .center()
                                .inert()
                                .child(
                                    rune::el()
                                        .key("canvas-empty-add-plus")
                                        .size(14.0, 14.0)
                                        .inert()
                                        .shape({
                                            let color = ink("#0a0a0d");
                                            rune::Shape::new()
                                                .line(
                                                    [12.0, 5.0],
                                                    [12.0, 19.0],
                                                    crate::lucide_stroke(14.0),
                                                    LineCap::Round,
                                                    color,
                                                )
                                                .line(
                                                    [5.0, 12.0],
                                                    [19.0, 12.0],
                                                    crate::lucide_stroke(14.0),
                                                    LineCap::Round,
                                                    color,
                                                )
                                                .view_box(24.0, 24.0)
                                        }),
                                ),
                        )
                        .child(
                            text("Add to Canvas".to_string())
                                .font({
                                    let mut style = rune::tok::TextStyle::new(11.5, 500.0);
                                    style.line_height = 17.25;
                                    style
                                })
                                .color(ink("#b8bac2")),
                        ),
                )
                .child(
                    text("Open a chat and it will appear here automatically.".to_string())
                        .key("canvas-empty-copy")
                        .max_w(320.0)
                        .font({
                            let mut style = rune::tok::TextStyle::new(10.0, 400.0);
                            style.line_height = 15.0;
                            style
                        })
                        .color(ink("#5e626b")),
                ),
        );
    } else {
        canvas = canvas.child(world);
    }
    if !s.cards.is_empty() {
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
                .hover(ink("#1d1d22"))
                .hover_group()
                .pl(7.0)
                .pr(9.0)
                .backdrop_blur(12.0)
                .role("button")
                .focusable(true)
                .label("Add to canvas")
                .on_press_enter(move |s: &mut S| {
                    s.canvas_mut().add_open = true;
                })
                .child(
                    rune::el()
                        .key("canvas-add-plus")
                        .size(13.0, 13.0)
                        .inert()
                        .shape({
                            let color = ink("#a9abb3");
                            rune::Shape::new()
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
                        }),
                )
                .child(
                    text("Add".to_string())
                        .font(rune::tok::TextStyle::new(9.5, 600.0))
                        .color(ink("#a9abb3"))
                        .group_color(ink("#f4f4f5")),
                ),
        );
    }
    if s.add_open {
        // Backdrop catcher (the reference UI's window pointerdown): anything outside
        // The menu closes it. Transparent, below the menu.
        canvas = canvas.child(
            rune::el()
                .key("canvas-add-backdrop")
                .absolute(0.0, 0.0)
                .size(vw, vh)
                .fill(rune::Color {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.0,
                })
                .role("button")
                .focusable(true)
                .label("Close add menu")
                .modal(true, move |s: &mut S| {
                    s.canvas_mut().add_open = false;
                }),
        );
        canvas = canvas.child(add_menu::<S>(s));
    }
    // Camera HUD (`CanvasStage` map controls): zoom out/in at the reference UI
    // 0.86/1.16 steps, fit-to-view, and layout reset. Fixed chrome — never
    // scaled with the world.
    // (Add menu lives just above: `add_menu` + `add_row`.)
    let hud_icon = |key: &str, label: &str, icon: Shape| {
        rune::el()
            .key(format!("cam-{key}"))
            .role("button")
            .focusable(true)
            .label(label.to_string())
            .size(28.0, 28.0)
            .radius(8.0)
            .center()
            .hover(ink("#ffffff0a"))
            .hover_group()
            .child(
                rune::el()
                    .key(format!("cam-{key}-icon"))
                    .size(13.0, 13.0)
                    .inert()
                    .shape(icon),
            )
    };
    canvas = canvas.child(
        row()
            .key("cam-hud")
            .absolute(vw - 127.0, 10.0)
            .items_center()
            .gap(1.0)
            .child(
                hud_icon("zoom-out", "Zoom out", icon_minus(ink("#81848d"))).on_press_tx_enter(
                    move |s: &mut S, tx| {
                        let c = s.canvas_mut();
                        c.cam_auto = false;
                        c.cam.zoom_by(0.86);
                        c.sync_cam_live();
                        // The world scales at draw time; no view run.
                        tx.live_only();
                    },
                ),
            )
            .child(
                hud_icon("zoom-in", "Zoom in", icon_plus(ink("#81848d"))).on_press_tx_enter(
                    move |s: &mut S, tx| {
                        let c = s.canvas_mut();
                        c.cam_auto = false;
                        c.cam.zoom_by(1.16);
                        c.sync_cam_live();
                        tx.live_only();
                    },
                ),
            )
            .child(
                hud_icon("fit", "Fit to view", icon_scan(ink("#81848d"))).on_press_tx_enter(
                    move |s: &mut S, tx| {
                        let c = s.canvas_mut();
                        let cards = c.cards.clone();
                        let (vw, vh) = (c.view_w, c.view_h);
                        c.cam.fit_in(&cards, vw, vh);
                        c.sync_cam_live();
                        c.cam_auto = true;
                        tx.live_only();
                    },
                ),
            )
            .child(
                hud_icon("reset", "Reset layout", icon_reset(ink("#81848d"))).on_press_enter(
                    move |s: &mut S| {
                        // `relayout`: tidy-tree slots again plus a fresh fit.
                        s.canvas_mut().relayout();
                    },
                ),
            ),
    );
    // Sidebar rows mirror the cards: the active chat carries the white-9%
    // wash (`.house-sidebar-row.is-active`); a press switches selection.
    // Rows key off card ids so the board and the list can never disagree.
    // NOTE: the side panel (chats/projects/agents/cloud/…) renders
    // shell-side — `canvas_body` is the canvas area only, so the panel can
    // swap while the map stays mounted. `chats_panel` below is the Chats tab.
    canvas
}

/// Minus (13pt camera HUD).
fn icon_minus(color: rune::Color) -> Shape {
    Shape::new()
        .line(
            [5.0, 12.0],
            [19.0, 12.0],
            crate::lucide_stroke(13.0),
            LineCap::Round,
            color,
        )
        .view_box(24.0, 24.0)
}

/// Plus (13pt camera HUD).
fn icon_plus(color: rune::Color) -> Shape {
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
}

/// Scan: four rounded corner brackets (Lucide, 13pt fit-to-view).
fn icon_scan(color: rune::Color) -> Shape {
    let p = Path::new()
        .move_to([3.0, 7.0])
        .line_to([3.0, 5.0])
        .cubic_to([3.0, 3.9], [3.9, 3.0], [5.0, 3.0])
        .line_to([7.0, 3.0])
        .move_to([17.0, 3.0])
        .line_to([19.0, 3.0])
        .cubic_to([20.1, 3.0], [21.0, 3.9], [21.0, 5.0])
        .line_to([21.0, 7.0])
        .move_to([21.0, 17.0])
        .line_to([21.0, 19.0])
        .cubic_to([21.0, 20.1], [20.1, 21.0], [19.0, 21.0])
        .line_to([17.0, 21.0])
        .move_to([7.0, 21.0])
        .line_to([5.0, 21.0])
        .cubic_to([3.9, 21.0], [3.0, 20.1], [3.0, 19.0])
        .line_to([3.0, 17.0]);
    Shape::new()
        .stroke_path(&p, crate::lucide_stroke(13.0), LineCap::Round, color)
        .view_box(24.0, 24.0)
}

/// RotateCcw: three-quarter arc + corner arrow (Lucide, 13pt reset).
fn icon_reset(color: rune::Color) -> Shape {
    const K: f32 = 4.971;
    let p = Path::new()
        .move_to([12.0, 3.0])
        .cubic_to([12.0 + K, 3.0], [21.0, 12.0 - K], [21.0, 12.0])
        .cubic_to([21.0, 12.0 + K], [12.0 + K, 21.0], [12.0, 21.0])
        .cubic_to([12.0 - K, 21.0], [3.0, 12.0 + K], [3.0, 12.0])
        .cubic_to([3.0, 9.9], [3.8, 7.8], [5.2, 6.2])
        .move_to([3.0, 3.0])
        .line_to([3.0, 8.0])
        .line_to([8.0, 8.0]);
    Shape::new()
        .stroke_path(&p, crate::lucide_stroke(13.0), LineCap::Round, color)
        .view_box(24.0, 24.0)
}

/// GripVertical: two columns of three dots (13pt card drag handle).
fn grip_icon(color: rune::Color) -> Shape {
    let mut shape = Shape::new();
    for (x, y) in [
        (9.0, 5.0),
        (15.0, 5.0),
        (9.0, 12.0),
        (15.0, 12.0),
        (9.0, 19.0),
        (15.0, 19.0),
    ] {
        let points: Vec<[f32; 2]> = (0..8)
            .map(|i| {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                [x + a.cos() * 1.15, y + a.sin() * 1.15]
            })
            .collect();
        shape = shape.fill(points, color);
    }
    shape.view_box(24.0, 24.0)
}

/// Actor tile glyph: MessageSquare / Code2 / Home by chat kind (16pt).
fn kind_icon(kind: &str, color: rune::Color) -> Shape {
    match kind {
        "Coding" => {
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
                .stroke_path(&p, crate::lucide_stroke(16.0), LineCap::Round, color)
                .view_box(24.0, 24.0)
        }
        "House" => {
            let p = Path::new()
                .move_to([3.0, 10.0])
                .line_to([12.0, 3.0])
                .line_to([21.0, 10.0])
                .line_to([21.0, 20.0])
                .line_to([3.0, 20.0])
                .close()
                .move_to([9.0, 20.0])
                .line_to([9.0, 14.0])
                .line_to([15.0, 14.0])
                .line_to([15.0, 20.0]);
            Shape::new()
                .stroke_path(&p, crate::lucide_stroke(16.0), LineCap::Round, color)
                .view_box(24.0, 24.0)
        }
        _ => chat_row_icon(16.0, color),
    }
}

/// Network: three nodes wired together (14pt connected-agents button).
fn network_icon(color: rune::Color) -> Shape {
    let square = |x: f32, y: f32| {
        Path::new()
            .move_to([x, y])
            .line_to([x + 6.0, y])
            .line_to([x + 6.0, y + 6.0])
            .line_to([x, y + 6.0])
            .close()
    };
    let p = square(9.0, 2.0)
        .move_to([5.0, 16.0])
        .line_to([5.0, 12.0])
        .line_to([19.0, 12.0])
        .line_to([19.0, 16.0])
        .move_to([12.0, 12.0])
        .line_to([12.0, 8.0]);
    let p = p
        .move_to([2.0, 16.0])
        .line_to([8.0, 16.0])
        .line_to([8.0, 22.0])
        .line_to([2.0, 22.0])
        .close()
        .move_to([16.0, 16.0])
        .line_to([22.0, 16.0])
        .line_to([22.0, 22.0])
        .line_to([16.0, 22.0])
        .close();
    Shape::new()
        .stroke_path(&p, crate::lucide_stroke(14.0), LineCap::Round, color)
        .view_box(24.0, 24.0)
}

/// Chevron down (Lucide, 14pt) for the sidebar mode switcher.
fn chevron_down(size: f32, color: rune::Color) -> Shape {
    let p = Path::new()
        .move_to([6.0, 9.0])
        .line_to([12.0, 15.0])
        .line_to([18.0, 9.0]);
    Shape::new()
        .stroke_path(&p, crate::lucide_stroke(size), LineCap::Round, color)
        .view_box(24.0, 24.0)
}

/// Search glyph (Lucide `Search`: 8pt lens + angled handle).
fn search_icon(color: rune::Color) -> Shape {
    const K: f32 = 4.418;
    let p = Path::new()
        .move_to([19.0, 11.0])
        .cubic_to([19.0, 11.0 + K], [11.0 + K, 19.0], [11.0, 19.0])
        .cubic_to([11.0 - K, 19.0], [3.0, 11.0 + K], [3.0, 11.0])
        .cubic_to([3.0, 11.0 - K], [11.0 - K, 3.0], [11.0, 3.0])
        .cubic_to([11.0 + K, 3.0], [19.0, 11.0 - K], [19.0, 11.0])
        .close()
        .move_to([21.0, 21.0])
        .line_to([16.65, 16.65]);
    Shape::new()
        .stroke_path(&p, crate::lucide_stroke(11.0), LineCap::Round, color)
        .view_box(24.0, 24.0)
}

/// Sidebar row glyph: Lucide v1.17 `code-xml` (`Code2`, `</>`) for chats
/// outside the General workspace.
fn code_row_icon(size: f32, color: rune::Color) -> Shape {
    // Lucide v1.17 `code-xml`: `m18 16 4-4-4-4` + `m6 8-4 4 4 4` + slash.
    let p = Path::new()
        .move_to([18.0, 16.0])
        .line_to([22.0, 12.0])
        .line_to([18.0, 8.0])
        .move_to([6.0, 8.0])
        .line_to([2.0, 12.0])
        .line_to([6.0, 16.0])
        .move_to([14.5, 4.0])
        .line_to([9.5, 20.0])
        .close();
    Shape::new()
        .stroke_path(&p, crate::lucide_stroke(size), LineCap::Round, color)
        .view_box(24.0, 24.0)
}

/// Sidebar row glyph: Lucide v1.17 `message-square`.
fn chat_row_icon(size: f32, color: rune::Color) -> Shape {
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

/// Chevron right (Lucide, 14pt) for collapsed spawn nests.
fn chevron_right(size: f32, color: rune::Color) -> Shape {
    let p = Path::new()
        .move_to([9.0, 18.0])
        .line_to([15.0, 12.0])
        .line_to([9.0, 6.0]);
    Shape::new()
        .stroke_path(&p, crate::lucide_stroke(size), LineCap::Round, color)
        .view_box(24.0, 24.0)
}

/// Corner-down-right glyph for nested spawned rows.
fn corner_down_right(color: rune::Color) -> Shape {
    let p = Path::new()
        .move_to([4.0, 4.0])
        .line_to([4.0, 11.0])
        .cubic_to([4.0, 13.2], [5.8, 15.0], [8.0, 15.0])
        .line_to([20.0, 15.0])
        .move_to([15.0, 10.0])
        .line_to([20.0, 15.0])
        .line_to([15.0, 20.0]);
    Shape::new()
        .stroke_path(&p, crate::lucide_stroke(10.0), LineCap::Round, color)
        .view_box(24.0, 24.0)
}

/// Lucide `Bot` (11–12pt): spawned chats get the agent glyph.
pub(crate) fn bot_icon(size: f32, color: rune::Color) -> Shape {
    let p = Path::new()
        .move_to([12.0, 8.0])
        .line_to([12.0, 4.0])
        .line_to([8.0, 4.0])
        .move_to([8.0, 8.0])
        .cubic_to([5.8, 8.0], [4.0, 9.8], [4.0, 12.0])
        .line_to([4.0, 16.0])
        .cubic_to([4.0, 18.2], [5.8, 20.0], [8.0, 20.0])
        .line_to([16.0, 20.0])
        .cubic_to([18.2, 20.0], [20.0, 18.2], [20.0, 16.0])
        .line_to([20.0, 12.0])
        .cubic_to([20.0, 9.8], [18.2, 8.0], [16.0, 8.0])
        .close()
        .move_to([2.0, 14.0])
        .line_to([4.0, 14.0])
        .move_to([20.0, 14.0])
        .line_to([22.0, 14.0])
        .move_to([15.0, 13.0])
        .line_to([15.0, 15.0])
        .move_to([9.0, 13.0])
        .line_to([9.0, 15.0]);
    Shape::new()
        .stroke_path(&p, crate::lucide_stroke(size), LineCap::Round, color)
        .view_box(24.0, 24.0)
}

/// One nested spawned chat (`renderKidLeaves`): quiet 11.5pt row with the
/// corner connector + bot glyph, indented under its expanded parent.
fn sidebar_spawn_row<S: CanvasHost + 'static>(
    chat: &crate::chats::ChatSummary,
    hover: &crate::motion::HoverSet,
) -> El {
    let title = crate::chats::display_title(&chat.title);
    let ws = chat.workspace_id.clone();
    let chat_id = chat.id.clone();
    let card_id = chat.card_id();
    let mut style = rune::tok::TextStyle::new(11.5, 400.0);
    style.line_height = 17.25;
    let offset = hover.offset(format!("side-kid-{card_id}-offset"), [0.0, 0.0]);
    let set_offset = offset.clone();
    row()
        .key(format!("side-kid-{card_id}"))
        .live_offset(offset.smooth(crate::motion::HOVER_SPRING))
        .on_hover_change(move |_s: &mut S, inside, tx| {
            set_offset.set(if inside { [1.0, 0.0] } else { [0.0, 0.0] });
            tx.live_only();
        })
        .w_full()
        .items_center()
        .gap(6.0)
        .pl(26.0)
        .pr(8.0)
        .py(6.0)
        .radius(8.0)
        .hover(rgba(255.0, 255.0, 255.0, 0.04))
        .role("button")
        .focusable(true)
        .label(format!("Open {title}"))
        .on_press_enter(move |s: &mut S| {
            commit_sidebar_rename(s);
            s.canvas_mut().open_chat_row(&ws, &chat_id);
        })
        .child(
            rune::el()
                .key(format!("side-kid-{card_id}-corner"))
                .size(10.0, 10.0)
                .inert()
                .shape(corner_down_right(ink("#3f3f46"))),
        )
        .child(
            rune::el()
                .key(format!("side-kid-{card_id}-bot"))
                .size(11.0, 11.0)
                .inert()
                .shape(bot_icon(11.0, ink("#52525b"))),
        )
        .child(
            text(title)
                .key(format!("side-kid-{card_id}-title"))
                .grow(1.0)
                .min_w(0.0)
                .truncate()
                .font(style)
                .color(ink("#71717a")),
        )
}

/// One action glyph with its hover tint stacked on top: `El::group_color`
/// only tints text/icons, not shapes, so the hover colour is a second shape
/// that fades in through the button's own hover group.
fn action_icon(
    size: f32,
    base: rune::Shape,
    hover: Option<rune::Shape>,
    shadow: Option<(f32, f32, rune::Color)>,
) -> El {
    let mut base_el = rune::el().size(size, size).shape(base);
    if let Some((y, blur, color)) = shadow {
        base_el = base_el.shadow(y, blur, color);
    }
    let mut stack = rune::el().size(size, size).inert().child(base_el);
    if let Some(hover) = hover {
        stack = stack.child(
            rune::el()
                .size(size, size)
                .absolute(0.0, 0.0)
                .group_opacity(0.0, 1.0)
                .shape(hover),
        );
    }
    stack
}

/// The hover action cluster (`HouseSidebar` `absolute right-1`): cloud status,
/// pin, two-step delete (check on the confirming click), rename. It fades in
/// with the row's hover group and floats over the time stamp.
fn sidebar_actions<S: CanvasHost + 'static>(
    card_id: &str,
    sid: &str,
    pinned: bool,
    confirming: bool,
    running: bool,
) -> El {
    let mut cluster = row()
        .key(format!("side-{card_id}-actions"))
        .pin(None, Some(2.0), Some(4.0), None)
        .w(108.0)
        .h(30.0)
        .items_center()
        .gap(2.0)
        .px(2.0)
        .radius(6.0)
        .fill(ink("#181818"))
        .border(1.0, rgba(255.0, 255.0, 255.0, 0.06))
        .border_layout()
        .group_opacity(0.0, 1.0);
    // `CloudBadge`: live runs carry the sky cloud (pulse + glow, sky-100 on
    // hover); idle shows zinc CloudOff, zinc-200 on hover. The badge is its
    // own hover group so its icon lights up only under the pointer, not when
    // The row's action cluster merely appears.
    let mut cloud_icon = action_icon(
        12.0,
        if running {
            crate::icons::cloud(12.0, ink("#e4e4e7"))
        } else {
            crate::icons::cloud_off(12.0, ink("#71717a"))
        },
        Some(if running {
            crate::icons::cloud(12.0, ink("#e0f2fe"))
        } else {
            crate::icons::cloud_off(12.0, ink("#e4e4e7"))
        }),
        running.then(|| (6.0, 6.0, rgba(125.0, 211.0, 252.0, 0.7))),
    );
    if running {
        cloud_icon = cloud_icon.looping(rune::LoopSpec::opacity(2.0, 0.0, 0.5, 1.0));
    }
    cluster = cluster.child(
        rune::el()
            .key(format!("side-{card_id}-cloud"))
            .role("button")
            .focusable(true)
            .label(if running {
                "Running — cloud run details"
            } else {
                "Cloud run details"
            })
            .size(24.0, 24.0)
            .radius(6.0)
            .center()
            .hover_group()
            .hover(if running {
                rgba(14.0, 165.0, 233.0, 0.20)
            } else {
                ink("#3f3f46")
            })
            // Cloud pages are out of scope for the chats-only shell; the
            // button still takes the press so it never opens the row.
            .on_press_enter(|_: &mut S| {})
            .child(cloud_icon),
    );
    let pin_sid = sid.to_string();
    let pin_icon = if pinned {
        action_icon(
            11.0,
            crate::icons::pin_off(11.0, ink("#e4e4e7")),
            None,
            None,
        )
    } else {
        action_icon(
            11.0,
            crate::icons::pin(11.0, ink("#71717a")),
            Some(crate::icons::pin(11.0, ink("#ffffff"))),
            None,
        )
    };
    cluster = cluster.child(
        rune::el()
            .key(format!("side-{card_id}-pin"))
            .role("button")
            .focusable(true)
            .label(if pinned { "Unpin" } else { "Pin" })
            .size(24.0, 24.0)
            .radius(6.0)
            .center()
            .hover_group()
            .hover(if pinned {
                rgba(255.0, 255.0, 255.0, 0.0)
            } else {
                rgba(255.0, 255.0, 255.0, 0.07)
            })
            .on_press_enter(move |s: &mut S| {
                commit_sidebar_rename(s);
                s.canvas_mut().toggle_pin(&pin_sid);
            })
            .child(pin_icon),
    );
    let delete_sid = sid.to_string();
    let delete_icon = if confirming {
        action_icon(
            11.0,
            crate::lucide_check(11.0, 2.0, ink("#f87171")),
            None,
            None,
        )
    } else {
        action_icon(
            11.0,
            crate::icons::trash(11.0, ink("#71717a")),
            Some(crate::icons::trash(11.0, ink("#ffffff"))),
            None,
        )
    };
    let mut delete = rune::el()
        .key(format!("side-{card_id}-delete"))
        .role("button")
        .focusable(true)
        .label(if confirming {
            "Click again to confirm"
        } else {
            "Delete"
        })
        .size(24.0, 24.0)
        .radius(6.0)
        .center()
        .hover_group()
        .hover(if confirming {
            rgba(239.0, 68.0, 68.0, 0.0)
        } else {
            rgba(255.0, 255.0, 255.0, 0.07)
        })
        .on_press_enter(move |s: &mut S| {
            commit_sidebar_rename(s);
            s.canvas_mut().press_delete(&delete_sid);
        });
    if confirming {
        delete = delete.fill(rgba(239.0, 68.0, 68.0, 0.15));
    }
    cluster = cluster.child(delete.child(delete_icon));
    let rename_sid = sid.to_string();
    cluster = cluster.child(
        rune::el()
            .key(format!("side-{card_id}-rename"))
            .role("button")
            .focusable(true)
            .label("Rename")
            .help("Rename")
            .size(24.0, 24.0)
            .radius(6.0)
            .center()
            .hover_group()
            .hover(rgba(255.0, 255.0, 255.0, 0.07))
            .on_press_enter(move |s: &mut S| {
                commit_sidebar_rename(s);
                s.canvas_mut().start_rename(&rename_sid);
            })
            .child(action_icon(
                11.0,
                crate::icons::pencil(11.0, ink("#71717a")),
                Some(crate::icons::pencil(11.0, ink("#ffffff"))),
                None,
            )),
    );
    cluster
}

/// The inline rename field (`HouseSidebar` row input): `rounded-md border
/// border-white/25 bg-black/40 px-2 py-1 text-[12px]`.
fn sidebar_rename_row<S: CanvasHost + 'static>(card_id: &str, edit: &rune::TextEdit) -> El {
    let mut style = rune::tok::TextStyle::new(12.0, 400.0);
    style.line_height = 18.0;
    row()
        .key(format!("side-{card_id}"))
        .w_full()
        .items_center()
        .pl(12.0)
        .pr(8.0)
        .py(8.0)
        .child(
            text_input(edit, style)
                .key(format!("side-{card_id}-rename-input"))
                .grow(1.0)
                .min_w(0.0)
                .h(28.0)
                .px(8.0)
                .py(4.0)
                .radius(6.0)
                .border(1.0, rgba(255.0, 255.0, 255.0, 0.25))
                .border_layout()
                .fill(rgba(0.0, 0.0, 0.0, 0.4))
                .caret(ink("#f2f2f4"), crate::motion::TEXT_SELECTION)
                .on_edit(move |s: &mut S| {
                    let canvas = s.canvas_mut();
                    match canvas.sessions.editing.as_mut() {
                        Some((_, edit)) => edit,
                        None => &mut canvas.sessions.rename_spare,
                    }
                })
                .on_submit(|s: &mut S, tx: &mut rune::Tx| {
                    commit_sidebar_rename(s);
                    tx.blur();
                })
                .on_input_key(|s: &mut S, key: &rune::KeyEvent, tx: &mut rune::Tx| {
                    if key.key != rune::Key::Escape {
                        return false;
                    }
                    s.canvas_mut().sessions.editing = None;
                    tx.blur();
                    true
                }),
        )
}

/// Title style for sidebar rows (`renderRow` 12.5pt title).
fn sidebar_title_style() -> rune::tok::TextStyle {
    let mut style = rune::tok::TextStyle::new(12.5, 400.0);
    style.line_height = 18.8;
    style
}

/// Timestamp style for sidebar rows (10pt stamp).
fn sidebar_stamp_style() -> rune::tok::TextStyle {
    let mut style = crate::motion::tabular(rune::tok::TextStyle::new(10.0, 400.0));
    style.line_height = 15.0;
    style
}

/// Plus glyph (Lucide, 11pt) for the per-project `+` buttons.
fn plus_icon(size: f32, color: rune::Color) -> Shape {
    Shape::new()
        .line(
            [12.0, 5.0],
            [12.0, 19.0],
            crate::lucide_stroke(size),
            LineCap::Round,
            color,
        )
        .line(
            [5.0, 12.0],
            [19.0, 12.0],
            crate::lucide_stroke(size),
            LineCap::Round,
            color,
        )
        .view_box(24.0, 24.0)
}

/// `from X` badge under a top-level spawned row (the reference UI `renderRow`'s
/// `auto` pill + `from {parentTitle}`): violet pill, 10pt zinc-600 label.
fn sidebar_from_badge(card_id: &str, parent_title: Option<&str>) -> El {
    let mut pill_style = rune::tok::TextStyle::new(9.0, 500.0);
    pill_style.line_height = 13.5;
    let mut label_style = rune::tok::TextStyle::new(10.0, 400.0);
    label_style.line_height = 15.0;
    row()
        .key(format!("side-{card_id}-from"))
        .w_full()
        .items_center()
        .gap(4.0)
        .child(
            text("auto".to_string())
                .key(format!("side-{card_id}-from-pill"))
                .font(pill_style)
                .color(rgba(196.0, 181.0, 253.0, 0.8))
                .px(6.0)
                // Pill text 13.5 + py 4 = 17.5 total, matching the reference UI's
                // 9pt text + `py-px` + 1pt border (`code-xml` badge).
                .py(2.0)
                .radius(8.0)
                .border(1.0, rgba(167.0, 139.0, 250.0, 0.2))
                .fill(rgba(167.0, 139.0, 250.0, 0.08)),
        )
        .child(
            text(match parent_title {
                Some(title) => format!("from {title}"),
                None => "spawned by agent".to_string(),
            })
            .key(format!("side-{card_id}-from-label"))
            .grow(1.0)
            .min_w(0.0)
            .truncate()
            .font(label_style)
            .color(ink("#52525b")),
        )
}

/// One top-level sidebar row plus its nested spawn leaves, in list order
/// (the reference UI `renderRow` + the nest group). `flat` is the search pose: no
/// nesting, spawned rows indent. `parent_title` is the resolved parent for
/// a spawned top-level row: `Some` renders `from X`, `None` renders
/// `spawned by agent` (the reference UI always badges spawned tops).
#[allow(clippy::too_many_lines)]
fn sidebar_thread<S: CanvasHost + 'static>(
    s: &CanvasState,
    chat: &crate::chats::ChatSummary,
    kids: &[&crate::chats::ChatSummary],
    flat: bool,
    parent_title: Option<String>,
) -> Vec<El> {
    let card_id = chat.card_id();
    let chat_key = chat.key();
    let title = crate::chats::display_title(&chat.title);
    let spawned = chat.parent_chat_id.is_some();
    let active = card_id == s.active_id;
    let sid = chat_key.clone();
    let pinned = s.is_pinned(&sid);
    let confirming = s.delete_confirming(&sid);
    let progress = s.delete_progress(&sid);
    let deleting = progress.is_some();
    let running = s.running_tab.as_deref() == Some(sid.as_str())
        || s.card(&card_id).map(|c| c.running).unwrap_or(false);
    let editing = s
        .sessions
        .editing
        .as_ref()
        .is_some_and(|(id, _)| id == &sid);
    let ws = chat.workspace_id.clone();
    let chat_id = chat.id.clone();
    let hover_offset = s.hover.offset(format!("side-{card_id}-offset"), [0.0, 0.0]);
    let mut item = if editing {
        let edit = &s
            .sessions
            .editing
            .as_ref()
            .expect("editing implies a draft")
            .1;
        sidebar_rename_row::<S>(&card_id, edit)
    } else {
        let mut item = row()
            .key(format!("side-{card_id}"))
            .w_full()
            .transition(crate::motion::sidebar_row_enter())
            .items_center()
            .gap(8.0)
            .pl(if spawned && flat { 26.0 } else { 12.0 })
            .pr(8.0)
            .py(8.0)
            .radius(8.0)
            .hover_group();
        if !deleting {
            // `.chat-row:hover { transform: translateX(1px) }`.
            let set_offset = hover_offset.clone();
            item = item
                .live_offset(hover_offset.smooth(crate::motion::HOVER_SPRING))
                .on_hover_change(move |_s: &mut S, inside, tx| {
                    set_offset.set(if inside { [1.0, 0.0] } else { [0.0, 0.0] });
                    tx.live_only();
                });
        }
        // The reference UI row states: spawned rows carry `is-spawned`
        // (white/2.5%, hover 5%, active 7.5%); regular rows hover
        // white/4% and active at 9%.
        if spawned {
            item = item.fill(rgba(
                255.0,
                255.0,
                255.0,
                if active { 0.075 } else { 0.025 },
            ));
            if !active && !deleting {
                item = item.hover(rgba(255.0, 255.0, 255.0, 0.05));
            }
        } else {
            if !active && !deleting {
                item = item.hover(rgba(255.0, 255.0, 255.0, 0.04));
            }
            if active {
                item = item.fill(rgba(255.0, 255.0, 255.0, 0.09));
            }
        }
        if !deleting {
            item = item
                .role("button")
                .focusable(true)
                .label(format!("Open {title}"))
                .on_press_enter(move |s: &mut S| {
                    commit_sidebar_rename(s);
                    s.canvas_mut().open_chat_row(&ws, &chat_id);
                });
        }
        if running {
            // `session-run-dot`: 6pt white dot with an 8pt glow and
            // `house-breathe` (1.6s, opacity 0.55 → 1).
            item = item.child(
                rune::el()
                    .key(format!("side-{card_id}-dot"))
                    .size(6.0, 6.0)
                    .radius(3.0)
                    .fill(ink("#fafafa"))
                    .shadow(0.0, 8.0, rgba(255.0, 255.0, 255.0, 0.65))
                    .looping(rune::LoopSpec::opacity(1.6, 0.0, 0.55, 1.0)),
            );
        } else {
            item = item.child(
                rune::el()
                    .key(format!("side-{card_id}-icon"))
                    .size(12.0, 12.0)
                    .inert()
                    .shape(if spawned {
                        bot_icon(12.0, ink("#a1a1aa"))
                    } else if chat.workspace_kind == "general" {
                        chat_row_icon(12.0, ink("#52525b"))
                    } else {
                        // The reference UI `renderRow`: non-spawned rows use the chat
                        // bubble only on the General workspace, `</>` elsewhere.
                        code_row_icon(12.0, ink("#52525b"))
                    }),
            );
        }
        let mut title_col = col()
            .key(format!("side-{card_id}-titlecol"))
            .grow(1.0)
            .min_w(0.0)
            .child(
                text(title.clone())
                    .key(format!("side-{card_id}-title"))
                    .min_w(0.0)
                    .truncate()
                    .help_when_truncated(title.clone())
                    .font(sidebar_title_style())
                    .color(ink(if active { "#ffffff" } else { "#a1a1aa" })),
            );
        if spawned && !deleting {
            let mut badge = sidebar_from_badge(&card_id, parent_title.as_deref());
            // The reference UI `mt-0.5` (2pt) between the title and the badge.
            badge = badge.mt(2.0);
            title_col = title_col.child(badge);
        }
        item = item.child(title_col);
        if pinned {
            item = item.child(
                rune::el()
                    .key(format!("side-{card_id}-pinmark"))
                    .size(10.0, 10.0)
                    .inert()
                    .rotate(std::f32::consts::FRAC_PI_4)
                    .shape(crate::icons::pin(10.0, ink("#71717a"))),
            );
        }
        let stamp = crate::chats::relative_time_short(&chat.updated_at);
        if !stamp.is_empty() {
            item = item.child(
                text(stamp)
                    .key(format!("side-{card_id}-time"))
                    .font(sidebar_stamp_style())
                    .color(ink("#52525b")),
            );
        }
        if !deleting {
            item = item.child(sidebar_actions::<S>(
                &card_id, &sid, pinned, confirming, running,
            ));
        }
        item
    };
    // An inline input's own press must edit the field, not open the
    // chat; the row wrapper carries only layout.
    if editing {
        item = item.on_press(|_: &mut S| {});
    }
    // The reference UI seeds every parent with spawned children as collapsed;
    // an explicit expand keeps it open across polls.
    let expanded = !kids.is_empty() && !s.sessions.spawn_collapsed.contains(&chat_key);
    // `flex items-center gap-0.5` + 20pt leading slot (`w-5`): the
    // role/expand gutter. Root rows leave it empty, so the row box
    // starts at x=81 and spans 205pt (the reference UI `house-sidebar-row`).
    let lead = if kids.is_empty() {
        rune::el()
            .key(format!("side-{card_id}-lead"))
            .w(20.0)
            .h(20.0)
    } else {
        let toggle = chat_key.clone();
        let open = expanded;
        rune::el()
            .key(format!("side-{card_id}-lead"))
            .role("button")
            .focusable(true)
            .label(if open {
                "Collapse nested chats"
            } else {
                "Expand spawned chats"
            })
            .size(20.0, 20.0)
            .radius(6.0)
            .center()
            .on_press_enter(move |s: &mut S| {
                commit_sidebar_rename(s);
                let sessions = &mut s.canvas_mut().sessions;
                if sessions.spawn_collapsed.remove(&toggle) {
                    sessions.spawn_expanded.insert(toggle.clone());
                } else {
                    sessions.spawn_collapsed.insert(toggle.clone());
                    sessions.spawn_expanded.remove(&toggle);
                }
            })
            .child(rune::el().size(11.0, 11.0).inert().shape(if open {
                chevron_down(11.0, ink("#52525b"))
            } else {
                chevron_right(11.0, ink("#52525b"))
            }))
    };
    let cell = col()
        .key(format!("side-{card_id}-cell"))
        .grow(1.0)
        .min_w(0.0)
        .child(item);
    let mut wrap = row()
        .key(format!("side-wrap-{card_id}"))
        .w_full()
        .items_center()
        .gap(2.0);
    if let Some(t) = progress {
        // The reference UI `.is-deleting` (`house-row-out`): 220 ms, eased with
        // `cubic-bezier(.4,0,.2,1)`, `max-height:40px -> 0` with the margin
        // gone, opacity to 0 and `translateX(-8px)`. The row is 34.8 pt
        // (spawned tops cap at 40), so the slot shrinks on the same clock.
        // The content is absolute during the animation: its 20 pt lead is a
        // flex min-content floor, and moving it out of flow lets the wrapper
        // reach zero. `motion(NONE)` lands each frame's height with the
        // paint instead of lagging behind on a layout spring.
        let base = if spawned { 40.0 } else { 34.8 };
        let target_h = (base * (1.0 - t)).max(0.0);
        let body = row()
            .w_full()
            .items_center()
            .gap(2.0)
            .child(lead)
            .child(cell)
            .absolute(0.0, 0.0)
            .inert();
        wrap = wrap
            .h(target_h)
            .max_h(target_h)
            .min_h(0.0)
            .clip_rounded(8.0)
            .mb(-6.0 * t)
            .opacity(1.0 - t)
            .offset(-8.0 * t, 0.0)
            .motion(rune::Spring::NONE)
            .child(body);
    } else {
        wrap = wrap.child(lead).child(cell);
    }
    let mut out = vec![wrap];
    if expanded {
        // The reference UI nests kids in a `flex flex-col gap-0.5` group directly under
        // The parent row: no gap to the first kid, 2pt between kids, and the
        // parent's own `mb-0.5` (2pt) after the last one. The flat list gap
        // (6) would triple-space the nest, so the group pulls its bottom
        // back up. (When an expanded nest ends a project group the group
        // tail reads 6 instead of 12; expanded nests are interactive states,
        // and the rest/collapsed/Load-more poses stay exact.)
        let mut nest = col().key(format!("side-nest-{card_id}")).w_full().gap(0.0);
        for (i, kid) in kids.iter().enumerate() {
            let mut kid_wrap = row()
                .key(format!("side-kid-wrap-{}", kid.card_id()))
                .w_full()
                .items_center()
                .gap(2.0)
                .child(
                    rune::el()
                        .key(format!("side-kid-{}-lead", kid.card_id()))
                        .w(20.0)
                        .h(20.0),
                )
                .child(
                    col()
                        .key(format!("side-kid-{}-cell", kid.card_id()))
                        .grow(1.0)
                        .min_w(0.0)
                        .child(sidebar_spawn_row::<S>(kid, &s.hover)),
                );
            if i > 0 {
                kid_wrap = kid_wrap.mt(2.0);
            }
            nest = nest.child(kid_wrap);
        }
        nest = nest.mb(-4.0);
        out.push(nest);
    }
    out
}

/// Split one project workspace's chats into top-level rows + same-workspace
/// kid nests (the reference UI `groupWorkspaceChats`). Only parents inside the group
/// nest; cross-workspace children stay top-level with a `from X` badge.
/// Kids sort oldest-first (the reference UI `childrenByParent` sort).
fn coding_group<'a>(
    chats: &'a [crate::chats::ChatSummary],
    ws_id: &str,
) -> (
    Vec<&'a crate::chats::ChatSummary>,
    std::collections::HashMap<&'a str, Vec<&'a crate::chats::ChatSummary>>,
) {
    let group_ids: std::collections::HashSet<&str> = chats
        .iter()
        .filter(|c| c.workspace_id == ws_id)
        .map(|c| c.id.as_str())
        .collect();
    let mut kids: std::collections::HashMap<&str, Vec<&crate::chats::ChatSummary>> =
        std::collections::HashMap::new();
    for chat in chats.iter().filter(|c| c.workspace_id == ws_id) {
        if let Some(parent) = chat.parent_chat_id.as_deref() {
            if parent != chat.id && group_ids.contains(parent) {
                kids.entry(parent).or_default().push(chat);
            }
        }
    }
    for arr in kids.values_mut() {
        arr.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    }
    let tops: Vec<&crate::chats::ChatSummary> = chats
        .iter()
        .filter(|c| {
            c.workspace_id == ws_id
                && c.parent_chat_id
                    .as_deref()
                    .is_none_or(|pid| pid == c.id || !group_ids.contains(pid))
        })
        .collect();
    (tops, kids)
}

/// Coding project group header (the reference UI `codingGroups` heading): chevron
/// collapse toggle with the 10pt uppercase project name + loaded chat
/// count, and a 20pt `+` button starting a chat in that project. Missing
/// project folders render the name amber (`text-amber-500/70`).
fn sidebar_group_header<S: CanvasHost + 'static>(
    s: &CanvasState,
    ws: &crate::chats::ChatWorkspace,
    total: usize,
) -> El {
    let ws_id = ws.id.clone();
    let ws_name = ws.name.clone();
    let collapsed = s.sessions.collapsed_ws.contains(&ws_id);
    let mut name_style = rune::tok::TextStyle::new(10.0, 600.0);
    name_style.line_height = 15.0;
    name_style.tracking = 1.0;
    let mut count_style = crate::motion::tabular(rune::tok::TextStyle::new(9.5, 400.0));
    count_style.line_height = 14.25;
    let name_color = if ws.exists {
        ink("#52525b")
    } else {
        rgba(245.0, 158.0, 11.0, 0.7)
    };
    let toggle_id = ws_id.clone();
    let plus_id = ws_id.clone();
    let chevron_scale = s.hover.scale(format!("side-group-{ws_id}-chevron"), 1.0);
    row()
        .key(format!("side-group-{ws_id}"))
        .w_full()
        .items_center()
        .gap(6.0)
        .px(4.0)
        .pb(4.0)
        .child(
            row()
                .key(format!("side-group-{ws_id}-toggle"))
                .grow(1.0)
                .min_w(0.0)
                .items_center()
                .gap(6.0)
                .role("button")
                .focusable(true)
                .label(if collapsed {
                    format!("Expand {ws_name}")
                } else {
                    format!("Collapse {ws_name}")
                })
                // The reference UI `hover:bg-zinc-800/50` + `hover:text-zinc-200`.
                // The rest fill is transparent, so the hover overlay is the
                // exact CSS wash; `group_color` lifts the label.
                .hover(rgba(39.0, 39.0, 42.0, 0.5))
                .hover_group()
                .on_hover_change({
                    let scale = chevron_scale.clone();
                    move |_s: &mut S, inside, tx| {
                        scale.set(if inside { 1.08 } else { 1.0 });
                        tx.live_only();
                    }
                })
                .on_press_enter(move |s: &mut S| {
                    commit_sidebar_rename(s);
                    let sessions = &mut s.canvas_mut().sessions;
                    if !sessions.collapsed_ws.remove(&toggle_id) {
                        sessions.collapsed_ws.insert(toggle_id.clone());
                    }
                })
                .child(
                    rune::el()
                        .key(format!("side-group-{ws_id}-chevron"))
                        .size(11.0, 11.0)
                        .inert()
                        .rotate(if collapsed {
                            -std::f32::consts::FRAC_PI_2
                        } else {
                            0.0
                        })
                        .rotate_motion(crate::motion::ROTATE)
                        .live_scale(chevron_scale.smooth(crate::motion::HOVER_SPRING))
                        .shape(chevron_down(11.0, ink("#52525b"))),
                )
                .child({
                    let mut name = text(ws_name.to_uppercase())
                        .key(format!("side-group-{ws_id}-name"))
                        .min_w(0.0)
                        .truncate()
                        .font(name_style)
                        .color(name_color);
                    if ws.exists {
                        name = name.group_color(ink("#e4e4e7"));
                    }
                    name
                })
                .child(
                    text(total.to_string())
                        .key(format!("side-group-{ws_id}-count"))
                        .font(count_style)
                        .color(ink("#3f3f46")),
                ),
        )
        .child(
            rune::el()
                .key(format!("side-group-{ws_id}-plus"))
                .role("button")
                .focusable(true)
                .label(format!("New chat in {ws_name}"))
                .help("New chat")
                .size(20.0, 20.0)
                .radius(6.0)
                .center()
                .hover_group()
                .hover(rgba(255.0, 255.0, 255.0, 0.08))
                .on_press_enter(move |s: &mut S| {
                    commit_sidebar_rename(s);
                    s.canvas_mut().new_session_in(&plus_id);
                })
                .child(
                    rune::el()
                        .key(format!("side-group-{ws_id}-plus-icon"))
                        .size(11.0, 11.0)
                        .inert()
                        .child(
                            rune::el()
                                .size(11.0, 11.0)
                                .shape(plus_icon(11.0, ink("#52525b"))),
                        )
                        .child(
                            rune::el()
                                .size(11.0, 11.0)
                                .absolute(0.0, 0.0)
                                .group_opacity(0.0, 1.0)
                                .shape(plus_icon(11.0, ink("#ffffff"))),
                        ),
                ),
        )
}

/// Load more button closing a paged sidebar section
/// (`LoadMoreButton`: 11pt/500 zinc-500, zinc-200 + white wash on hover).
fn sidebar_load_more<S: CanvasHost + 'static>(
    remaining: usize,
    project: Option<String>,
    general: bool,
) -> El {
    let mut style = rune::tok::TextStyle::new(11.0, 500.0);
    style.line_height = 16.5;
    let mut item = row()
        .key(format!(
            "side-loadmore-{}",
            project.as_deref().unwrap_or("general")
        ))
        .w_full()
        .center()
        .py(6.0)
        .mb(6.0)
        // The list gap already separates the button from the rows above
        // (the reference UI: row `mb-1` + wrapper `mb-0.5` = 6).
        .radius(8.0)
        .role("button")
        .focusable(true)
        .label(format!("Load {remaining} more chats"))
        .hover_group()
        .hover(rgba(255.0, 255.0, 255.0, 0.04))
        .child(
            rune::el()
                .key(format!(
                    "side-loadmore-{}-label",
                    project.as_deref().unwrap_or("general")
                ))
                .inert()
                .child(
                    text(format!("Load more ({remaining})"))
                        .font(style)
                        .color(ink("#71717a")),
                )
                .child(
                    text(format!("Load more ({remaining})"))
                        .absolute(0.0, 0.0)
                        .font(style)
                        .color(ink("#e4e4e7"))
                        .group_opacity(0.0, 1.0),
                ),
        );
    if general {
        item = item.on_press_enter(|s: &mut S| {
            commit_sidebar_rename(s);
            s.canvas_mut().sessions.general_limit += GENERAL_PAGE;
        });
    } else if let Some(ws_id) = project {
        item = item.on_press_enter(move |s: &mut S| {
            commit_sidebar_rename(s);
            let sessions = &mut s.canvas_mut().sessions;
            let limit = sessions
                .project_limits
                .get(&ws_id)
                .copied()
                .unwrap_or(PROJECT_PAGE);
            sessions
                .project_limits
                .insert(ws_id.clone(), limit + PROJECT_PAGE);
        });
    }
    item
}

/// Chats panel: kind dropdown + new chat, live search, and one row per chat
/// filtered by kind + query. A row press selects and focuses the card.
pub fn chats_panel<S: CanvasHost + 'static>(s: &CanvasState) -> El {
    // Header: workspace switcher + new chat (`HouseSidebar` px-3.5 pt-4 pb-2).
    let head = col()
        .key("map-head")
        .w_full()
        .px(14.0)
        .pt(16.0)
        .pb(8.0)
        .child(
            row()
                .key("map-kindrow")
                .w_full()
                .items_center()
                .gap(6.0)
                .child(
                    row()
                        .key("map-kind")
                        .grow(1.0)
                        .min_w(0.0)
                        .items_center()
                        .gap(6.0)
                        .px(8.0)
                        .py(6.0)
                        .radius(8.0)
                        .role("button")
                        .focusable(true)
                        .label("Switch mode")
                        .help("Switch mode")
                        .on_press_enter(move |s: &mut S| {
                            let c = s.canvas_mut();
                            c.kind_open = !c.kind_open;
                        })
                        .child(
                            text(s.sessions.kind.clone())
                                .truncate()
                                .font({
                                    // `text-[15px]` at the reference 1.5 line-height:
                                    // The switcher is 34.5pt with py-1.5.
                                    let mut style = rune::tok::TextStyle::new(15.0, 600.0);
                                    style.line_height = 22.5;
                                    // The reference UI `tracking-tight`: -0.025em at 15pt.
                                    style.tracking = -0.375;
                                    style
                                })
                                .color(ink("#fafafa")),
                        )
                        .child(
                            rune::el()
                                .key("map-kind-chevron")
                                .size(14.0, 14.0)
                                .inert()
                                .shape(chevron_down(14.0, ink("#71717a"))),
                        ),
                )
                .child(
                    rune::el()
                        .key("map-new")
                        .role("button")
                        .focusable(true)
                        .label("New chat")
                        .help("New chat")
                        .size(24.0, 24.0)
                        .radius(6.0)
                        .center()
                        .on_press_enter(move |s: &mut S| {
                            s.canvas_mut().new_session();
                        })
                        .child(
                            rune::el()
                                .key("map-new-icon")
                                .size(13.0, 13.0)
                                .inert()
                                .shape({
                                    let color = ink("#71717a");
                                    rune::Shape::new()
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
                                }),
                        ),
                ),
        );
    // Floating menu under the switcher (the reference UI `absolute top-[calc(100%+6px)]`).
    // Built separately and mounted after the list so it paints (and hits) on top.
    let mut kind_menu: Option<El> = None;
    if s.kind_open {
        let mut menu = col()
            .key("map-kind-menu")
            .absolute(14.0, 52.0)
            .w(220.0)
            .transition(crate::motion::menu_enter())
            .radius(12.0)
            .border(1.0, ink("#ffffff17"))
            .fill(ink("#151515"))
            .clip_rounded(12.0);
        for (i, kind) in CHAT_KINDS.iter().enumerate() {
            let current = s.sessions.kind == *kind;
            let target = kind.to_string();
            let desc = [
                "Create, learn, and explore",
                "Build, debug, and ship",
                "Manage home, tasks, and routines",
                "Life admin, research, and records",
            ][i];
            menu = menu.child(
                col()
                    .key(format!("map-kind-{kind}"))
                    .w_full()
                    .px(12.0)
                    .py(10.0)
                    .gap(1.0)
                    .fill(ink(if current { "#ffffff0d" } else { "#00000000" }))
                    .role("button")
                    .focusable(true)
                    .label(format!("Switch to {kind}"))
                    .hover(ink("#ffffff0a"))
                    .hover_group()
                    .on_press_enter(move |s: &mut S| {
                        let c = s.canvas_mut();
                        c.sessions.kind = target.clone();
                        c.kind_open = false;
                    })
                    .child(
                        text(kind.to_string())
                            .font(rune::tok::TextStyle::new(13.0, 600.0))
                            .color(ink(if current { "#ffffff" } else { "#e4e4e7" })),
                    )
                    .child(
                        text(desc.to_string())
                            .font(rune::tok::TextStyle::new(11.0, 400.0))
                            .color(ink("#71717a")),
                    ),
            );
        }
        kind_menu = Some(menu);
    }
    let mut panel = col()
        .key("map-sidebar")
        .w_full()
        .h_full()
        .fill(ink("#0f0f0f"))
        .child(head)
        // Search: `mx-3.5 mb-2.5 px-2 py-1 rounded-md bg-white/[0.04]`
        // (27.3pt tall at the reference UI's 11.5pt input). Rune draws its hairline
        // inside the box, so py-1 is py 5 to keep the same outer height.
        .child(
            row()
                .key("map-search-wrap")
                .mx(14.0)
                .mb(10.0)
                .py(4.0)
                .items_center()
                .gap(6.0)
                .px(8.0)
                .radius(6.0)
                .fill(ink("#ffffff0a"))
                .border(1.0, ink("#ffffff0f"))
                .border_layout()
                .child(
                    rune::el()
                        .key("map-search-icon")
                        .size(11.0, 11.0)
                        .inert()
                        .shape(search_icon(ink("#52525b"))),
                )
                .child(
                    text_input(&s.sessions.search, rune::tok::TextStyle::new(11.5, 400.0))
                        .key("map-search")
                        .grow(1.0)
                        .h(17.3)
                        .placeholder("Search chats…", ink("#52525b"))
                        .caret(ink("#f2f2f4"), crate::motion::TEXT_SELECTION)
                        .on_edit(move |s: &mut S| &mut s.canvas_mut().sessions.search),
                ),
        );
    // List: one row per chat (`px-2.5 pb-3.5`, rows `pl-3 pr-2 py-2 mb-1`).
    // The horizontal inset lives on a wrapper outside the scroll view: a
    // Rune scroll's content width ignores its own right padding, which let
    // rows overlap the panel edge and clipped the time stamps.
    let mut list = col()
        .key("map-chat-list")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        })
        .pb(14.0)
        .gap(6.0);
    if s.sessions.deleting.is_some() {
        // The deleting row's own height lands per frame; the rows below it
        // must land on the same frame. The list's layout spring would
        // otherwise chase the collapse and leave a growing gap.
        list = list.motion(rune::Spring::NONE);
    }
    let query = s.sessions.search.text.trim().to_lowercase();
    if !s.chat_rows.is_empty() {
        // Live sidebar (`HouseSidebar`): every chat on the server, newest
        // first; clicking one opens (or selects) its tab. Spawned chats nest
        // under their parent (collapsed by default, chevron expands); search
        // flattens so children stay findable.
        let all_keys: std::collections::HashSet<String> =
            s.chat_rows.iter().map(|c| c.key()).collect();
        let mut spawn_kids: std::collections::HashMap<String, Vec<&crate::chats::ChatSummary>> =
            std::collections::HashMap::new();
        for chat in &s.chat_rows {
            let Some(parent) = &chat.parent_chat_id else {
                continue;
            };
            let pws = chat
                .parent_workspace_id
                .as_deref()
                .unwrap_or(&chat.workspace_id);
            let parent_key = format!("{pws}:{parent}");
            if parent_key != chat.key() && all_keys.contains(&parent_key) {
                spawn_kids.entry(parent_key).or_default().push(chat);
            }
        }
        let flat = !query.is_empty();
        // The reference UI `pinnedSort`: pinned chats first, otherwise the server's
        // newest-first order (Rust's sort is stable).
        let mut ordered: Vec<&crate::chats::ChatSummary> = s.chat_rows.iter().collect();
        ordered.sort_by_key(|chat| !s.is_pinned(&chat.key()));
        let lookup = crate::chats::parent_lookup(&s.chat_rows);
        if s.sessions.kind == "Coding" && !flat {
            // Coding filter: collapsible per-project groups (the reference UI
            // `codingGroups`), each with its own 7-row page. Same-workspace
            // spawns nest under their parent; cross-workspace children stay
            // top-level with a `from X` badge. Search stays flat above.
            let groups: Vec<&crate::chats::ChatWorkspace> =
                s.workspaces.iter().filter(|w| w.is_project()).collect();
            if groups.is_empty() {
                let mut empty_style = rune::tok::TextStyle::new(11.5, 400.0);
                empty_style.line_height = 17.25;
                list = list.child(
                    text("No projects yet - use + to add one.".to_string())
                        .key("map-no-projects")
                        .px(12.0)
                        .py(12.0)
                        .font(empty_style)
                        .color(ink("#52525b")),
                );
            }
            for (gi, ws) in groups.iter().enumerate() {
                let total = s
                    .chat_rows
                    .iter()
                    .filter(|c| c.workspace_id == ws.id)
                    .count();
                let mut header = sidebar_group_header::<S>(s, ws, total);
                if gi > 0 {
                    // List gap (6) + header margin (4) = the reference UI `mb-2.5` (10).
                    header = header.mt(4.0);
                }
                if !s.sessions.collapsed_ws.contains(&ws.id) {
                    // The expanded header's own bottom margin cancels the
                    // list gap: header `pb-1` (4) then the first row, with
                    // no extra gap (the reference UI stacks them in a plain div).
                    // Collapsed headers keep the margin for the 10pt group
                    // rhythm.
                    header = header.mb(-6.0);
                }
                list = list.child(header);
                if s.sessions.collapsed_ws.contains(&ws.id) {
                    continue;
                }
                let (mut tops, group_kids) = coding_group(&s.chat_rows, &ws.id);
                // The reference UI `pinnedSort`: pinned first, else the server's
                // newest-first order (Rust's sort is stable).
                tops.sort_by_key(|chat| !s.is_pinned(&chat.key()));
                if tops.is_empty() {
                    let mut empty_style = rune::tok::TextStyle::new(11.0, 400.0);
                    empty_style.line_height = 16.5;
                    list = list.child(
                        text("No chats yet".to_string())
                            .key(format!("side-group-{}-empty", ws.id))
                            .px(12.0)
                            .py(6.0)
                            .font(empty_style)
                            .color(ink("#3f3f46")),
                    );
                    continue;
                }
                let limit = s
                    .sessions
                    .project_limits
                    .get(&ws.id)
                    .copied()
                    .unwrap_or(PROJECT_PAGE);
                let shown = tops.len().min(limit);
                for chat in &tops[..shown] {
                    let kids: Vec<&crate::chats::ChatSummary> = group_kids
                        .get(chat.id.as_str())
                        .map(|kids| {
                            kids.iter()
                                .filter(|kid| {
                                    CanvasState::kind_for(&kid.workspace_kind) == s.sessions.kind
                                })
                                .copied()
                                .collect()
                        })
                        .unwrap_or_default();
                    let parent_title = chat
                        .parent_chat_id
                        .as_ref()
                        .and_then(|pid| lookup.get(pid))
                        .map(|(title, _)| title.clone());
                    for el in sidebar_thread::<S>(s, chat, &kids, flat, parent_title) {
                        list = list.child(el);
                    }
                }
                if tops.len() > shown {
                    list = list.child(sidebar_load_more::<S>(
                        tops.len() - shown,
                        Some(ws.id.clone()),
                        false,
                    ));
                }
            }
        } else {
            // Flat list (the reference UI `pinnedSort` + per-section paging): pinned
            // chats first, otherwise the server's newest-first order (Rust's
            // sort is stable). The Main filter pages at 15 rows.
            let mut filt: Vec<&crate::chats::ChatSummary> = s.chat_rows.iter().collect();
            filt.sort_by_key(|chat| !s.is_pinned(&chat.key()));
            let mut tops: Vec<&crate::chats::ChatSummary> = Vec::new();
            for chat in filt {
                let title = crate::chats::display_title(&chat.title);
                let kind = CanvasState::kind_for(&chat.workspace_kind);
                if kind != s.sessions.kind {
                    continue;
                }
                if !query.is_empty() && !title.to_lowercase().contains(&query) {
                    continue;
                }
                let parent_key = chat.parent_chat_id.as_ref().map(|parent| {
                    let pws = chat
                        .parent_workspace_id
                        .as_deref()
                        .unwrap_or(&chat.workspace_id);
                    format!("{pws}:{parent}")
                });
                if !flat && parent_key.is_some_and(|pk| all_keys.contains(&pk)) {
                    continue;
                }
                tops.push(chat);
            }
            let total = tops.len();
            let shown = if flat || s.sessions.kind != "Main" {
                total
            } else {
                total.min(s.sessions.general_limit)
            };
            for chat in &tops[..shown] {
                let chat_key = chat.key();
                let mut kids: Vec<&crate::chats::ChatSummary> = if flat {
                    Vec::new()
                } else {
                    spawn_kids
                        .get(&chat_key)
                        .map(|kids| {
                            kids.iter()
                                .filter(|kid| {
                                    CanvasState::kind_for(&kid.workspace_kind) == s.sessions.kind
                                })
                                .copied()
                                .collect()
                        })
                        .unwrap_or_default()
                };
                kids.sort_by(|a, b| a.created_at.cmp(&b.created_at));
                let parent_title = chat
                    .parent_chat_id
                    .as_ref()
                    .and_then(|pid| lookup.get(pid))
                    .map(|(title, _)| title.clone());
                for el in sidebar_thread::<S>(s, chat, &kids, flat, parent_title) {
                    list = list.child(el);
                }
            }
            if !flat && total > shown {
                list = list.child(sidebar_load_more::<S>(total - shown, None, true));
            }
        }
        panel = panel.child(
            col()
                .key("map-list-wrap")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .px(10.0)
                .child(list),
        );
        if let Some(menu) = kind_menu {
            panel = panel.child(menu);
        }
        return panel;
    }
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
            .pl(12.0)
            .pr(8.0)
            .py(8.0)
            .radius(8.0)
            .role("button")
            .focusable(true)
            .label(format!("Open {}", card.title))
            .on_press_enter(move |s: &mut S| {
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
        let stamp = s
            .sessions
            .threads
            .get(&card.id)
            .and_then(|t| t.rows.last())
            .map(|r| crate::chats::relative_time_short(&r.at))
            .unwrap_or_default();
        if card.running {
            item = item.child(
                rune::el()
                    .key(format!("side-{}-dot", card.id))
                    .size(6.0, 6.0)
                    .radius(3.0)
                    .fill(ink("#fafafa")),
            );
        } else {
            item = item.child(
                rune::el()
                    .key(format!("side-{}-icon", card.id))
                    .size(12.0, 12.0)
                    .inert()
                    .shape(chat_row_icon(12.0, ink("#52525b"))),
            );
        }
        item = item.child(
            text(card.title.clone())
                .key(format!("side-{}-title", card.id))
                .grow(1.0)
                .min_w(0.0)
                .truncate()
                .font(rune::tok::TextStyle::new(12.5, 400.0))
                .color(ink(if active { "#ffffff" } else { "#a1a1aa" })),
        );
        if !stamp.is_empty() {
            item = item.child(
                text(stamp)
                    .key(format!("side-{}-time", card.id))
                    .font(rune::tok::TextStyle::new(10.0, 400.0))
                    .color(ink("#52525b")),
            );
        }
        list = list.child(item);
    }
    if s.real && s.chat_rows.is_empty() && s.cards.is_empty() {
        // The reference UI `HouseSidebar`: the empty list says so instead of leaving a
        // blank panel under the search field.
        let mut style = rune::tok::TextStyle::new(11.0, 400.0);
        style.line_height = 16.5;
        list = list.child(
            text("No chats yet.".to_string())
                .key("map-no-chats")
                .px(12.0)
                .py(6.0)
                .font(style)
                .color(ink("#52525b")),
        );
    }
    panel = panel.child(list);
    if let Some(menu) = kind_menu {
        panel = panel.child(menu);
    }
    panel
}

fn init_map() -> CanvasState {
    CanvasState::map()
}

pub fn canvas_spec() -> AppSpec<CanvasState> {
    AppSpec {
        fonts: &[include_bytes!("../assets/InterVariable.ttf")],
        families: ("Inter", "ui-monospace"),
        setup: Some(crate::image_cache::setup::<CanvasState>),
        ..AppSpec::new("Stark Canvas", (MAP_W, MAP_H), init_map, canvas_view)
    }
}

pub fn run_canvas_headless(extra_args: Vec<String>) {
    rune::App::new(canvas_spec()).args(extra_args).run();
}

#[cfg(test)]
mod tests {
    #[test]
    fn spawned_card_history_lands_before_it_renders() {
        use super::*;
        let mut canvas = CanvasState::map();
        // A non-tab card: spawned children are on the map without a thread.
        let card_id = "chat:ws-general:chat-2".to_string();
        assert!(canvas.card(&card_id).is_some());
        canvas.sessions.threads.remove(&card_id);
        let rows = vec![crate::chats::ChatMsgRow {
            key: "m1".to_string(),
            role: "agent".to_string(),
            body: "child reply".to_string(),
            raw_body: "child reply".to_string(),
            ..crate::chats::ChatMsgRow::default()
        }];
        assert!(canvas.apply_chat_messages("ws-general:chat-2", &rows));
        let thread = canvas
            .sessions
            .threads
            .get(&card_id)
            .expect("thread created for the card");
        assert_eq!(thread.rows.len(), 1);
        assert_eq!(thread.rows[0].body, "child reply");
        assert!(canvas.hydrated_chats.contains("ws-general:chat-2"));
    }

    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn card_dock_attachments_are_per_card_and_capped() {
        let mut s = CanvasState::map();
        let card = s.cards[0].id.clone();
        let other = s.cards[1].id.clone();
        let dir = std::env::temp_dir().join(format!(
            "stark-card-files-{}-{}",
            std::process::id(),
            crate::clock::now_millis()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let paths: Vec<std::path::PathBuf> = (0..9)
            .map(|index| {
                let path = dir.join(format!("file-{index}.txt"));
                std::fs::write(&path, b"x").unwrap();
                path
            })
            .collect();
        s.add_card_attachments(&card, &paths);
        assert_eq!(s.card_attachments(&card).len(), 8, "eight-file cap");
        assert!(
            s.card_attachment_errors
                .get(&card)
                .is_some_and(|error| error.contains("up to 8")),
            "amber cap message"
        );
        assert!(s.card_attachments(&other).is_empty(), "per-card draft");
        assert!(
            s.sessions.threads[&card].draft_attachments.is_empty(),
            "map draft must not leak into the full chat dock"
        );
        s.remove_card_attachment(&card, "local:0");
        assert_eq!(s.card_attachments(&card).len(), 7);
        assert!(
            !s.card_attachment_errors.contains_key(&card),
            "clearing under the cap drops the message"
        );
        s.clear_card_draft(&card);
        assert!(s.card_attachments(&card).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mini_bubble_actions_copy_and_arm_rewind() {
        let mut s = CanvasState::map();
        let card = s.cards[0].id.clone();
        {
            let thread = s.thread_mut(&card);
            thread.live = false;
            thread.rows = vec![
                crate::rune_transcript::RowData {
                    key: "msg-0".to_string(),
                    role: "user",
                    body: "hello".to_string(),
                    ..Default::default()
                },
                crate::rune_transcript::RowData {
                    key: "msg-1".to_string(),
                    role: "agent",
                    body: "hi".to_string(),
                    ..Default::default()
                },
            ];
        }
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(s, canvas_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        let copy = format!("{card}-mini-msg-0-copy");
        let (cx, cy) = rt.find(&copy).expect("copy action mounted");
        rt.pointer_move(cx, cy);
        rt.pointer_down(cx, cy);
        rt.pointer_up(cx, cy);
        rt.frame(1.0 / 60.0);
        assert!(
            rt.state.sessions.threads[&card]
                .copied_messages
                .contains_key("msg-0"),
            "copy feedback keyed to the message"
        );
        let rewind = format!("{card}-mini-msg-0-rewind");
        let (rx, ry) = rt.find(&rewind).expect("rewind action mounted");
        rt.pointer_move(rx, ry);
        rt.pointer_down(rx, ry);
        rt.pointer_up(rx, ry);
        rt.frame(1.0 / 60.0);
        assert!(
            rt.state.sessions.threads[&card]
                .rewind_armed
                .contains_key("msg-0"),
            "first rewind press arms the two-step confirm"
        );
    }

    #[test]
    fn background_card_mounts_the_compact_ask_panel() {
        let mut s = CanvasState::map();
        let background = s.cards[1].id.clone();
        s.active_id = s.cards[0].id.clone();
        s.sessions.sheet = Some(s.active_id.clone());
        let chat_id = s.sessions.threads[&background].chat_id.clone();
        let mut batch = crate::ask::AskBatch::list_from_json(&serde_json::json!({
            "batches": [{
                "batch_id": "b-card",
                "chat_id": chat_id,
                "created": 1,
                "questions": [{
                    "id": "q1",
                    "question": "Which branch?",
                    "options": ["main", "dev"]
                }]
            }]
        }))
        .remove(0);
        batch.chat_id = chat_id;
        s.sessions.threads.get_mut(&background).unwrap().ask.batches = vec![batch];
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(s, canvas_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        assert!(rt.find("ask-panel").is_some(), "compact ask mounted");
        let (rect, _) = rt.presented("ask-panel").expect("panel laid out");
        assert!(
            rect[3] <= 250.5,
            "canvas ask panel respects its 250pt cap, got {}",
            rect[3]
        );
    }

    #[test]
    fn history_refresh_keeps_seen_rows_and_drops_deleted_keys() {
        let mut canvas = CanvasState::map();
        let mut thread = crate::rune_transcript::RuneTranscriptState::screenshot();
        thread.seen_rows.insert("keep".to_string());
        thread.seen_rows.insert("gone".to_string());
        canvas
            .sessions
            .threads
            .insert("chat:ws:chat-1".to_string(), thread);
        let rows = vec![crate::chats::ChatMsgRow {
            key: "keep".to_string(),
            ..Default::default()
        }];
        assert!(canvas.apply_chat_messages("ws:chat-1", &rows));
        let thread = &canvas.sessions.threads["chat:ws:chat-1"];
        assert!(
            thread.seen_rows.contains("keep"),
            "a surviving row keeps its settled entrance"
        );
        assert!(
            !thread.seen_rows.contains("gone"),
            "a removed row would animate again if it returned"
        );
        assert_eq!(thread.rows.len(), 1);
    }

    #[test]
    fn local_chat_rekey_moves_every_reference() {
        let mut s = CanvasState::map();
        s.real = true;
        let old = s.new_session();
        assert!(old.starts_with("chat:local:"));
        s.inputs.get_mut(&old).unwrap().replace_text("keep me");
        s.closed.push((old.clone(), "New Chat".to_string()));
        s.layout_meta.insert(
            old.clone(),
            CardLayoutMeta {
                is_tab: true,
                depth: 2,
                parent_item_id: Some("chat:ws-general:chat-1".to_string()),
            },
        );
        s.card_attachments.insert(
            old.clone(),
            vec![crate::chats::ChatAttachment {
                id: "a1".to_string(),
                name: "notes.txt".to_string(),
                file_type: "text/plain".to_string(),
                size: 3,
                path: "notes.txt".to_string(),
                is_image: false,
                url: None,
            }],
        );
        s.card_attachment_errors
            .insert(old.clone(), "keep me".to_string());
        s.card_drop = Some(old.clone());
        {
            let thread = s.sessions.threads.get_mut(&old).unwrap();
            thread.pending_rekey = Some(("team".to_string(), "real-9".to_string()));
            assert_eq!(thread.workspace_id, "_general");
            assert!(thread.chat_id.is_empty());
            assert!(!thread.chat_ready);
        }

        assert!(s.rekey_local_chat(&old, "team", "real-9"));
        let new = "chat:team:real-9";
        assert!(s.card(&old).is_none());
        assert!(s.card(new).is_some());
        assert!(s.sessions.threads.contains_key(new));
        assert!(!s.sessions.threads.contains_key(&old));
        assert!(s.sessions.order.contains(&new.to_string()));
        assert!(!s.sessions.order.contains(&old));
        assert_eq!(s.sessions.sheet.as_deref(), Some(new));
        assert_eq!(s.active_id, new);
        assert_eq!(s.inputs.get(new).unwrap().text, "keep me");
        assert!(!s.inputs.contains_key(&old));
        assert_eq!(s.card_attachments(new).len(), 1);
        assert!(!s.card_attachments.contains_key(&old));
        assert_eq!(s.card_attachment_errors.get(new).unwrap(), "keep me");
        assert_eq!(s.card_drop.as_deref(), Some(new));
        assert!(s.closed.iter().any(|(id, _)| id == new));
        assert!(s.layout_meta.contains_key(new));
        assert!(s.manual_cards.contains(new));
        assert_eq!(s.active_tab, "team:real-9");
        assert!(s.open_tabs.iter().any(|key| key == "team:real-9"));
        let thread = &s.sessions.threads[new];
        assert_eq!(thread.workspace_id, "team");
        assert_eq!(thread.chat_id, "real-9");
        assert!(thread.chat_ready);
        assert!(thread.pending_rekey.is_none());
        // Once rekeyed, the old id is no longer a local draft.
        assert!(!s.rekey_local_chat(&old, "team", "other"));
    }

    #[test]
    fn snapshot_keeps_rekeyed_live_card_and_open_tab() {
        let mut s = CanvasState::map();
        s.real = true;
        let old = s.new_session();
        assert!(s.rekey_local_chat(&old, "team", "real-9"));
        let new = "chat:team:real-9".to_string();
        {
            let thread = s.sessions.threads.get_mut(&new).unwrap();
            thread.live = true;
            thread.rows = vec![crate::rune_transcript::RowData {
                key: "live-user".to_string(),
                role: "user",
                body: "draft".to_string(),
                ..Default::default()
            }];
        }
        let mut messages = std::collections::HashMap::new();
        messages.insert("ws-general:chat-1".to_string(), Vec::new());
        let snapshot = crate::chats::ChatSnapshot {
            open_tabs: vec!["ws-general:chat-1".to_string()],
            active_tab: String::new(),
            messages,
            chats: vec![crate::chats::ChatSummary {
                id: "chat-1".to_string(),
                workspace_id: "ws-general".to_string(),
                workspace_name: "General".to_string(),
                workspace_kind: "general".to_string(),
                title: "New Conversation".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        s.apply_chat_snapshot(&snapshot);
        // The server does not know about this tab yet; the native client keeps
        // it mounted and open.
        assert!(s.card(&new).is_some());
        assert!(s.open_tabs.iter().any(|key| key == "team:real-9"));
        assert_eq!(s.active_tab, "team:real-9");
        // And the in-flight run keeps its local rows.
        let thread = &s.sessions.threads[&new];
        assert!(thread.live);
        assert_eq!(thread.rows.len(), 1);
        assert_eq!(thread.rows[0].key, "live-user");
    }

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
        // The map has no dock of its own (the reference UI: the home chat card owns the
        // prompt dock); chrome stays to the add FAB + camera HUD.
        for key in ["canvas-add", "cam-hud", "cam-fit", "cam-reset"] {
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
                ..Default::default()
            },
            crate::rune_transcript::RowData {
                key: "a1".to_string(),
                role: "agent",
                body: "4152 ÷ 5 = 830.4".to_string(),
                at: "2026-11-09T14:44:30Z".to_string(),
                ..Default::default()
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
            format!("{empty}-convo-empty"),
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
    fn session_polls_preserve_positions_and_camera_until_structure_changes() {
        fn chat(id: &str, updated: &str) -> crate::chats::ChatSummary {
            crate::chats::ChatSummary {
                id: id.to_string(),
                workspace_id: "_general".to_string(),
                title: id.to_string(),
                updated_at: updated.to_string(),
                ..Default::default()
            }
        }
        let mut state = CanvasState::map();
        state.view_w = 740.0;
        state.view_h = 500.0;
        let mut snapshot = crate::chats::ChatSnapshot {
            open_tabs: vec!["_general:a".into(), "_general:b".into()],
            active_tab: "_general:a".into(),
            messages: std::collections::HashMap::new(),
            chats: vec![
                chat("a", "2026-01-01T00:00:00Z"),
                chat("b", "2026-01-02T00:00:00Z"),
            ],
            ..Default::default()
        };
        state.adopt_boot_snapshot(&snapshot);
        assert!(state.cam_fitted, "the first snapshot fits the map");
        let fitted = state.cam;
        let a = "chat:_general:a";
        let b = "chat:_general:b";
        let (ax, ay) = (state.card(a).unwrap().x, state.card(a).unwrap().y);
        // A manual arrangement survives poll/resize (the reference UI keeps auto items'
        // x/y and the saved view).
        {
            let card = state.cards.iter_mut().find(|c| c.id == a).unwrap();
            card.x += 120.0;
            card.y += 45.0;
        }
        state.set_viewport(800.0, 520.0);
        assert_eq!(state.cam, fitted, "resizing does not refit");
        state.apply_chat_snapshot(&snapshot);
        assert_eq!(
            (state.card(a).unwrap().x, state.card(a).unwrap().y),
            (ax + 120.0, ay + 45.0)
        );
        assert_eq!(state.cam, fitted, "a poll does not refit");
        // `relayout` snaps auto cards back to the tidy-tree slot.
        state.relayout();
        assert_eq!(
            (state.card(a).unwrap().x, state.card(a).unwrap().y),
            (ax, ay)
        );
        // Becoming a child changes the structural slot: the stale x/y is
        // dropped for the fresh layout position.
        snapshot.chats[1].parent_chat_id = Some("a".to_string());
        snapshot.chats[1].parent_workspace_id = Some("_general".to_string());
        let (fresh, _) = crate::chats::layout(&snapshot);
        let fresh_b = fresh.iter().find(|g| g.id == b).unwrap();
        state.apply_chat_snapshot(&snapshot);
        assert_eq!(
            state.card(b).map(|c| (c.x, c.y)),
            Some((fresh_b.x, fresh_b.y))
        );
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
        assert!(
            (rt.state.cam_scale_live.get() - rt.state.cam.scale).abs() < 1e-4,
            "live scale follows the camera: {}",
            rt.state.cam_scale_live.get()
        );
        let (x, y) = rt.find("cam-reset").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.state.cam_auto, "reset returns the camera to auto-fit");
        assert!(
            (rt.state.cam.scale - 0.84033614).abs() < 0.01,
            "reset fits the stock board: {}",
            rt.state.cam.scale
        );
        assert!(
            (rt.state.cam_scale_live.get() - rt.state.cam.scale).abs() < 1e-4,
            "live scale follows a fit: {}",
            rt.state.cam_scale_live.get()
        );
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
        let drawn_before = rt.find("chat:ws-general:chat-1").expect("card drawn");
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
        assert_eq!(
            rt.state.cam_live.get(),
            [rt.state.cam.x, rt.state.cam.y],
            "live offset follows the camera"
        );
        // The camera is published through live_offset/live_scale; the drawn
        // rect must follow without any layout change to the card itself.
        let drawn_after = rt.find("chat:ws-general:chat-1").expect("card still drawn");
        assert!(
            (drawn_after.0 - drawn_before.0 - 40.0).abs() < 2.0
                && (drawn_after.1 - drawn_before.1 - 30.0).abs() < 2.0,
            "drawn card follows the live camera: {drawn_before:?} -> {drawn_after:?}"
        );
    }

    #[test]
    fn header_drag_moves_card_by_pointer_travel() {
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(CanvasState::map(), canvas_view, (MAP_W, MAP_H), env);
        rt.frame(0.0);
        // Drag from the grip: the head now carries buttons (title opens the
        // sheet), so the grip is the drag-safe spot — like the reference UI's
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
    fn running_card_has_border_sweep_status_and_no_sheen_overlay() {
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(CanvasState::map(), canvas_view, (MAP_W, MAP_H), env);
        rt.frame(0.0);
        let run_id = "chat:ws-general:chat-4";
        let idle_id = "chat:ws-general:chat-1";
        assert!(
            rt.find(&format!("{run_id}-badge")).is_some(),
            "running badge mounted"
        );
        // The sweep is a fragment-level border paint (`El::border_sweep`), not
        // a child: a child would cover content and could not be clipped to the
        // exact 1 pt ring.
        assert!(
            rt.find(&format!("{run_id}-sheen")).is_none(),
            "the old full-card sheen child is gone"
        );
        assert!(
            rt.find(&format!("{idle_id}-sheen")).is_none(),
            "idle card has no sweep child either"
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

    fn sidebar_state() -> CanvasState {
        let chat = |id: &str, title: &str, at: &str| crate::chats::ChatSummary {
            id: id.to_string(),
            workspace_id: "_general".to_string(),
            workspace_name: "General".to_string(),
            workspace_kind: "general".to_string(),
            title: title.to_string(),
            updated_at: at.to_string(),
            ..Default::default()
        };
        let mut s = CanvasState::map();
        s.real = true;
        s.workspaces = vec![crate::chats::ChatWorkspace {
            id: "_general".to_string(),
            name: "General".to_string(),
            kind: "general".to_string(),
            exists: true,
            ..Default::default()
        }];
        s.chat_rows = vec![
            chat("a", "alpha", "2026-11-11T09:00:00Z"),
            chat("b", "bravo", "2026-11-10T09:00:00Z"),
        ];
        s.sessions.pinned = vec!["_general:b".to_string()];
        s
    }

    fn sidebar_only(s: &CanvasState, _: &mut rune::Cx<CanvasState>) -> El {
        col()
            .key("sidebar-test-root")
            .w_full()
            .h_full()
            .child(chats_panel::<CanvasState>(s))
    }

    #[test]
    fn an_empty_boot_snapshot_clears_the_demo_cards() {
        let mut s = CanvasState::map();
        assert!(!s.cards.is_empty(), "the demo scene starts with cards");
        let snapshot = crate::chats::ChatSnapshot::default();
        s.adopt_boot_snapshot(&snapshot);
        assert!(s.cards.is_empty(), "a fresh account has no canvas cards");
        assert!(s.sessions.order.is_empty());
        assert!(s.sessions.threads.is_empty());
        assert!(s.sessions.sheet.is_none());
        assert!(s.active_id.is_empty());
        assert!(s.open_tabs.is_empty());
    }

    #[test]
    fn rename_field_takes_focus_and_selects_the_title() {
        use rune::{Key, KeyEvent, Mods};
        let mut s = sidebar_state();
        s.start_rename("_general:b");
        {
            let (_, edit) = s.sessions.editing.as_ref().expect("rename started");
            assert!(edit.focused, "the shell tick must not see an instant blur");
            assert_eq!((edit.anchor, edit.focus), (0, 5), "title is selected");
        }
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(s, sidebar_only, (248.0, 900.0), env);
        rt.frame(0.0);
        for ch in ["G", "o"] {
            rt.key(KeyEvent {
                key: Key::Text(ch.to_string()),
                mods: Mods::default(),
            });
            rt.frame(0.0);
        }
        let (_, edit) = rt.state.sessions.editing.as_ref().expect("still editing");
        assert_eq!(edit.text, "Go", "typing replaces the selection");
        assert!(edit.focused, "the mounted field keeps focus");
        // Enter commits the trimmed title and closes the field.
        rt.key(KeyEvent {
            key: Key::Enter,
            mods: Mods::default(),
        });
        for _ in 0..3 {
            rt.frame(1.0 / 60.0);
        }
        assert!(
            rt.state.sessions.editing.is_none(),
            "submit closes the field"
        );
        assert_eq!(rt.state.chat_rows[1].title, "Go");
    }

    #[test]
    fn rename_commits_when_the_field_loses_focus() {
        use rune::{Key, KeyEvent, Mods};
        let mut s = sidebar_state();
        s.start_rename("_general:b");
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(s, sidebar_only, (248.0, 900.0), env);
        rt.frame(0.0);
        rt.key(KeyEvent {
            key: Key::Text("Z".to_string()),
            mods: Mods::default(),
        });
        rt.frame(0.0);
        assert_eq!(
            rt.state.sessions.editing.as_ref().unwrap().1.text,
            "Z",
            "typing replaces the selected title"
        );
        let (sx, sy) = rt.find("map-search").unwrap();
        rt.pointer_down(sx, sy);
        rt.pointer_up(sx, sy);
        for _ in 0..3 {
            rt.frame(1.0 / 60.0);
        }
        assert!(
            !rt.state.sessions.editing.as_ref().unwrap().1.focused,
            "clicking the search field blurs the rename"
        );
        assert!(crate::rune_canvas::settle_sidebar_editing(&mut rt.state));
        assert!(rt.state.sessions.editing.is_none(), "blur closes the field");
        assert_eq!(rt.state.chat_rows[1].title, "Z", "blur commits the title");
    }

    #[test]
    fn escape_cancels_inline_rename_without_writing() {
        use rune::{Key, KeyEvent, Mods};
        let mut s = sidebar_state();
        s.start_rename("_general:b");
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(s, sidebar_only, (248.0, 900.0), env);
        rt.frame(0.0);
        rt.key(KeyEvent {
            key: Key::Escape,
            mods: Mods::default(),
        });
        rt.frame(0.0);
        assert!(
            rt.state.sessions.editing.is_none(),
            "escape closes the field"
        );
        assert_eq!(
            rt.state.chat_rows[1].title, "bravo",
            "escape keeps the title"
        );
    }

    #[test]
    fn sidebar_pins_sort_first_and_hover_reveals_the_action_cluster() {
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(sidebar_state(), sidebar_only, (248.0, 900.0), env);
        rt.frame(0.0);
        let b = "side-chat:_general:b";
        let a = "side-chat:_general:a";
        let (rb, _) = rt.presented(b).unwrap();
        let (ra, _) = rt.presented(a).unwrap();
        assert!(rb[1] < ra[1], "pinned bravo sorts above alpha");
        assert!(
            rt.find("side-chat:_general:b-pinmark").is_some(),
            "pinned rows carry the rotated pin"
        );
        let actions = "side-chat:_general:b-actions";
        let (rect, opacity) = rt.presented(actions).unwrap();
        assert!(opacity < 0.01, "cluster rests hidden: {opacity}");
        assert!(
            (rect[0] + rect[2] - 234.0).abs() < 1.5,
            "right-1 to the row: {rect:?}"
        );
        // Hovering the row fades the cluster in.
        rt.pointer_move(120.0, rb[1] + rb[3] * 0.5);
        for _ in 0..6 {
            rt.frame(1.0 / 60.0);
        }
        let (_, opacity) = rt.presented(actions).unwrap();
        assert!(opacity > 0.5, "hover reveals the actions: {opacity}");
    }

    #[test]
    fn two_presses_on_the_row_delete_button_start_the_fade() {
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(sidebar_state(), sidebar_only, (248.0, 900.0), env);
        rt.frame(0.0);
        let (bx, by) = rt.find("side-chat:_general:b").unwrap();
        rt.pointer_move(bx, by);
        for _ in 0..4 {
            rt.frame(1.0 / 60.0);
        }
        let (dx, dy) = rt.find("side-chat:_general:b-delete").unwrap();
        rt.pointer_down(dx, dy);
        rt.pointer_up(dx, dy);
        for _ in 0..2 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.state.delete_confirming("_general:b"), "first press arms");
        rt.pointer_down(dx, dy);
        rt.pointer_up(dx, dy);
        for _ in 0..2 {
            rt.frame(1.0 / 60.0);
        }
        assert!(
            rt.state.sessions.deleting.is_some(),
            "second press starts the fade: {:?}",
            rt.state.sessions.deleting
        );
    }

    #[test]
    fn deleting_row_fades_and_collapses_without_the_action_cluster() {
        let mut s = sidebar_state();
        let now = crate::clock::now_millis();
        // Mid-animation: a long window keeps the test's own startup time from
        // moving off the midpoint. Eased progress is ~0.5, so the 34.8 pt slot
        // should be near 17.4 pt with the opacity near 0.5.
        s.sessions.deleting = Some(("_general:b".to_string(), now - 100_000, now + 100_000));
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(s, sidebar_only, (248.0, 900.0), env);
        rt.frame(0.0);
        let actions = "side-chat:_general:b-actions";
        assert!(rt.find(actions).is_none(), "deleting row drops the cluster");
        let (rect, opacity) = rt.presented("side-wrap-chat:_general:b").unwrap();
        assert!(
            rect[3] < 12.0 && rect[3] > 2.0,
            "the slot collapses with the fade: {rect:?}"
        );
        assert!(
            opacity > 0.05 && opacity < 0.4,
            "row is fading with the slot: {opacity} at {rect:?}"
        );
    }

    #[test]
    fn coding_group_nests_same_workspace_oldest_first() {
        use crate::chats::ChatSummary;
        let chat = |ws: &str, id: &str, parent: Option<&str>, created: &str| ChatSummary {
            id: id.to_string(),
            workspace_id: ws.to_string(),
            workspace_name: ws.to_string(),
            workspace_kind: "project".to_string(),
            title: id.to_string(),
            updated_at: created.to_string(),
            created_at: created.to_string(),
            parent_chat_id: parent.map(|p| p.to_string()),
            ..Default::default()
        };
        let rows = vec![
            chat("ws-a", "top", None, "2026-11-11T09:00:00Z"),
            chat("ws-a", "new-kid", Some("top"), "2026-11-11T10:00:00Z"),
            chat("ws-a", "old-kid", Some("top"), "2026-11-11T08:00:00Z"),
            chat("ws-a", "xws", Some("elsewhere"), "2026-11-11T07:00:00Z"),
            chat("ws-b", "elsewhere", None, "2026-11-11T06:00:00Z"),
        ];
        let (tops, kids) = coding_group(&rows, "ws-a");
        let top_ids: Vec<&str> = tops.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            top_ids,
            vec!["top", "xws"],
            "same-ws kids nest, cross-ws stay top"
        );
        let nest: Vec<&str> = kids["top"].iter().map(|c| c.id.as_str()).collect();
        assert_eq!(nest, vec!["old-kid", "new-kid"], "oldest first");
    }

    fn coding_state() -> CanvasState {
        use crate::chats::{ChatSummary, ChatWorkspace};
        let chat = |ws: &str, id: &str, parent: Option<&str>, at: &str| ChatSummary {
            id: id.to_string(),
            workspace_id: ws.to_string(),
            workspace_name: ws.to_string(),
            workspace_kind: "project".to_string(),
            title: id.to_string(),
            updated_at: at.to_string(),
            created_at: at.to_string(),
            parent_chat_id: parent.map(|p| p.to_string()),
            ..Default::default()
        };
        let ws = |id: &str, name: &str, exists: bool| ChatWorkspace {
            id: id.to_string(),
            name: name.to_string(),
            kind: "project".to_string(),
            exists,
            ..Default::default()
        };
        let mut s = CanvasState::map();
        s.real = true;
        s.workspaces = vec![ws("ws-a", "Alpha", true), ws("ws-b", "Beta", false)];
        s.chat_rows = vec![
            chat("ws-a", "top", None, "2026-11-11T09:00:00Z"),
            chat("ws-a", "kid", Some("top"), "2026-11-11T10:00:00Z"),
            chat("ws-a", "xws", Some("gone"), "2026-11-11T07:00:00Z"),
        ];
        s.sessions.kind = "Coding".to_string();
        s
    }

    #[test]
    fn coding_filter_groups_projects_with_counts_and_badges() {
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(coding_state(), sidebar_only, (248.0, 900.0), env);
        rt.frame(0.0);
        // Both project headers render with their loaded chat counts; the
        // missing folder's name is present for its amber wash.
        assert!(rt.find("side-group-ws-a").is_some(), "Alpha group header");
        assert!(rt.find("side-group-ws-b").is_some(), "Beta group header");
        // Cross-workspace child stays top-level with a `from` badge whose
        // lookup misses (parent is not in any workspace).
        assert!(
            rt.find("side-chat:ws-a:xws-from-label").is_some(),
            "from badge"
        );
        // Beta has no chats.
        assert!(rt.find("side-group-ws-b-empty").is_some(), "empty group");
    }

    #[test]
    fn coding_filter_pages_each_project_at_seven() {
        use crate::chats::ChatSummary;
        let mut s = coding_state();
        for i in 0..8 {
            s.chat_rows.push(ChatSummary {
                id: format!("extra-{i}"),
                workspace_id: "ws-a".to_string(),
                workspace_name: "Alpha".to_string(),
                workspace_kind: "project".to_string(),
                title: format!("extra-{i}"),
                updated_at: format!("2026-11-01T00:00:{i:02}Z"),
                created_at: format!("2026-11-01T00:00:{i:02}Z"),
                ..Default::default()
            });
        }
        // 10 tops in ws-a (top, xws + 8 extras): 7 visible + Load more (3).
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(s, sidebar_only, (248.0, 900.0), env);
        rt.frame(0.0);
        assert!(
            rt.find("side-loadmore-ws-a").is_some(),
            "Load more pages ws-a"
        );
        // Press Load more: the limit grows past the total and the button leaves.
        let (lx, ly) = rt.find("side-loadmore-ws-a").expect("load more placed");
        rt.pointer_down(lx, ly);
        rt.pointer_up(lx, ly);
        rt.frame(1.0 / 60.0);
        rt.frame(1.0 / 60.0);
        assert!(
            rt.find("side-loadmore-ws-a").is_none() && rt.find("side-chat:ws-a:extra-7").is_some(),
            "Load more reveals the rest"
        );
    }

    #[test]
    fn general_filter_pages_at_fifteen() {
        let mut s = sidebar_state();
        for i in 0..16 {
            s.chat_rows.push(crate::chats::ChatSummary {
                id: format!("g{i}"),
                workspace_id: "_general".to_string(),
                workspace_name: "General".to_string(),
                workspace_kind: "general".to_string(),
                title: format!("g{i}"),
                updated_at: format!("2026-11-01T00:00:{i:02}Z"),
                created_at: format!("2026-11-01T00:00:{i:02}Z"),
                ..Default::default()
            });
        }
        // 18 tops: 15 visible + Load more (3).
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(s, sidebar_only, (248.0, 900.0), env);
        rt.frame(0.0);
        assert!(
            rt.find("side-loadmore-general").is_some(),
            "general pages at 15"
        );
    }

    #[test]
    fn plus_button_starts_a_chat_in_that_project() {
        let mut s = coding_state();
        let id = s.new_session_in("ws-b");
        let thread = s.sessions.threads.get(&id).expect("thread created");
        assert_eq!(thread.workspace_id, "ws-b", "plus targets its own project");
    }

    #[test]
    fn inline_rename_commits_and_delete_is_two_step() {
        let mut s = sidebar_state();
        s.start_rename("_general:b");
        assert!(s.sessions.confirm_delete.is_none());
        s.sessions.editing.as_mut().expect("rename started").1 = rune::TextEdit::new("  Renamed  ");
        let write = s.commit_editing().expect("changed title writes");
        assert_eq!(write.0, "_general");
        assert_eq!(write.1, "b");
        assert_eq!(write.2, "Renamed");
        assert_eq!(s.chat_rows[1].title, "Renamed");
        assert!(s.sessions.editing.is_none());
        // A blank or unchanged title only closes the field.
        s.start_rename("_general:b");
        s.sessions.editing.as_mut().expect("rename started").1 = rune::TextEdit::new("   ");
        assert!(s.commit_editing().is_none());
        // First press arms the 2.5s confirm; the second starts the 220ms fade.
        s.press_delete("_general:b");
        assert!(s.delete_confirming("_general:b"));
        assert!(s.sessions.deleting.is_none());
        s.press_delete("_general:b");
        assert!(s.sessions.confirm_delete.is_none());
        assert!(s
            .sessions
            .deleting
            .as_ref()
            .is_some_and(|(id, _, _)| id == "_general:b"));
        assert!(s.delete_fade("_general:b").is_some());
    }

    #[test]
    fn new_chat_lands_in_the_mode_workspace() {
        let mut s = CanvasState::map();
        s.workspaces = vec![
            crate::chats::ChatWorkspace {
                id: "_general".to_string(),
                kind: "general".to_string(),
                ..Default::default()
            },
            crate::chats::ChatWorkspace {
                id: "_house".to_string(),
                kind: "house".to_string(),
                ..Default::default()
            },
            crate::chats::ChatWorkspace {
                id: "_personal".to_string(),
                kind: "personal".to_string(),
                ..Default::default()
            },
            crate::chats::ChatWorkspace {
                id: "proj-zeta".to_string(),
                name: "Zeta".to_string(),
                kind: "project".to_string(),
                ..Default::default()
            },
            crate::chats::ChatWorkspace {
                id: "proj-alpha".to_string(),
                name: "Alpha".to_string(),
                kind: "project".to_string(),
                ..Default::default()
            },
        ];
        for (kind, expected) in [
            ("Main", "_general"),
            ("Coding", "proj-alpha"),
            ("House", "_house"),
            ("General", "_personal"),
        ] {
            s.sessions.kind = kind.to_string();
            let id = s.new_session();
            assert_eq!(
                s.sessions.threads[&id].workspace_id, expected,
                "{kind} drafts in {expected}"
            );
        }
    }
}
