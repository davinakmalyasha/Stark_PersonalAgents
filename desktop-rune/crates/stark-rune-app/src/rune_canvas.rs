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
/// Icon rail width (React `HouseRail`: 36pt button + 6pt gutters + hairline).
pub const RAIL_W: f32 = 49.0;
pub const DOT_STEP: f32 = 26.0;
/// Chat kinds (the workspace dropdown). Chats are the only type — no
/// workspace scoping, everything lives in side tabs on one page.
pub const CHAT_KINDS: [&str; 4] = ["Main", "Coding", "House", "General"];

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
    #[cfg(test)]
    pub fn fit(&mut self, cards: &[MapCard]) {
        self.fit_in(cards, MAP_W - MAP_SIDEBAR_W, MAP_H);
    }

    /// `fitView` against a live stage size: React's paddings and clamps
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
    /// Expanded spawn nests (`HouseSidebar.collapsedSpawns` inverted): spawn
    /// children stay nested under their parent until the chevron opens them.
    pub spawn_open: std::collections::HashSet<String>,
}

impl std::fmt::Debug for Sessions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sessions")
            .field("threads", &self.threads.keys().collect::<Vec<_>>())
            .field("order", &self.order)
            .field("sheet", &self.sheet)
            .field("kind", &self.kind)
            .field("spawn_open", &self.spawn_open)
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
            spawn_open: self.spawn_open.clone(),
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
            spawn_open: std::collections::HashSet::new(),
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
    /// Presentation copy of the camera. Pan and zoom publish here and say
    /// `tx.live_only()`, so dragging the map never rebuilds the view (Rune
    /// binds offsets/scales at draw time; hit tests follow them).
    pub cam_live: rune::Live<[f32; 2]>,
    pub cam_scale_live: rune::Live<f32>,
    /// Dot grid offset: camera pan modulo the 26pt tile. The field is one
    /// tile larger than the viewport, so wrapping by a tile is invisible.
    pub grid_live: rune::Live<[f32; 2]>,
    /// Camera follows the content (React `fitView`) until the first manual
    /// pan/zoom; the shell refits when the window resizes.
    pub cam_auto: bool,
    /// `fittedRef`: the map fits once when the first cards arrive, never
    /// again on a session poll (React preserves the user's pan/zoom).
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
    /// Every chat on the server (sidebar rows), newest first. Empty until the
    /// first real snapshot lands (or when the server is unreachable).
    pub chat_rows: Vec<crate::chats::ChatSummary>,
    /// Open tab keys (`workspace:chat`, React `open_tabs`) and the active one.
    pub open_tabs: Vec<String>,
    pub active_tab: String,
    /// A sidebar/tab click wants this chat's history loaded by the shell.
    pub pending_chat_fetch: Option<(String, String)>,
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
            open_tabs: self.open_tabs.clone(),
            active_tab: self.active_tab.clone(),
            pending_chat_fetch: self.pending_chat_fetch.clone(),
            real: self.real,
            manual_cards: self.manual_cards.clone(),
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
            open_tabs: Vec::new(),
            active_tab: String::new(),
            pending_chat_fetch: None,
            real: false,
            manual_cards: std::collections::HashSet::new(),
            inputs: std::collections::HashMap::new(),
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
        // The home chat overlay always shows the active session (React's
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

    /// Rebuild the map cards from `open_tabs` + `chat_rows` using the React
    /// tidy-tree layout. Local draft cards survive the rebuild.
    fn sync_graph_cards(&mut self) {
        let snapshot = crate::chats::ChatSnapshot {
            open_tabs: self.open_tabs.clone(),
            active_tab: self.active_tab.clone(),
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
            // the card's structural slot (depth / parent / tab) changed.
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
    }

    /// Adopt the live server snapshot: real cards at React's coordinates,
    /// real sidebar rows, and real message history for every open tab.
    pub fn apply_chat_snapshot(&mut self, snapshot: &crate::chats::ChatSnapshot) {
        if snapshot.open_tabs.is_empty() && snapshot.chats.is_empty() {
            return;
        }
        self.real = true;
        self.chat_rows = snapshot.chats.clone();
        self.open_tabs = snapshot.open_tabs.clone();
        // A chat this client just created may not be in the server's saved
        // `open_tabs` yet (native never writes UI state). Keep manual cards
        // open so their live thread survives the refresh.
        for id in &self.manual_cards {
            if let Some(key) = id.strip_prefix("chat:") {
                if !self.open_tabs.iter().any(|open| open == key) {
                    self.open_tabs.push(key.to_string());
                }
            }
        }
        self.active_tab = snapshot.active_tab.clone();
        if self.active_tab.is_empty() && !self.open_tabs.is_empty() {
            if let Some(id) = self.manual_cards.iter().next() {
                if let Some(key) = id.strip_prefix("chat:") {
                    self.active_tab = key.to_string();
                }
            }
        }
        self.sync_graph_cards();
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
            // A snapshot can land while a just-created chat is streaming (the
            // `chat` frame refreshes the sidebar mid-run). Local rows and the
            // live flag stay authoritative until the run finishes.
            thread.chat_id = chat_id;
            if !thread.live && !thread.aborting {
                if let Some(rows) = snapshot.messages.get(&key) {
                    thread.rows = rows.iter().map(row_from_chat).collect();
                }
            }
        }
        let want = format!("chat:{}", snapshot.active_tab);
        self.active_id = if self.card(&want).is_some() {
            want
        } else {
            self.sessions.order.first().cloned().unwrap_or_default()
        };
        if let Some(card) = self.cards.iter().find(|c| c.id == self.active_id) {
            self.sessions.kind = card.kind.clone();
        }
        self.sessions.sheet = if self.active_id.is_empty() {
            None
        } else {
            Some(self.active_id.clone())
        };
        if self.cam_auto && !self.cam_fitted {
            let (vw, vh) = (self.view_w, self.view_h);
            self.cam.fit_in(&self.cards, vw, vh);
            self.sync_cam_live();
            self.cam_fitted = true;
        }
    }

    /// Fill one card's transcript from a per-chat history fetch.
    pub fn apply_chat_messages(&mut self, key: &str, rows: &[crate::chats::ChatMsgRow]) -> bool {
        let card_id = format!("chat:{key}");
        let Some(thread) = self.sessions.threads.get_mut(&card_id) else {
            return false;
        };
        thread.rows = rows.iter().map(row_from_chat).collect();
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
        self.pending_chat_fetch = Some((workspace_id.to_string(), chat_id.to_string()));
        self.select_card(&card_id);
    }

    pub fn card(&self, id: &str) -> Option<&MapCard> {
        self.cards.iter().find(|c| c.id == id)
    }

    /// Resize the canvas stage (shell poll). The camera keeps its view across
    /// resizes, like React (only the first card set triggers `fitView`).
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

    /// React `relayout` (Reset layout): auto cards snap back to their tidy-tree
    /// slots, manual drafts stay put, then the map refits.
    pub fn relayout(&mut self) {
        let snapshot = crate::chats::ChatSnapshot {
            open_tabs: self.open_tabs.clone(),
            active_tab: self.active_tab.clone(),
            messages: std::collections::HashMap::new(),
            chats: self.chat_rows.clone(),
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
        let n = self.cards.len();
        let id = format!("chat:local:{}", crate::notes::mint_id("s"));
        let kind = self.sessions.kind.clone();
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
                    rune::el()
                        .key(format!("{id}-close"))
                        .size(14.0, 14.0)
                        .role("button")
                        .label(format!("Remove {}", card.title))
                        .shape(crate::lucide_x(14.0, 2.0, ink("#71717a")))
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

/// Lucide `Paperclip` approximation at a 24-unit box (the native card's
/// attach button is inert; the glyph still matches React's dock).
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

fn mini_bubble(id: &str, r: &crate::rune_transcript::RowData, live_row: bool) -> El {
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
        .child(
            text(r.body.clone())
                .font({
                    let mut style = rune::tok::TextStyle::new(12.5, 400.0);
                    style.line_height = 12.5 * 1.55;
                    style
                })
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
    // `.canvas-bubble-foot`: 22pt tall, time pushed right (the hover actions
    // are invisible at rest but set the height).
    let t = crate::rune_transcript::clock(&r.at);
    bubble = bubble.child(
        row()
            .w_full()
            .items_center()
            .min_h(22.0)
            .mt(8.0)
            .child(rune::el().grow(1.0))
            .child(
                text(t)
                    .key(format!("{id}-mini-{}-time", r.key))
                    .font(rune::tok::TextStyle::new(9.5, 400.0))
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
            .child(bubble)
            .child(avatar())
    } else {
        row()
            .key(format!("{id}-minrow-{}", r.key))
            .w_full()
            .items_start()
            .gap(8.0)
            .child(avatar())
            .child(bubble)
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
                .fill(ink(tone))
                .looping(rune::LoopSpec::opacity(2.2, 0.0, 0.35, 1.0)),
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
                .label(format!("Close {}", card.title))
                .child(rune::el().size(14.0, 14.0).inert().shape(crate::lucide_x(
                    14.0,
                    2.0,
                    ink("#71717a"),
                )))
                .on_press(move |s: &mut S| {
                    s.canvas_mut().close_session(&close_id);
                }),
        );
    // Conversation (`canvas-card-conversation`): React shows the latest user
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
                    convo = convo.child(mini_bubble(
                        &id,
                        &t.rows[i],
                        live && t.rows[i].key == "live-agent",
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
    // Card dock (`canvas-card-dock`): 4/14/14 padding around the bordered
    // input wrap; attach (inert — no Rune file picker), input, and the 25pt
    // send button. React's `sending` spinner state has no native target.
    let draft_empty = input.text.trim().is_empty();
    let send_id = id.clone();
    let edit_id = id.clone();
    let send_disabled = live || draft_empty;
    let attach = rune::el()
        .key(format!("{id}-attach"))
        .size(22.0, 22.0)
        .radius(6.0)
        .center()
        .inert()
        .child(
            rune::el()
                .size(12.0, 12.0)
                .inert()
                .shape(paperclip_icon(12.0, ink("#676b74"))),
        );
    let wrap = row()
        .key(format!("{id}-input-wrap"))
        .w_full()
        .min_h(38.0)
        .items_center()
        .gap(5.0)
        .px(5.0)
        .py(4.0)
        .radius(10.0)
        .border(1.0, ink("#ffffff16"))
        .border_layout()
        .fill(rune::Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.22,
        })
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
                .caret(ink("#f2f2f4"), ink("#3b82f6"))
                .on_edit(move |s: &mut S| s.canvas_mut().input_mut(&edit_id)),
        )
        .child(
            rune::el()
                .key(format!("{id}-send"))
                .role("button")
                .label(format!("Send to {send_id}"))
                .size(25.0, 25.0)
                .radius(8.0)
                .fill(if send_disabled {
                    ink("#ffffff14")
                } else {
                    ink("#f2f2f4")
                })
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
    let dock = col()
        .key(format!("{id}-dock"))
        .w_full()
        .px(14.0)
        .pt(4.0)
        .pb(14.0)
        .child(wrap);
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
                    rune::el()
                        .key("canvas-add-close")
                        .size(12.0, 12.0)
                        .role("button")
                        .label("Close add menu")
                        .shape(crate::lucide_x(12.0, 2.0, ink("#71717a")))
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
    let vw = s.view_w.max(1.0);
    let vh = s.view_h.max(1.0);
    // World layer: everything that pans/zooms. Origin top-left so pan offsets
    // map 1:1; `scale` springs on the layout spring and hit tests follow the
    // scaled rect (`El::scale`). Chrome (HUD, chat overlay) stays fixed, like
    // React's `[data-canvas-ui]`.
    let mut world = col()
        .key("canvas-world")
        .absolute(0.0, 0.0)
        .live_offset(&s.cam_live)
        .size(vw, vh)
        .origin(0.0, 0.0)
        .live_scale(&s.cam_scale_live);
    // Background pan surface: fixed, unscaled (React `CanvasStage` drag on
    // the stage). Cards sit above it, so empty presses pan while header
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
    // Fixed dotted grid (React `.canvas-grid`, map variant): 26pt tile,
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
        .clip_rounded(0.0)
        // React: dotted grid + stage background are fixed under the
        // transformed world (dots slide with pan, not zoom).
        .child(pan)
        .child(grid);
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
            .pl(7.0)
            .pr(9.0)
            .backdrop_blur(12.0)
            .role("button")
            .label("Add to canvas")
            .on_press(move |s: &mut S| {
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
                .size(vw, vh)
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
    let hud_icon = |key: &str, label: &str, icon: Shape| {
        rune::el()
            .key(format!("cam-{key}"))
            .role("button")
            .label(label.to_string())
            .size(28.0, 28.0)
            .radius(8.0)
            .center()
            .child(
                rune::el()
                    .key(format!("cam-{key}-icon"))
                    .size(13.0, 13.0)
                    .inert()
                    .shape(icon),
            )
    };
    canvas = canvas
        .child(
            row()
                .key("cam-hud")
                .absolute(vw - 127.0, 10.0)
                .items_center()
                .gap(1.0)
                .child(
                    hud_icon("zoom-out", "Zoom out", icon_minus(ink("#81848d"))).on_press_tx(
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
                    hud_icon("zoom-in", "Zoom in", icon_plus(ink("#81848d"))).on_press_tx(
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
                    hud_icon("fit", "Fit to view", icon_scan(ink("#81848d"))).on_press_tx(
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
                    hud_icon("reset", "Reset layout", icon_reset(ink("#81848d"))).on_press(
                        move |s: &mut S| {
                            // `relayout`: tidy-tree slots again plus a fresh fit.
                            s.canvas_mut().relayout();
                        },
                    ),
                ),
        )
        .child(
            // `.canvas-map-hint`: bottom-left, 9pt dim copy (hidden behind the
            // home chat card when it is open, exactly like React).
            text("Drag to pan · scroll to zoom · drag cards to arrange".to_string())
                .key("cam-hint")
                .absolute(18.0, vh - 26.0)
                .font(rune::tok::TextStyle::new(9.0, 400.0))
                .color(ink("#4f525a")),
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
fn sidebar_spawn_row<S: CanvasHost + 'static>(chat: &crate::chats::ChatSummary) -> El {
    let title = crate::chats::display_title(&chat.title);
    let ws = chat.workspace_id.clone();
    let chat_id = chat.id.clone();
    let card_id = chat.card_id();
    let mut style = rune::tok::TextStyle::new(11.5, 400.0);
    style.line_height = 17.25;
    row()
        .key(format!("side-kid-{card_id}"))
        .w_full()
        .items_center()
        .gap(6.0)
        .pl(26.0)
        .pr(8.0)
        .py(6.0)
        .radius(8.0)
        .role("button")
        .label(format!("Open {title}"))
        .on_press(move |s: &mut S| {
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
                        .label("Switch mode")
                        .on_press(move |s: &mut S| {
                            let c = s.canvas_mut();
                            c.kind_open = !c.kind_open;
                        })
                        .child(
                            text(s.sessions.kind.clone())
                                .truncate()
                                .font({
                                    // `text-[15px]` at Tailwind's 1.5 line-height:
                                    // the switcher is 34.5pt with py-1.5.
                                    let mut style = rune::tok::TextStyle::new(15.0, 600.0);
                                    style.line_height = 22.5;
                                    // React `tracking-tight`: -0.025em at 15pt.
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
                        .label("New chat")
                        .size(24.0, 24.0)
                        .radius(6.0)
                        .center()
                        .on_press(move |s: &mut S| {
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
    // Floating menu under the switcher (React `absolute top-[calc(100%+6px)]`).
    // Built separately and mounted after the list so it paints (and hits) on top.
    let mut kind_menu: Option<El> = None;
    if s.kind_open {
        let mut menu = col()
            .key("map-kind-menu")
            .absolute(14.0, 52.0)
            .w(220.0)
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
                    .label(format!("Switch to {kind}"))
                    .on_press(move |s: &mut S| {
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
        // (27.3pt tall at React's 11.5pt input). Rune draws its hairline
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
                        .caret(ink("#f2f2f4"), ink("#3b82f6"))
                        .on_edit(move |s: &mut S| &mut s.canvas_mut().sessions.search),
                ),
        );
    let title_style = || {
        let mut style = rune::tok::TextStyle::new(12.5, 400.0);
        style.line_height = 18.8;
        style
    };
    let stamp_style = || {
        let mut style = rune::tok::TextStyle::new(10.0, 400.0);
        style.line_height = 15.0;
        style
    };
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
        for chat in &s.chat_rows {
            let card_id = chat.card_id();
            let chat_key = chat.key();
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
            let kids: Vec<&&crate::chats::ChatSummary> = if flat {
                Vec::new()
            } else {
                spawn_kids
                    .get(&chat_key)
                    .map(|kids| {
                        kids.iter()
                            .filter(|kid| {
                                CanvasState::kind_for(&kid.workspace_kind) == s.sessions.kind
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let expanded = !kids.is_empty() && s.sessions.spawn_open.contains(&card_id);
            let active = card_id == s.active_id;
            let running = s.card(&card_id).map(|c| c.running).unwrap_or(false);
            let row_id = format!("side-{card_id}");
            let ws = chat.workspace_id.clone();
            let chat_id = chat.id.clone();
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
                .label(format!("Open {title}"))
                .on_press(move |s: &mut S| {
                    s.canvas_mut().open_chat_row(&ws, &chat_id);
                });
            if active {
                item = item.fill(rune::Color {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.09,
                });
            }
            if running {
                item = item.child(
                    rune::el()
                        .key(format!("side-{card_id}-dot"))
                        .size(6.0, 6.0)
                        .radius(3.0)
                        .fill(ink("#fafafa")),
                );
            } else {
                item = item.child(
                    rune::el()
                        .key(format!("side-{card_id}-icon"))
                        .size(12.0, 12.0)
                        .inert()
                        .shape(chat_row_icon(12.0, ink("#52525b"))),
                );
            }
            item = item.child(
                text(title)
                    .key(format!("side-{card_id}-title"))
                    .grow(1.0)
                    .min_w(0.0)
                    .truncate()
                    .font(title_style())
                    .color(ink(if active { "#ffffff" } else { "#a1a1aa" })),
            );
            let stamp = crate::chats::relative_time_short(&chat.updated_at);
            if !stamp.is_empty() {
                item = item.child(
                    text(stamp)
                        .key(format!("side-{card_id}-time"))
                        .font(stamp_style())
                        .color(ink("#52525b")),
                );
            }
            // `flex items-center gap-0.5` + 20pt leading slot (`w-5`): the
            // role/expand gutter. Root rows leave it empty, so the row box
            // starts at x=81 and spans 205pt (React `house-sidebar-row`).
            let lead = if kids.is_empty() {
                rune::el()
                    .key(format!("side-{card_id}-lead"))
                    .w(20.0)
                    .h(20.0)
            } else {
                let toggle = card_id.clone();
                let open = expanded;
                rune::el()
                    .key(format!("side-{card_id}-lead"))
                    .role("button")
                    .label(if open {
                        "Collapse nested chats"
                    } else {
                        "Expand spawned chats"
                    })
                    .size(20.0, 20.0)
                    .radius(6.0)
                    .center()
                    .on_press(move |s: &mut S| {
                        let set = &mut s.canvas_mut().sessions.spawn_open;
                        if !set.remove(&toggle) {
                            set.insert(toggle.clone());
                        }
                    })
                    .child(rune::el().size(11.0, 11.0).inert().shape(if open {
                        chevron_down(11.0, ink("#52525b"))
                    } else {
                        chevron_right(11.0, ink("#52525b"))
                    }))
            };
            list = list.child(
                row()
                    .key(format!("side-wrap-{card_id}"))
                    .w_full()
                    .items_center()
                    .gap(2.0)
                    .child(lead)
                    .child(
                        col()
                            .key(format!("side-{card_id}-cell"))
                            .grow(1.0)
                            .min_w(0.0)
                            .child(item),
                    ),
            );
            if expanded {
                for kid in kids {
                    list = list.child(
                        row()
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
                                    .child(sidebar_spawn_row::<S>(kid)),
                            ),
                    );
                }
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
    use super::*;
    use rune::{Env, Runtime};

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
        // The map has no dock of its own (React: the home chat card owns the
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
        };
        state.apply_chat_snapshot(&snapshot);
        assert!(state.cam_fitted, "the first snapshot fits the map");
        let fitted = state.cam;
        let a = "chat:_general:a";
        let b = "chat:_general:b";
        let (ax, ay) = (state.card(a).unwrap().x, state.card(a).unwrap().y);
        // A manual arrangement survives poll/resize (React keeps auto items'
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
