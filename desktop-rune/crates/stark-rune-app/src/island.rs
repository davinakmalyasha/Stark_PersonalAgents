//! Stark Island: the native top-edge agent notch.
//!
//! The island is a Rune overlay, not an in-app panel. It stays above every
//! application, takes only the pointer inside its own pill (`OverlayInput::
//! Regions`), and is fed from the shell through [`IslandLink`]. Its own state
//! owns hover/leave timing and the view; the shell owns runs and voice.
//!
//! The overlay window is intentionally larger than the notch. The transparent
//! area is click-through, while the notch marks itself with `El::hit()`, so a
//! 520-point window never steals a click from the application underneath.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rune::{col, el, row, text, Cx, El, OverlayHandle, TextEdit, Tx};

use crate::shell::ShellState;

/// The native overlay is larger than any state it draws: the pill grows into
/// it without a native resize, and the clear area stays click-through. The
/// chat surface sets the ceiling (552 × 480 plus breathing room).
pub const ISLAND_WINDOW: (f32, f32) = (620.0, 560.0);

const SHELF_W: f32 = 456.0;
const CHAT_W: f32 = 552.0;
/// Compact on purpose: a scanner + composer, not a full-page chat. Longer
/// transcripts scroll inside the body.
const CHAT_H: f32 = 480.0;

const HAIRLINE: &str = "#ffffff14";
const TEXT: &str = "#f4f4f5";
const MUTED: &str = "#8a8a93";
const RED: &str = "#f87171";
const SPRING: rune::Spring = rune::Spring::new(0.36, 0.16);

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

fn rgba(r: f32, g: f32, b: f32, a: f32) -> rune::Color {
    rune::Color { r, g, b, a }
}

/// A chat/thread the island can open or speak to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IslandTarget {
    /// `main`, `personal`, or a canvas card id.
    pub thread_key: String,
    pub title: String,
    pub card_id: Option<String>,
    pub workspace_id: String,
    pub chat_id: String,
}

impl IslandTarget {
    pub fn display(&self) -> &str {
        if self.title.trim().is_empty() {
            "Stark"
        } else {
            &self.title
        }
    }
}

/// One live agent row in the cockpit.
#[derive(Clone, Debug, PartialEq)]
pub struct IslandRun {
    pub target: IslandTarget,
    pub activity: String,
    pub elapsed: String,
    /// A local run can be stopped from the island; a cloud run only opens.
    pub stoppable: bool,
}

/// What a shelf row opens: a chat, a group project, or a cloud run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IslandKind {
    #[default]
    Chat,
    Project,
    Cloud,
}

/// One row in the activity shelf or in the add menu's existing list. The
/// overlay never reaches into shell state: everything it draws is here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IslandShelfItem {
    /// Unique row key (`thread key`, `project:<id>`, `cloud:<chat>`).
    pub key: String,
    pub kind: IslandKind,
    pub title: String,
    pub subtitle: String,
    pub meta: String,
    pub running: bool,
    pub waiting: u64,
    pub target: IslandTarget,
    /// Group project id (`kind == Project`).
    pub project_id: String,
}

/// One compact transcript row in the in-overlay chat.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IslandChatRow {
    pub key: String,
    pub role: String,
    pub body: String,
    pub at: String,
    pub agent: String,
    pub streaming: bool,
}

/// The one chat the overlay is watching. The shell builds this from the
/// thread the island asked to watch, so a full chat stays one small clone
/// instead of every open thread.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IslandChat {
    pub target: IslandTarget,
    pub rows: Vec<IslandChatRow>,
    pub live: bool,
    pub status: String,
    /// The thread's own composer draft, seeded into the overlay once.
    pub draft: String,
    /// Bumped when a finished utterance replaces that draft, so the overlay
    /// can pick up a voice final while its chat is open.
    pub draft_rev: u64,
}

/// A pending question, a finished/failed run, or a voice draft.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IslandAttentionKind {
    Question,
    Done,
    Error,
    Draft,
}

/// A one-line attention card. `expires_ms == 0` means it stays until the
/// underlying state clears (a pending question or a mic error).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IslandAttention {
    pub kind: IslandAttentionKind,
    pub title: String,
    pub body: String,
    pub target: IslandTarget,
    pub expires_ms: u64,
}

/// The data the shell publishes to the overlay.
#[derive(Clone, Debug, PartialEq)]
pub struct IslandModel {
    pub runs: Vec<IslandRun>,
    /// Active work for the expanded shelf: running chats, active group
    /// projects and cloud runs, newest activity first.
    pub shelf: Vec<IslandShelfItem>,
    /// Everything the radial add menu can attach: open/recent chats, group
    /// projects and cloud runs.
    pub existing: Vec<IslandShelfItem>,
    /// The chat the overlay is watching, if any. `None` shows the shelf.
    pub chat: Option<IslandChat>,
    /// Bumped when the shell creates a chat for the island to open.
    pub focus_seq: u64,
    pub focus: Option<IslandTarget>,
    /// Thread key of the bus currently capturing audio (empty when idle).
    pub voice_thread: String,
    pub listening: bool,
    /// Live mode armed: models warm, island up, mic off. Arming alone
    /// raises the island — capture is a separate, later step.
    pub armed: bool,
    pub level: f32,
    pub voice_text: String,
    pub target: IslandTarget,
    pub attention: Option<IslandAttention>,
    pub auto_send_voice: bool,
    pub island_enabled: bool,
    /// A coarse wall-clock bucket while listening or running, so the overlay
    /// keeps repainting its waveform/elapsed label without a separate clock.
    pub tick_ms: u64,
}

impl Default for IslandModel {
    fn default() -> Self {
        Self {
            runs: Vec::new(),
            shelf: Vec::new(),
            existing: Vec::new(),
            chat: None,
            focus_seq: 0,
            focus: None,
            voice_thread: String::new(),
            listening: false,
            armed: false,
            level: 0.0,
            voice_text: String::new(),
            target: IslandTarget::default(),
            attention: None,
            auto_send_voice: false,
            island_enabled: true,
            tick_ms: 0,
        }
    }
}

/// Actions the overlay asks the shell to perform. The overlay never reaches
/// into shell state directly; this keeps overlay commits small and ordered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IslandCommand {
    StopListen,
    OpenChat(IslandTarget),
    StopRun(String),
    SendDraft(String),
    DismissVoiceNote(String),
    /// The overlay opened a chat: keep a compact transcript for it in the
    /// published model until `UnwatchChat`.
    WatchChat(IslandTarget),
    UnwatchChat,
    /// Send typed overlay text to a thread (creates the chat when needed).
    SendPrompt {
        target: IslandTarget,
        text: String,
    },
    /// The overlay asked for a fresh chat; the shell creates it, then reports
    /// it back through `focus_seq`/`focus`.
    NewChat,
    NewProject,
    NewCloudTask,
    OpenProject(String),
    OpenCloud,
    /// Start/stop capture on a specific thread (the overlay's mic).
    ToggleVoice(String),
    /// Ask the platform for keyboard focus (macOS panels / layer shells).
    FocusKeys,
    /// Windows overlays are click-through and never activate by default. The
    /// chat screen flips this on so its text input can take keys.
    SetActivatable(bool),
    /// The chat pill almost fills the overlay window; capture every click
    /// while it is up so a stale/rounded input region can never leak a click
    /// to the app underneath. The shelf goes back to `Regions`.
    SetFullInput(bool),
}

/// Shared between the shell and the overlay runtime.
pub struct IslandShared {
    pub model: Mutex<IslandModel>,
    pub revision: AtomicU64,
    pub commands: Mutex<VecDeque<IslandCommand>>,
    pub app: Option<rune::AppHandle<ShellState>>,
}

impl IslandShared {
    pub fn new(app: Option<rune::AppHandle<ShellState>>) -> Self {
        Self {
            model: Mutex::new(IslandModel::default()),
            revision: AtomicU64::new(0),
            commands: Mutex::new(VecDeque::new()),
            app,
        }
    }

    pub fn push(&self, command: IslandCommand) {
        if let Ok(mut queue) = self.commands.lock() {
            queue.push_back(command);
        }
        if let Some(app) = &self.app {
            app.wake();
        }
    }

    pub fn take_commands(&self) -> Vec<IslandCommand> {
        self.commands
            .lock()
            .map(|mut queue| queue.drain(..).collect())
            .unwrap_or_default()
    }

    fn bump(&self) {
        self.revision.fetch_add(1, Ordering::Relaxed);
    }
}

/// The add control's popover. `Radial` is the three satellite buttons;
/// `Type` offers new/existing for one kind; `Existing` lists what can be
/// attached. Hover opens the radial; a selection closes the whole menu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AddMenu {
    #[default]
    Closed,
    Radial,
    Type(IslandKind),
    Existing(IslandKind),
}

/// The overlay's own state. Hover is intentionally local so pointer travel
/// never rebuilds the whole shell.
pub struct IslandState {
    pub shared: Arc<IslandShared>,
    pub hovered: bool,
    pub collapse_at: Option<f64>,
    /// The chat surface is open and pinned: a pointer leave must not collapse
    /// it while the user reads or types.
    pub open_chat: Option<IslandTarget>,
    /// Recently opened chats, newest last (capped at three). Tabs let the
    /// overlay switch between concurrent work without going back to the list.
    pub tabs: Vec<IslandTarget>,
    /// One composer draft per open chat.
    pub composers: HashMap<String, TextEdit>,
    pub add_menu: AddMenu,
    seen_revision: u64,
    seen_focus: u64,
    /// Shell draft already seeded into a chat composer (never overwrite what
    /// the user has typed since).
    seeded_draft: Option<String>,
    seen_draft_rev: Option<u64>,
    last_chat_draft: String,
}

impl IslandState {
    pub fn new(shared: Arc<IslandShared>) -> Self {
        Self {
            shared,
            hovered: false,
            collapse_at: None,
            open_chat: None,
            tabs: Vec::new(),
            composers: HashMap::new(),
            add_menu: AddMenu::Closed,
            seen_revision: 0,
            seen_focus: 0,
            seeded_draft: None,
            seen_draft_rev: None,
            last_chat_draft: String::new(),
        }
    }

    pub fn composer(&mut self, key: &str) -> &mut TextEdit {
        self.composers.entry(key.to_string()).or_default()
    }

    /// Open a chat and pin the surface. Watching is a shell round-trip: the
    /// next model publish carries that thread's compact transcript.
    pub fn enter_chat(&mut self, target: IslandTarget) {
        if !self
            .tabs
            .iter()
            .any(|tab| tab.thread_key == target.thread_key)
        {
            self.tabs.push(target.clone());
            // Three concurrent chats is the compact surface's honest limit.
            if self.tabs.len() > 3 {
                self.tabs.remove(0);
            }
        }
        self.open_chat = Some(target.clone());
        self.add_menu = AddMenu::Closed;
        self.composer(&target.thread_key);
        self.seeded_draft = None;
        self.seen_draft_rev = None;
        self.last_chat_draft.clear();
        self.shared.push(IslandCommand::WatchChat(target));
        self.shared.push(IslandCommand::FocusKeys);
        self.shared.push(IslandCommand::SetActivatable(true));
        self.shared.push(IslandCommand::SetFullInput(true));
    }

    pub fn leave_chat(&mut self) {
        self.open_chat = None;
        self.add_menu = AddMenu::Closed;
        self.seeded_draft = None;
        self.seen_draft_rev = None;
        self.shared.push(IslandCommand::UnwatchChat);
        self.shared.push(IslandCommand::SetActivatable(false));
        self.shared.push(IslandCommand::SetFullInput(false));
    }

    pub fn switch_chat(&mut self, target: IslandTarget) {
        if self.open_chat.as_ref().map(|open| &open.thread_key) == Some(&target.thread_key) {
            return;
        }
        self.open_chat = Some(target.clone());
        self.composer(&target.thread_key);
        self.seeded_draft = None;
        self.seen_draft_rev = None;
        self.last_chat_draft.clear();
        self.shared.push(IslandCommand::WatchChat(target));
        self.shared.push(IslandCommand::FocusKeys);
    }

    fn send_chat(&mut self, running: bool) {
        let Some(target) = self.open_chat.clone() else {
            return;
        };
        if running {
            self.shared.push(IslandCommand::StopRun(target.thread_key));
            return;
        }
        let text = self
            .composers
            .get(&target.thread_key)
            .map(|edit| edit.text.trim().to_string())
            .unwrap_or_default();
        if text.is_empty() {
            return;
        }
        if let Some(edit) = self.composers.get_mut(&target.thread_key) {
            edit.text.clear();
            edit.anchor = 0;
            edit.focus = 0;
        }
        self.shared.push(IslandCommand::SendPrompt { target, text });
    }
}

/// Handle kept by `ShellState`; publishes models and shows/hides the surface.
#[derive(Clone)]
pub struct IslandLink {
    shared: Arc<IslandShared>,
    handle: OverlayHandle<IslandState>,
}

impl std::fmt::Debug for IslandLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IslandLink")
    }
}

impl IslandLink {
    pub fn new(shared: Arc<IslandShared>, handle: OverlayHandle<IslandState>) -> Self {
        Self { shared, handle }
    }

    /// Push only real changes; a voice level at 60 Hz is a real change while
    /// the mic is live, while a settled idle island wakes nothing.
    pub fn publish(&self, model: IslandModel) {
        let changed = match self.shared.model.lock() {
            Ok(mut current) => {
                if *current == model {
                    false
                } else {
                    *current = model;
                    true
                }
            }
            Err(_) => false,
        };
        if changed {
            self.shared.bump();
            self.handle.update(|_, tx| tx.animated());
        }
    }

    pub fn show(&self) {
        self.handle.show();
    }

    pub fn hide(&self) {
        self.handle.hide();
    }

    pub fn set_capture_exclusion(&self, exclude: bool) {
        self.handle.set_exclude_from_capture(exclude);
    }

    /// macOS panels and layer-shell overlays need an explicit keyboard
    /// request; Windows uses `set_activatable` instead.
    pub fn request_keys(&self) {
        self.handle.request_keys();
    }

    pub fn set_activatable(&self, active: bool) {
        self.handle.set_activatable(active);
    }

    pub fn set_input(&self, input: rune::OverlayInput) {
        self.handle.set_input(input);
    }

    pub fn take_commands(&self) -> Vec<IslandCommand> {
        self.shared.take_commands()
    }
}

pub fn island_poll(s: &mut IslandState, _clock: f64, tx: &mut Tx) -> bool {
    let revision = s.shared.revision.load(Ordering::Relaxed);
    let mut changed = false;
    let model = s.shared.model.lock().map(|m| m.clone()).unwrap_or_default();
    if revision != s.seen_revision {
        s.seen_revision = revision;
        changed = true;
    }
    // A new chat created from the add menu comes back as a focus target.
    if model.focus_seq != s.seen_focus {
        s.seen_focus = model.focus_seq;
        if let Some(target) = model.focus.clone() {
            s.enter_chat(target);
            changed = true;
        }
    }
    // A fresh chat gets its real server key after the first send. The shell
    // republishes the watched chat under that key; adopt it and carry the
    // tab + draft across so the overlay never drops the conversation.
    if let (Some(open), Some(chat)) = (s.open_chat.clone(), model.chat.as_ref()) {
        if chat.target.thread_key != open.thread_key {
            if let Some(tab) = s
                .tabs
                .iter_mut()
                .find(|tab| tab.thread_key == open.thread_key)
            {
                *tab = chat.target.clone();
            }
            if let Some(edit) = s.composers.remove(&open.thread_key) {
                s.composers
                    .entry(chat.target.thread_key.clone())
                    .or_insert(edit);
            }
            s.open_chat = Some(chat.target.clone());
            s.seeded_draft = None;
            s.seen_draft_rev = None;
            s.last_chat_draft.clear();
            changed = true;
        }
    }
    // A chat's title follows its first message. Keep the header and its tab
    // reading the live title instead of the draft-time "New Chat".
    if let (Some(open), Some(chat)) = (s.open_chat.as_mut(), model.chat.as_ref()) {
        if chat.target.thread_key == open.thread_key && chat.target.title != open.title {
            *open = chat.target.clone();
            if let Some(tab) = s
                .tabs
                .iter_mut()
                .find(|tab| tab.thread_key == chat.target.thread_key)
            {
                *tab = chat.target.clone();
            }
            changed = true;
        }
    }
    // Seed the overlay composer with the shell thread's draft once per open,
    // then follow voice finals that replace that draft while the chat is up.
    if let Some(open) = s.open_chat.clone() {
        if let Some(chat) = model
            .chat
            .as_ref()
            .filter(|chat| chat.target.thread_key == open.thread_key)
        {
            let first = s.seeded_draft.as_deref() != Some(open.thread_key.as_str());
            let fresh = first || s.seen_draft_rev != Some(chat.draft_rev);
            if fresh {
                let last = s.last_chat_draft.clone();
                let edit = s.composer(&open.thread_key);
                // Replace an untouched voice draft; never clobber words the
                // user typed over it.
                if edit.text.is_empty() || edit.text == last {
                    edit.text = chat.draft.clone();
                    edit.anchor = edit.text.len();
                    edit.focus = edit.text.len();
                }
                s.seeded_draft = Some(open.thread_key.clone());
                s.seen_draft_rev = Some(chat.draft_rev);
                s.last_chat_draft = chat.draft.clone();
                changed = true;
            }
        }
    }
    if let Some(at) = s.collapse_at {
        // The overlay poll clock is elapsed-since-start; hover deadlines use
        // the same wall clock as the handler so a leave always lands.
        let now = crate::clock::now_millis() as f64 / 1000.0;
        if now >= at {
            s.collapse_at = None;
            // Hover clears even while a chat is pinned, so leaving the chat
            // returns to a shelf that collapses on the next pointer leave.
            if s.hovered {
                s.hovered = false;
                changed = true;
            }
        } else {
            tx.wake_after(Duration::from_secs_f64((at - now).max(0.01)));
        }
    }
    changed
}

/// The Rune overlay spec. `prewarm` makes the first reveal instant; the
/// window is transparent and excludes itself from the taskbar.
pub fn island_spec() -> impl Into<rune::ConfiguredOverlay<IslandState>> {
    let mut spec = rune::OverlaySpec::new("Stark Island", ISLAND_WINDOW, island_view);
    spec.interactive = true;
    spec.options(rune::OverlayOptions {
        input: rune::OverlayInput::Regions,
        level: rune::OverlayLevel::AboveOverlays,
        // The in-overlay chat composer takes keys, but only while its screen
        // is up. `SetActivatable` keeps ordinary pill clicks non-activating.
        keys: rune::KeyPolicy::OnDemand,
        prewarm: true,
        ..Default::default()
    })
    .poll(island_poll)
    .on_key(island_key)
    .animate_reveal()
    // Live mode is bursty: once the island hides after a draft/utterance,
    // give the GPU surface back so an idle app holds no second surface.
    .release_gpu_after(std::time::Duration::from_secs(20))
}

fn island_key(s: &mut IslandState, key: &rune::KeyEvent, _tx: &mut Tx) -> bool {
    if key.key != rune::Key::Escape {
        return false;
    }
    if s.add_menu != AddMenu::Closed {
        s.add_menu = AddMenu::Closed;
        return true;
    }
    if s.open_chat.is_some() {
        s.leave_chat();
        return true;
    }
    false
}

fn hover_handler(s: &mut IslandState, inside: bool, tx: &mut Tx) {
    if inside {
        s.hovered = true;
        s.collapse_at = None;
    } else if s.hovered {
        // A short grace lets the pointer cross the rounded edge without the
        // cockpit snapping shut under it. A pinned chat ignores the leave.
        s.add_menu = AddMenu::Closed;
        s.collapse_at = Some(crate::clock::now_millis() as f64 / 1000.0 + 0.16);
        tx.wake_after(Duration::from_millis(180));
    }
}

fn open_cmd(target: &IslandTarget) -> IslandCommand {
    IslandCommand::OpenChat(target.clone())
}

fn action_button(
    key: &str,
    label: &str,
    fill: rune::Color,
    glyph: El,
    press: impl Fn(&mut IslandState) + 'static,
) -> El {
    el().key(key)
        .role("button")
        .label(label)
        .h(26.0)
        .min_w(26.0)
        .radius(13.0)
        .fill(fill)
        .center()
        .hover(rgba(1.0, 1.0, 1.0, 0.10))
        .on_press(move |s: &mut IslandState| press(s))
        .child(glyph)
}

fn tiny_label(key: &str, value: &str, size: f32, weight: f32, color: rune::Color) -> El {
    text(value.to_string())
        .key(key.to_string())
        .truncate()
        .max_w(200.0)
        .font(rune::tok::TextStyle::new(size, weight))
        .color(color)
}

fn mic_glyph(size: f32, color: rune::Color) -> El {
    el().size(size, size).inert().shape(crate::icons::lucide(
        &[
            "M12 19v3",
            "M19 10v2a7 7 0 0 1-14 0v-2",
            "M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3z",
        ],
        size,
        2.0,
        color,
    ))
}

fn stop_glyph(size: f32, color: rune::Color) -> El {
    el().size(size, size)
        .inert()
        .child(crate::lucide_square_filled(size, color))
}

fn x_glyph(size: f32, color: rune::Color) -> El {
    el().size(size, size)
        .inert()
        .shape(crate::lucide_x(size, 2.0, color))
}

fn check_glyph(size: f32, color: rune::Color) -> El {
    el().size(size, size)
        .inert()
        .shape(crate::lucide_check(size, 2.0, color))
}

fn alert_glyph(size: f32, color: rune::Color) -> El {
    el()
        .size(size, size)
        .inert()
        .shape(crate::icons::lucide(
            &[
                "M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z",
                "M12 9v4",
                "M12 17h.01",
            ],
            size,
            2.0,
            color,
        ))
}

fn help_glyph(size: f32, color: rune::Color) -> El {
    el().size(size, size).inert().shape(crate::icons::lucide(
        &[
            "M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3",
            "M12 17h.01",
            "M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0z",
        ],
        size,
        2.0,
        color,
    ))
}

fn arrow_glyph(size: f32, color: rune::Color) -> El {
    el().size(size, size).inert().shape(crate::icons::lucide(
        &["M7 17 17 7", "M7 7h10v10"],
        size,
        2.0,
        color,
    ))
}

fn live_mark(color: rune::Color) -> El {
    el().key("island-mark")
        .size(19.0, 19.0)
        .inert()
        .shape(crate::shell::icon_live(19.0, color))
}

fn pulse_dot(size: f32, color: rune::Color, animate: bool) -> El {
    let mut dot = el().size(size, size).radius(size / 2.0).fill(color).inert();
    if animate {
        dot = dot.looping(rune::LoopSpec::opacity(2.0, 0.0, 0.42, 1.0));
    }
    dot
}

fn waveform(model: &IslandModel, width: f32, height: f32) -> El {
    let now = model.tick_ms as f32 / 1000.0;
    // While listening, keep a visible idle wave even before the first level
    // packet: the island should read as alive, not as a dead dotted line.
    let amount = if model.listening {
        model.level.clamp(0.0, 1.0).max(0.38)
    } else {
        model.level.clamp(0.0, 1.0)
    };
    let mut bars = row()
        .key("island-wave")
        .w(width)
        .h(height)
        .items_center()
        .justify_center()
        .gap(2.0)
        .inert();
    // A soft centre-weighted envelope plus a travelling phase. The envelope
    // guarantees visible peaks even before the first level packet; the phase
    // moves with the model tick, and each bar also breathes on its own loop so
    // the listening state reads alive even on a quiet microphone.
    const ENVELOPE: [f32; 7] = [0.55, 0.80, 1.0, 0.90, 0.68, 0.45, 0.62];
    for (i, envelope) in ENVELOPE.iter().enumerate() {
        let phase = (now * 7.0 + i as f32 * 0.82).sin() * 0.5 + 0.5;
        let bar_h =
            (4.0 + (height - 6.0) * amount * envelope * (0.55 + 0.45 * phase)).clamp(4.0, height);
        let dim = if amount <= 0.01 {
            0.35
        } else {
            0.65 + amount * 0.35
        };
        bars = bars.child(
            el().key(format!("island-wave-bar-{i}"))
                .size(3.0, bar_h)
                .radius(1.5)
                .fill(ink(TEXT).alpha(dim))
                .looping(rune::LoopSpec::opacity(
                    1.15 + i as f64 * 0.06,
                    i as f64 * 0.11,
                    0.42,
                    1.0,
                ))
                .inert(),
        );
    }
    bars
}

fn attention_icon(kind: IslandAttentionKind) -> El {
    match kind {
        IslandAttentionKind::Done => check_glyph(14.0, ink(TEXT)),
        IslandAttentionKind::Draft => mic_glyph(14.0, ink(TEXT)),
        IslandAttentionKind::Error => alert_glyph(14.0, ink(RED)),
        IslandAttentionKind::Question => help_glyph(14.0, ink(TEXT)),
    }
}

fn plus_glyph(size: f32, color: rune::Color) -> El {
    el().size(size, size).inert().shape(crate::icons::lucide(
        &["M12 5v14", "M5 12h14"],
        size,
        2.0,
        color,
    ))
}

fn back_glyph(size: f32, color: rune::Color) -> El {
    el().size(size, size)
        .inert()
        .shape(crate::icons::lucide(&["m15 18-6-6 6-6"], size, 2.0, color))
}

fn send_glyph(size: f32, color: rune::Color) -> El {
    el().size(size, size).inert().shape(crate::icons::lucide(
        &["M14.536 21.686a.5.5 0 0 0 .937-.024l6.5-19a.496.496 0 0 0-.635-.635l-19 6.5a.5.5 0 0 0-.024.937l7.93 3.18a2 2 0 0 1 1.112 1.11z", "m21.854 2.147-10.94 10.939"],
        size,
        2.0,
        color,
    ))
}

fn chat_glyph(size: f32, color: rune::Color) -> El {
    el().size(size, size).inert().shape(crate::icons::lucide(
        &["M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"],
        size,
        2.0,
        color,
    ))
}

fn users_glyph(size: f32, color: rune::Color) -> El {
    el().size(size, size).inert().shape(crate::icons::lucide(
        &[
            "M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2",
            "M16 3.128a4 4 0 0 1 0 7.744",
            "M22 21v-2a4 4 0 0 0-3-3.87",
            "M9 7a4 4 0 1 0 0 8 4 4 0 0 0 0-8z",
        ],
        size,
        2.0,
        color,
    ))
}

fn cloud_glyph(size: f32, color: rune::Color) -> El {
    el().size(size, size)
        .inert()
        .shape(crate::icons::cloud(size, color))
}

fn kind_glyph(kind: IslandKind, size: f32, color: rune::Color) -> El {
    match kind {
        IslandKind::Chat => chat_glyph(size, color),
        IslandKind::Project => users_glyph(size, color),
        IslandKind::Cloud => cloud_glyph(size, color),
    }
}

fn kind_label(kind: IslandKind) -> &'static str {
    match kind {
        IslandKind::Chat => "Chat",
        IslandKind::Project => "Group project",
        IslandKind::Cloud => "Cloud task",
    }
}

fn kind_section(kind: IslandKind) -> &'static str {
    match kind {
        IslandKind::Chat => "Chats",
        IslandKind::Project => "Projects",
        IslandKind::Cloud => "Cloud",
    }
}

/// Small text-only "control" used inside the chat surface.
fn icon_button(
    key: &str,
    label: &str,
    size: f32,
    fill: rune::Color,
    glyph: El,
    press: impl Fn(&mut IslandState) + 'static,
) -> El {
    el().key(key)
        .role("button")
        .focusable(true)
        .label(label)
        .size(size, size)
        .radius(size / 2.0)
        .fill(fill)
        .center()
        .hover(rgba(1.0, 1.0, 1.0, 0.12))
        .on_press(move |s: &mut IslandState| press(s))
        .child(glyph)
}

/// Compact mark + count header for the collapsed pill, the listening card and
/// the attention card. The expanded shelf has its own header.
fn compact_header(
    model: &IslandModel,
    listening: bool,
    attention: Option<&IslandAttention>,
    height: f32,
) -> El {
    let mut head = row()
        .key("island-header")
        .w_full()
        .h(height)
        .px(12.0)
        .items_center()
        .gap(8.0)
        .on_press(|s: &mut IslandState| {
            let target = s
                .shared
                .model
                .lock()
                .map(|m| m.target.clone())
                .unwrap_or_default();
            s.shared.push(open_cmd(&target));
        });

    if listening {
        head = head
            .child(waveform(model, 58.0, 24.0))
            .child(tiny_label(
                "island-listening-label",
                "Listening",
                12.0,
                600.0,
                ink(TEXT),
            ))
            .child(
                tiny_label(
                    "island-listening-target",
                    model.target.display(),
                    10.5,
                    400.0,
                    ink(MUTED),
                )
                .max_w(130.0),
            )
            .child(rune::el().grow(1.0))
            .child(action_button(
                "island-voice-stop",
                "Stop listening",
                rgba(1.0, 1.0, 1.0, 0.10),
                stop_glyph(9.0, ink(TEXT)),
                |s| s.shared.push(IslandCommand::StopListen),
            ));
        return head;
    }

    if let Some(attention) = attention {
        let dismiss = attention.kind == IslandAttentionKind::Error && attention.expires_ms == 0;
        let send = attention.kind == IslandAttentionKind::Draft;
        let action = if dismiss {
            "Dismiss"
        } else if send {
            "Send"
        } else {
            "Open"
        };
        let target = attention.target.clone();
        head = head
            .child(
                el().size(22.0, 22.0)
                    .center()
                    .inert()
                    .child(attention_icon(attention.kind)),
            )
            .child(
                col()
                    .grow(1.0)
                    .min_w(0.0)
                    .gap(1.0)
                    .child(tiny_label(
                        "island-attention-title",
                        &attention.title,
                        12.0,
                        600.0,
                        ink(TEXT),
                    ))
                    .child(tiny_label(
                        "island-attention-body",
                        &attention.body,
                        10.5,
                        400.0,
                        ink(MUTED),
                    )),
            )
            .child(action_button(
                "island-attention-action",
                action,
                rgba(1.0, 1.0, 1.0, 0.08),
                if dismiss {
                    x_glyph(11.0, ink(TEXT))
                } else {
                    arrow_glyph(12.0, ink(TEXT))
                },
                move |s| {
                    if dismiss {
                        s.shared
                            .push(IslandCommand::DismissVoiceNote(target.thread_key.clone()));
                    } else if send {
                        s.shared
                            .push(IslandCommand::SendDraft(target.thread_key.clone()));
                    } else {
                        s.shared.push(open_cmd(&target));
                    }
                },
            ));
        return head;
    }

    head = head.child(live_mark(ink("#c7c7cf")));
    if !model.runs.is_empty() {
        head = head
            .child(
                tiny_label(
                    "island-run-count",
                    &format!("{} running", model.runs.len()),
                    11.0,
                    600.0,
                    ink(TEXT),
                )
                .max_w(130.0),
            )
            .child(rune::el().grow(1.0))
            .child(pulse_dot(6.0, ink(TEXT), true));
    } else {
        head = head
            .child(rune::el().grow(1.0))
            .child(pulse_dot(5.0, ink("#5a5a62"), false));
    }
    head
}

fn shelf_height(model: &IslandModel, menu: AddMenu) -> f32 {
    let list = if model.shelf.is_empty() {
        64.0
    } else {
        // Three 54 pt rows at a time; the rest scroll inside the shelf.
        (model.shelf.len().min(3) as f32 * 54.0 + 4.0).min(166.0) + 8.0
    };
    let attention = if model.attention.is_some() { 54.0 } else { 0.0 };
    let more = if model.shelf.len() > 3 { 16.0 } else { 0.0 };
    let base = 44.0 + list + more + attention + 8.0;
    // The add popover floats inside the pill: give it room instead of
    // spilling a satellite or its label past the rounded edge.
    let menu = match menu {
        AddMenu::Closed => 0.0,
        AddMenu::Radial => 168.0,
        AddMenu::Type(_) => 202.0,
        AddMenu::Existing(_) => 324.0,
    };
    base.max(menu)
}

fn shelf_row(item: &IslandShelfItem) -> El {
    let target = item.target.clone();
    let kind = item.kind;
    let key = item.key.clone();
    let project_id = item.project_id.clone();
    let mut row = row()
        .key(format!("island-shelf-{key}"))
        .w_full()
        .h(54.0)
        .px(12.0)
        .items_center()
        .gap(10.0)
        .radius(10.0)
        .hover(rgba(1.0, 1.0, 1.0, 0.05))
        .role("button")
        .focusable(true)
        .label(format!("Open {}", item.title))
        .on_press(move |s: &mut IslandState| match kind {
            IslandKind::Chat => s.enter_chat(target.clone()),
            IslandKind::Project => s
                .shared
                .push(IslandCommand::OpenProject(project_id.clone())),
            IslandKind::Cloud => s.shared.push(IslandCommand::OpenCloud),
        })
        .child(
            el().size(30.0, 30.0)
                .radius(9.0)
                .fill(rgba(1.0, 1.0, 1.0, 0.045))
                .center()
                .inert()
                .child(kind_glyph(
                    kind,
                    14.0,
                    ink(if item.running { TEXT } else { "#9b9ba5" }),
                )),
        )
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(2.0)
                .child(tiny_label(
                    "island-shelf-title",
                    &item.title,
                    12.0,
                    600.0,
                    ink(TEXT),
                ))
                .child(tiny_label(
                    "island-shelf-subtitle",
                    &item.subtitle,
                    10.0,
                    400.0,
                    ink(MUTED),
                )),
        );
    if !item.meta.is_empty() {
        row = row.child(
            text(item.meta.clone())
                .key("island-shelf-meta")
                .font(rune::tok::TextStyle::new(
                    9.5,
                    if item.waiting > 0 { 600.0 } else { 400.0 },
                ))
                .color(ink(if item.waiting > 0 { TEXT } else { "#777782" })),
        );
    }
    if item.running {
        row = row.child(pulse_dot(5.0, ink(TEXT), kind == IslandKind::Chat));
    }
    row
}

fn attention_shelf_row(attention: &IslandAttention) -> El {
    let target = attention.target.clone();
    let dismiss_key = attention.target.thread_key.clone();
    let dismiss = attention.kind == IslandAttentionKind::Error && attention.expires_ms == 0;
    let send = attention.kind == IslandAttentionKind::Draft;
    let mut row = row()
        .key("island-attention-row")
        .w_full()
        .h(48.0)
        .mx(8.0)
        .px(10.0)
        .items_center()
        .gap(8.0)
        .radius(10.0)
        .fill(rgba(1.0, 1.0, 1.0, 0.025))
        .on_press(move |s: &mut IslandState| s.shared.push(open_cmd(&target)))
        .child(
            el().size(18.0, 18.0)
                .center()
                .inert()
                .child(attention_icon(attention.kind)),
        )
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(1.0)
                .child(tiny_label(
                    "island-attention-row-title",
                    &attention.title,
                    11.5,
                    600.0,
                    ink(TEXT),
                ))
                .child(tiny_label(
                    "island-attention-row-body",
                    &attention.body,
                    10.0,
                    400.0,
                    ink(MUTED),
                )),
        );
    if dismiss {
        row = row.child(action_button(
            "island-attention-dismiss",
            "Dismiss",
            rgba(1.0, 1.0, 1.0, 0.08),
            x_glyph(10.0, ink(TEXT)),
            move |s| {
                s.shared
                    .push(IslandCommand::DismissVoiceNote(dismiss_key.clone()));
            },
        ));
    } else if send {
        row = row.child(action_button(
            "island-attention-send",
            "Send voice draft",
            rgba(1.0, 1.0, 1.0, 0.08),
            arrow_glyph(12.0, ink(TEXT)),
            move |s| {
                s.shared.push(IslandCommand::SendDraft(dismiss_key.clone()));
            },
        ));
    } else {
        row = row.child(arrow_glyph(12.0, ink(MUTED)));
    }
    row
}

fn shelf_header(model: &IslandModel) -> El {
    let active = model
        .shelf
        .iter()
        .filter(|item| item.running || item.waiting > 0)
        .count();
    let copy = if model.shelf.is_empty() {
        "Quiet".to_string()
    } else if active > 0 {
        format!("{active} active")
    } else {
        format!("{} open", model.shelf.len())
    };
    row()
        .key("island-shelf-header")
        .w_full()
        .h(44.0)
        .px(14.0)
        .items_center()
        .gap(8.0)
        .child(pulse_dot(
            5.0,
            ink(if active > 0 { TEXT } else { "#5a5a62" }),
            active > 0,
        ))
        .child(tiny_label(
            "island-shelf-status",
            &copy,
            11.0,
            600.0,
            ink(if active > 0 { TEXT } else { MUTED }),
        ))
        .child(rune::el().grow(1.0))
        // The add control floats above this corner; reserve its hit area.
        .child(rune::el().size(30.0, 30.0).inert())
}

fn shelf_body(model: &IslandModel) -> El {
    let mut body = col().key("island-shelf").w_full().gap(2.0).px(4.0);
    if model.shelf.is_empty() {
        body = body.child(
            col()
                .key("island-shelf-empty")
                .w_full()
                .h(64.0)
                .items_center()
                .justify_center()
                .gap(3.0)
                .child(
                    el().size(18.0, 18.0)
                        .inert()
                        .shape(crate::shell::icon_live(18.0, ink("#66666f"))),
                )
                .child(tiny_label(
                    "island-shelf-empty-title",
                    "No active work",
                    11.5,
                    600.0,
                    ink("#a1a1aa"),
                ))
                .child(tiny_label(
                    "island-shelf-empty-hint",
                    "Hover the + to start something",
                    9.5,
                    400.0,
                    ink("#66666f"),
                )),
        );
    } else {
        let mut list = col()
            .key("island-shelf-list")
            .w_full()
            .gap(2.0)
            .max_h(166.0)
            .scroll(rune::ScrollSpec {
                stick_bottom: false,
            });
        for item in &model.shelf {
            list = list.child(shelf_row(item));
        }
        body = body.child(list);
        let more = model.shelf.len().saturating_sub(3);
        if more > 0 {
            body = body.child(
                row()
                    .key("island-shelf-more")
                    .w_full()
                    .h(16.0)
                    .px(14.0)
                    .items_center()
                    .child(
                        text(format!("+{more} more · scroll"))
                            .font(rune::tok::TextStyle::new(9.0, 500.0))
                            .color(ink("#66666f")),
                    ),
            );
        }
    }
    if let Some(attention) = &model.attention {
        body = body.child(attention_shelf_row(attention));
    }
    body
}

fn existing_count(model: &IslandModel, kind: IslandKind) -> usize {
    model
        .existing
        .iter()
        .filter(|item| item.kind == kind)
        .count()
}

fn add_new(s: &mut IslandState, kind: IslandKind) {
    s.add_menu = AddMenu::Closed;
    match kind {
        IslandKind::Chat => s.shared.push(IslandCommand::NewChat),
        IslandKind::Project => s.shared.push(IslandCommand::NewProject),
        IslandKind::Cloud => s.shared.push(IslandCommand::NewCloudTask),
    }
}

fn add_type_row(
    key: &str,
    title: &str,
    subtitle: &str,
    glyph: El,
    muted: bool,
    press: impl Fn(&mut IslandState) + 'static,
) -> El {
    row()
        .key(key.to_string())
        .w_full()
        .h(42.0)
        .px(9.0)
        .items_center()
        .gap(9.0)
        .radius(8.0)
        .hover(rgba(1.0, 1.0, 1.0, 0.06))
        .role("button")
        .focusable(true)
        .label(title.to_string())
        .on_press(move |s: &mut IslandState| press(s))
        .child(
            el().size(24.0, 24.0)
                .radius(7.0)
                .fill(rgba(1.0, 1.0, 1.0, 0.045))
                .center()
                .inert()
                .child(glyph),
        )
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(1.0)
                .child(tiny_label(
                    "island-add-row-title",
                    title,
                    11.5,
                    600.0,
                    ink(if muted { MUTED } else { TEXT }),
                ))
                .child(tiny_label(
                    "island-add-row-subtitle",
                    subtitle,
                    9.5,
                    400.0,
                    ink("#777782"),
                )),
        )
}

fn add_type_panel(model: &IslandModel, kind: IslandKind) -> El {
    let count = existing_count(model, kind);
    let (new_title, new_sub) = match kind {
        IslandKind::Chat => ("New chat", "Start a fresh conversation"),
        IslandKind::Project => ("New group project", "Open the project builder"),
        IslandKind::Cloud => ("New cloud task", "Open the cloud board"),
    };
    let (existing_title, existing_sub) = match kind {
        IslandKind::Chat => (
            "Existing chats",
            if count == 1 {
                "1 available".to_string()
            } else {
                format!("{count} available")
            },
        ),
        IslandKind::Project => (
            "Active projects",
            if count == 1 {
                "1 available".to_string()
            } else {
                format!("{count} available")
            },
        ),
        IslandKind::Cloud => (
            "Running tasks",
            if count == 1 {
                "1 available".to_string()
            } else {
                format!("{count} available")
            },
        ),
    };
    col()
        .key("island-add-panel")
        .w_full()
        .gap(2.0)
        .radius(14.0)
        .fill(ink("#141417"))
        .border(1.0, ink("#ffffff1c"))
        .border_layout()
        .px(7.0)
        .py(7.0)
        .box_shadow(rune::Shadow {
            x: 0.0,
            y: 14.0,
            blur: 34.0,
            spread: -6.0,
            color: rgba(0.0, 0.0, 0.0, 0.72),
            inset: false,
        })
        .child(
            row()
                .key("island-add-head")
                .w_full()
                .h(28.0)
                .px(7.0)
                .items_center()
                .gap(7.0)
                .child(
                    el().size(14.0, 14.0)
                        .inert()
                        .child(kind_glyph(kind, 14.0, ink("#a1a1aa"))),
                )
                .child(
                    text(kind_label(kind).to_string())
                        .font(rune::tok::TextStyle::new(11.5, 600.0))
                        .color(ink(TEXT)),
                )
                .child(rune::el().grow(1.0))
                .child(icon_button(
                    "island-add-close",
                    "Close add menu",
                    22.0,
                    rgba(1.0, 1.0, 1.0, 0.0),
                    x_glyph(11.0, ink(MUTED)),
                    |s| s.add_menu = AddMenu::Closed,
                )),
        )
        .child(add_type_row(
            "island-add-new",
            new_title,
            new_sub,
            plus_glyph(13.0, ink(TEXT)),
            false,
            move |s| add_new(s, kind),
        ))
        .child(add_type_row(
            "island-add-existing",
            existing_title,
            &existing_sub,
            kind_glyph(kind, 13.0, ink(if count > 0 { TEXT } else { MUTED })),
            count == 0,
            move |s| s.add_menu = AddMenu::Existing(kind),
        ))
}

fn add_existing_row(item: &IslandShelfItem) -> El {
    let kind = item.kind;
    let target = item.target.clone();
    let project_id = item.project_id.clone();
    row()
        .key(format!("island-add-row-{}", item.key))
        .w_full()
        .h(40.0)
        .px(7.0)
        .items_center()
        .gap(8.0)
        .radius(7.0)
        .hover(rgba(1.0, 1.0, 1.0, 0.06))
        .role("button")
        .focusable(true)
        .label(format!("Add {}", item.title))
        .on_press(move |s: &mut IslandState| {
            s.add_menu = AddMenu::Closed;
            match kind {
                IslandKind::Chat => s.enter_chat(target.clone()),
                IslandKind::Project => s
                    .shared
                    .push(IslandCommand::OpenProject(project_id.clone())),
                IslandKind::Cloud => s.shared.push(IslandCommand::OpenCloud),
            }
        })
        .child(
            el().size(22.0, 22.0)
                .radius(6.0)
                .fill(rgba(1.0, 1.0, 1.0, 0.04))
                .center()
                .inert()
                .child(kind_glyph(kind, 11.0, ink(MUTED))),
        )
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(1.0)
                .child(tiny_label(
                    "island-add-item-title",
                    &item.title,
                    10.5,
                    600.0,
                    ink(TEXT),
                ))
                .child(tiny_label(
                    "island-add-item-subtitle",
                    &item.subtitle,
                    9.0,
                    400.0,
                    ink("#777782"),
                )),
        )
        .child(
            text(item.meta.clone())
                .font(rune::tok::TextStyle::new(9.0, 500.0))
                .color(ink("#66666f")),
        )
}

fn add_existing_panel(model: &IslandModel, kind: IslandKind) -> El {
    let items: Vec<&IslandShelfItem> = model
        .existing
        .iter()
        .filter(|item| item.kind == kind)
        .collect();
    let mut list = col()
        .key("island-add-list")
        .w_full()
        .gap(1.0)
        .max_h(216.0)
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        });
    if items.is_empty() {
        list = list.child(
            col()
                .key("island-add-empty")
                .w_full()
                .h(54.0)
                .items_center()
                .justify_center()
                .child(tiny_label(
                    "island-add-empty-title",
                    "Nothing to add yet",
                    10.5,
                    500.0,
                    ink(MUTED),
                )),
        );
    } else {
        for item in items {
            list = list.child(add_existing_row(item));
        }
    }
    col()
        .key("island-add-panel")
        .w_full()
        .gap(4.0)
        .radius(14.0)
        .fill(ink("#141417"))
        .border(1.0, ink("#ffffff1c"))
        .border_layout()
        .px(7.0)
        .py(7.0)
        .box_shadow(rune::Shadow {
            x: 0.0,
            y: 14.0,
            blur: 34.0,
            spread: -6.0,
            color: rgba(0.0, 0.0, 0.0, 0.72),
            inset: false,
        })
        .child(
            row()
                .key("island-add-head")
                .w_full()
                .h(28.0)
                .px(3.0)
                .items_center()
                .gap(4.0)
                .child(icon_button(
                    "island-add-back",
                    "Back",
                    22.0,
                    rgba(1.0, 1.0, 1.0, 0.0),
                    back_glyph(12.0, ink(MUTED)),
                    move |s| s.add_menu = AddMenu::Type(kind),
                ))
                .child(
                    text(kind_section(kind).to_string())
                        .font(rune::tok::TextStyle::new(11.5, 600.0))
                        .color(ink(TEXT)),
                )
                .child(rune::el().grow(1.0))
                .child(icon_button(
                    "island-add-close",
                    "Close add menu",
                    22.0,
                    rgba(1.0, 1.0, 1.0, 0.0),
                    x_glyph(11.0, ink(MUTED)),
                    |s| s.add_menu = AddMenu::Closed,
                )),
        )
        .child(list)
}

fn add_satellite(key: &str, x: f32, y: f32, label: &str, glyph: El, kind: IslandKind) -> El {
    row()
        .key(key.to_string())
        .absolute(x, y)
        .w(112.0)
        .h(32.0)
        .px(5.0)
        .items_center()
        .justify_end()
        .gap(7.0)
        .radius(16.0)
        .hover(rgba(1.0, 1.0, 1.0, 0.06))
        .role("button")
        .focusable(true)
        .label(format!("Add {}", kind_label(kind)))
        .on_press(move |s: &mut IslandState| s.add_menu = AddMenu::Type(kind))
        .child(
            text(label.to_string())
                .font(rune::tok::TextStyle::new(9.5, 500.0))
                .color(ink("#a1a1aa")),
        )
        .child(
            el().key(format!("{key}-disc"))
                .size(28.0, 28.0)
                .radius(14.0)
                .fill(ink("#1b1b20"))
                .border(1.0, ink("#ffffff1c"))
                .center()
                .inert()
                .child(glyph),
        )
}

/// The plus / radial menu / chooser, floating over the shelf's top-right.
/// Hovering the plus opens the fan; picking a fan button opens its chooser.
fn add_layer(s: &IslandState, model: &IslandModel, width: f32) -> El {
    let open = s.add_menu != AddMenu::Closed;
    let layer_w = if open { 278.0 } else { 34.0 };
    let layer_h = match s.add_menu {
        AddMenu::Closed | AddMenu::Radial => 148.0,
        AddMenu::Type(_) => 180.0,
        AddMenu::Existing(_) => 300.0,
    };
    let left = width - layer_w - 8.0;
    let mut layer = col()
        .key("island-add-layer")
        .absolute(left, 7.0)
        .w(layer_w)
        .h(if open { layer_h } else { 30.0 })
        .on_hover_change(|s: &mut IslandState, inside, _tx| {
            if inside {
                if s.add_menu == AddMenu::Closed {
                    s.add_menu = AddMenu::Radial;
                }
            } else {
                // Leaving the fan/chooser always drops back to the shelf and
                // lifts the scrim, as if the menu were never opened.
                s.add_menu = AddMenu::Closed;
            }
        });
    // Pinned to the fan's right edge, not placed from its width: the fan
    // grows leftward, and the plus must stay exactly under the pointer.
    let mut plus = el()
        .key("island-add")
        .role("button")
        .focusable(true)
        .label(if open { "Close add menu" } else { "Add" })
        .pin(None, Some(0.0), Some(4.0), None)
        .size(30.0, 30.0)
        .radius(15.0)
        .center()
        .fill(if open { ink("#ffffff") } else { ink("#232328") })
        .border(
            1.0,
            if open {
                ink("#ffffff")
            } else {
                ink("#ffffff1f")
            },
        )
        .hover(if open { ink("#ffffff") } else { ink("#2d2d34") })
        .on_press(|s: &mut IslandState| {
            s.add_menu = if s.add_menu == AddMenu::Closed {
                AddMenu::Radial
            } else {
                AddMenu::Closed
            };
        });
    plus = plus.child(if open {
        x_glyph(13.0, ink("#0b0b0d"))
    } else {
        plus_glyph(13.0, ink(TEXT))
    });
    layer = layer.child(plus);
    if open {
        match s.add_menu {
            AddMenu::Radial => {
                layer = layer
                    .child(add_satellite(
                        "island-radial-chat",
                        84.0,
                        2.0,
                        "Chat",
                        chat_glyph(14.0, ink(TEXT)),
                        IslandKind::Chat,
                    ))
                    .child(add_satellite(
                        "island-radial-group",
                        94.0,
                        38.0,
                        "Group",
                        users_glyph(14.0, ink(TEXT)),
                        IslandKind::Project,
                    ))
                    .child(add_satellite(
                        "island-radial-cloud",
                        112.0,
                        74.0,
                        "Cloud",
                        cloud_glyph(14.0, ink(TEXT)),
                        IslandKind::Cloud,
                    ));
            }
            AddMenu::Type(kind) => {
                layer = layer.child(
                    col()
                        .absolute(0.0, 40.0)
                        .w(270.0)
                        .child(add_type_panel(model, kind)),
                );
            }
            AddMenu::Existing(kind) => {
                layer = layer.child(
                    col()
                        .absolute(0.0, 40.0)
                        .w(270.0)
                        .child(add_existing_panel(model, kind)),
                );
            }
            AddMenu::Closed => {}
        }
    }
    layer
}

/// The dim + blur behind the add menu. It sits over the shelf, while the add
/// layer is painted after it; a press on the blurred area closes the menu,
/// and moving the pointer off the layer does the same on hover. No `modal`:
/// a modal child makes the pointer look like it left the notch, which would
/// collapse the whole shelf under the menu.
fn add_scrim() -> El {
    el().key("island-add-scrim")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(rgba(0.0, 0.0, 0.0, 0.34))
        .backdrop_blur(5.0)
        .corners([0.0, 0.0, 18.0, 18.0])
        .clip_corners([0.0, 0.0, 18.0, 18.0])
        .role("button")
        .label("Close add menu")
        .on_press(|s: &mut IslandState| s.add_menu = AddMenu::Closed)
}

fn tab_row(s: &IslandState) -> El {
    let mut tabs = row()
        .key("island-chat-tabs")
        .w_full()
        .h(30.0)
        .px(12.0)
        .pb(3.0)
        .gap(5.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], ink(HAIRLINE))
        .border_layout();
    for tab in &s.tabs {
        let target = tab.clone();
        let active = s
            .open_chat
            .as_ref()
            .is_some_and(|open| open.thread_key == tab.thread_key);
        let mut chip = row()
            .key(format!("island-chat-tab-{}", tab.thread_key))
            .role("button")
            .focusable(true)
            .label(format!("Switch to {}", tab.display()))
            .h(22.0)
            .max_w(158.0)
            .px(8.0)
            .items_center()
            .gap(5.0)
            .radius(6.0)
            .fill(if active {
                rgba(1.0, 1.0, 1.0, 0.10)
            } else {
                rgba(1.0, 1.0, 1.0, 0.0)
            })
            .border(
                1.0,
                if active {
                    rgba(1.0, 1.0, 1.0, 0.16)
                } else {
                    rgba(1.0, 1.0, 1.0, 0.07)
                },
            )
            .border_layout()
            .hover(rgba(1.0, 1.0, 1.0, 0.08))
            .on_press(move |s: &mut IslandState| s.switch_chat(target.clone()))
            .child(
                text(tab.display().to_string())
                    .grow(1.0)
                    .min_w(0.0)
                    .truncate()
                    .font(rune::tok::TextStyle::new(
                        10.0,
                        if active { 600.0 } else { 400.0 },
                    ))
                    .color(ink(if active { TEXT } else { MUTED })),
            );
        if active {
            chip = chip.child(pulse_dot(4.0, ink(TEXT), false));
        }
        tabs = tabs.child(chip);
    }
    tabs
}

fn chat_bubble(item: &IslandChatRow) -> El {
    if item.role == "event" {
        return row()
            .key(format!("island-chat-row-{}", item.key))
            .w_full()
            .justify_center()
            .child(
                text(item.body.clone())
                    .font(rune::tok::TextStyle::new(9.5, 500.0))
                    .color(ink("#777782")),
            );
    }
    let user = item.role == "user";
    let error = item.role == "error";
    let mut style = rune::tok::TextStyle::new(12.0, 400.0);
    style.line_height = 18.0;
    let mut bubble = col()
        .key(format!("island-chat-bubble-{}", item.key))
        .radius(13.0)
        .fill(ink(if user {
            "#28282f"
        } else if error {
            "#3a2226"
        } else {
            "#19191e"
        }))
        .border(1.0, ink(if error { "#fb71853d" } else { "#ffffff12" }))
        .px(11.0)
        .py(8.0)
        .gap(4.0);
    if !user {
        bubble = bubble.child(
            text(if item.agent.trim().is_empty() {
                "Stark".to_string()
            } else {
                item.agent.clone()
            })
            .font(rune::tok::TextStyle::new(9.5, 650.0))
            .color(ink("#f0f0f3")),
        );
    }
    // Reuse the real chat's markdown renderer: code blocks, tables and inline
    // styling look like the rest of Stark instead of raw `**` and fences.
    let body_el = if item.body.trim().is_empty() {
        text("Thinking".to_string())
            .key(format!("island-chat-body-{}", item.key))
            .font(style)
            .color(ink("#ededf1"))
    } else {
        crate::rune_transcript::render_markdown_in_group(
            &format!("island-chat-md-{}", item.key),
            &item.body,
            if user { "#dedee5" } else { "#d2d2da" },
            style,
        )
    };
    bubble = bubble.child(body_el);
    if item.streaming {
        let mut dots = row().items_center().gap(3.0).pt(1.0);
        for (n, delay) in [0.0, 0.14, 0.28].into_iter().enumerate() {
            dots = dots.child(
                el().key(format!("island-chat-typing-{}-{n}", item.key))
                    .size(3.0, 3.0)
                    .radius(1.5)
                    .fill(ink("#9b9ba5"))
                    .looping(rune::LoopSpec::opacity(1.1, delay, 0.35, 1.0)),
            );
        }
        bubble = bubble.child(dots);
    }
    let mut line = row()
        .key(format!("island-chat-row-{}", item.key))
        .w_full()
        .gap(7.0);
    if user {
        line = line.justify_end().child(bubble.max_w(390.0));
    } else {
        line = line
            .child(
                el().size(20.0, 20.0)
                    .radius(10.0)
                    .fill(ink(if error { "#3a2226" } else { "#24242a" }))
                    .center()
                    .inert()
                    .child(
                        text(if error {
                            "!".to_string()
                        } else {
                            "A".to_string()
                        })
                        .font(rune::tok::TextStyle::new(9.0, 700.0))
                        .color(ink("#c9c9d1")),
                    ),
            )
            .child(bubble.grow(1.0).min_w(0.0));
    }
    line
}

fn chat_mic_button(model: &IslandModel, target: &IslandTarget) -> El {
    let listening = model.listening && model.voice_thread == target.thread_key;
    let key = target.thread_key.clone();
    let mut button = el()
        .key("island-chat-mic")
        .role("button")
        .focusable(true)
        .label(if listening {
            "Stop listening"
        } else {
            "Start listening"
        })
        .size(30.0, 30.0)
        .radius(15.0)
        .center()
        .fill(if listening {
            ink("#ffffff")
        } else {
            ink("#232328")
        })
        .border(
            1.0,
            if listening {
                ink("#ffffff")
            } else {
                ink("#ffffff1f")
            },
        )
        .hover(if listening {
            ink("#ffffff")
        } else {
            ink("#2d2d34")
        })
        .on_press(move |s: &mut IslandState| {
            s.shared.push(IslandCommand::ToggleVoice(key.clone()));
        });
    if listening {
        let mut bars = row().h(13.0).items_center().gap(1.8).inert();
        const BAR_H: [f32; 5] = [5.0, 9.0, 13.0, 8.0, 11.0];
        for (i, h) in BAR_H.iter().enumerate() {
            bars = bars.child(
                el().size(1.8, *h)
                    .radius(0.9)
                    .fill(ink("#111113"))
                    .looping(rune::LoopSpec::opacity(1.2, i as f64 * 0.13, 0.35, 1.0))
                    .inert(),
            );
        }
        button = button.child(bars);
    } else {
        button = button.child(mic_glyph(13.0, ink(TEXT)));
    }
    button
}

fn chat_surface(s: &IslandState, model: &IslandModel, target: &IslandTarget) -> El {
    let key = target.thread_key.clone();
    let chat = model
        .chat
        .as_ref()
        .filter(|chat| chat.target.thread_key == key);
    let title = chat
        .map(|chat| chat.target.display().to_string())
        .unwrap_or_else(|| target.display().to_string());
    let running = chat.is_some_and(|chat| chat.live);
    let status = match chat {
        Some(chat) if !chat.status.trim().is_empty() => chat.status.clone(),
        _ => "Opening…".to_string(),
    };
    let open_target = target.clone();
    let head = row()
        .key("island-chat-header")
        .w_full()
        .h(48.0)
        .px(10.0)
        .items_center()
        .gap(8.0)
        .child(icon_button(
            "island-chat-back",
            "Back to activity",
            26.0,
            rgba(1.0, 1.0, 1.0, 0.0),
            back_glyph(14.0, ink(MUTED)),
            |s| s.leave_chat(),
        ))
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(1.0)
                .child(
                    text(title.clone())
                        .key("island-chat-title")
                        .truncate()
                        .font(rune::tok::TextStyle::new(12.5, 650.0))
                        .color(ink(TEXT)),
                )
                .child(
                    row()
                        .items_center()
                        .gap(5.0)
                        .child(pulse_dot(
                            4.0,
                            ink(if running { TEXT } else { "#5a5a62" }),
                            running,
                        ))
                        .child(tiny_label(
                            "island-chat-status",
                            &status,
                            9.5,
                            400.0,
                            ink(MUTED),
                        )),
                ),
        )
        .child(icon_button(
            "island-chat-open",
            "Open in Stark",
            26.0,
            rgba(1.0, 1.0, 1.0, 0.0),
            arrow_glyph(13.0, ink(MUTED)),
            move |s| s.shared.push(open_cmd(&open_target)),
        ));

    let mut body = col()
        .key("island-chat-body")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .gap(9.0)
        .px(14.0)
        .py(8.0)
        .scroll(rune::ScrollSpec { stick_bottom: true });
    match chat {
        None => {
            body = body
                .items_center()
                .justify_center()
                .child(pulse_dot(6.0, ink(MUTED), true))
                .child(tiny_label(
                    "island-chat-loading",
                    "Opening the chat…",
                    10.5,
                    400.0,
                    ink(MUTED),
                ));
        }
        Some(chat) if chat.rows.is_empty() => {
            body = body
                .items_center()
                .justify_center()
                .gap(4.0)
                .child(chat_glyph(20.0, ink("#66666f")))
                .child(tiny_label(
                    "island-chat-empty",
                    "Say something to start",
                    11.0,
                    500.0,
                    ink("#a1a1aa"),
                ));
        }
        Some(chat) => {
            for row in &chat.rows {
                body = body.child(chat_bubble(row));
            }
        }
    }

    let edit_key = key.clone();
    let submit_key = key.clone();
    let empty = TextEdit::new("");
    let edit = s.composers.get(&key).unwrap_or(&empty);
    let placeholder = if model.listening && model.voice_thread == key {
        "Listening…".to_string()
    } else if title.chars().count() <= 20 {
        format!("Message {title}")
    } else {
        "Message…".to_string()
    };
    let input = rune::text_input(edit, {
        let mut style = rune::tok::TextStyle::new(12.5, 400.0);
        style.line_height = 19.0;
        style
    })
    .key("island-chat-input")
    .w_full()
    .min_h(34.0)
    .max_h(68.0)
    .pt(2.0)
    .pb(2.0)
    .placeholder(placeholder, ink("#74747d"))
    .caret(ink("#f2f2f4"), crate::motion::TEXT_SELECTION)
    .on_edit(move |s: &mut IslandState| s.composer(&edit_key))
    .on_input_key(
        move |s: &mut IslandState, key: &rune::KeyEvent, _tx: &mut Tx| {
            if key.key != rune::Key::Enter || key.mods.shift {
                return false;
            }
            if s.composers
                .get(&submit_key)
                .is_some_and(|edit| edit.preedit.is_some())
            {
                return false;
            }
            s.send_chat(running);
            true
        },
    )
    .on_submit(move |s: &mut IslandState, _tx: &mut Tx| s.send_chat(running));

    let has_text = s
        .composers
        .get(&key)
        .is_some_and(|edit| !edit.text.trim().is_empty());
    let mut send = el()
        .key("island-chat-send")
        .role("button")
        .focusable(true)
        .label(if running { "Stop run" } else { "Send" })
        .size(32.0, 32.0)
        .radius(16.0)
        .center()
        .fill(if running || has_text {
            ink("#ffffff")
        } else {
            ink("#29292f")
        })
        .on_press(move |s: &mut IslandState| s.send_chat(running));
    if running || has_text {
        send = send.hover(ink("#ededf1"));
    }
    send = send.child(if running {
        stop_glyph(9.0, ink("#0b0b0d"))
    } else {
        send_glyph(13.0, ink(if has_text { "#0b0b0d" } else { "#71717a" }))
    });

    let listening = model.listening && model.voice_thread == key;
    let mut composer = col()
        .key("island-chat-composer")
        .w_full()
        .gap(5.0)
        .px(12.0)
        .pt(8.0)
        .pb(10.0)
        .border_sides([1.0, 0.0, 0.0, 0.0], ink(HAIRLINE))
        .border_layout();
    if let Some(attention) = &model.attention {
        let microphone = attention.title == "Microphone";
        let mine = attention.target.thread_key.is_empty() || attention.target.thread_key == key;
        if microphone && mine {
            let note_key = attention.target.thread_key.clone();
            composer = composer.child(
                row()
                    .key("island-chat-mic-error")
                    .w_full()
                    .items_center()
                    .gap(6.0)
                    .px(2.0)
                    .child(alert_glyph(11.0, ink(RED)))
                    .child(
                        tiny_label(
                            "island-chat-mic-error-text",
                            &attention.body,
                            9.5,
                            400.0,
                            ink(RED),
                        )
                        .max_w(470.0),
                    )
                    .child(rune::el().grow(1.0))
                    .child(icon_button(
                        "island-chat-mic-error-x",
                        "Dismiss microphone error",
                        20.0,
                        rgba(1.0, 1.0, 1.0, 0.0),
                        x_glyph(10.0, ink(MUTED)),
                        move |s| {
                            s.shared
                                .push(IslandCommand::DismissVoiceNote(note_key.clone()))
                        },
                    )),
            );
        }
    }
    if listening && !model.voice_text.trim().is_empty() {
        composer = composer.child(
            row()
                .w_full()
                .items_center()
                .gap(7.0)
                .child(waveform(model, 34.0, 18.0))
                .child(
                    tiny_label(
                        "island-chat-live-text",
                        model.voice_text.trim(),
                        10.0,
                        400.0,
                        ink("#a1a1aa"),
                    )
                    .max_w(440.0),
                ),
        );
    }
    composer = composer.child(input).child(
        row()
            .w_full()
            .items_center()
            .gap(8.0)
            .child(chat_mic_button(model, target))
            .child(rune::el().grow(1.0))
            .child(
                text(if running {
                    "Working".to_string()
                } else if has_text {
                    "Enter to send".to_string()
                } else {
                    "Shift+Enter for a new line".to_string()
                })
                .font(rune::tok::TextStyle::new(9.0, 400.0))
                .color(ink("#66666f")),
            )
            .child(send),
    );

    let mut pill = col().key("island-chat").w_full().h_full().child(head);
    if s.tabs.len() > 1 {
        pill = pill.child(tab_row(s));
    }
    pill.child(body).child(composer)
}

fn listening_body(model: &IslandModel) -> El {
    let copy = if model.voice_text.trim().is_empty() {
        "Speak now…".to_string()
    } else {
        model.voice_text.clone()
    };
    col()
        .key("island-listening-body")
        .w_full()
        .px(14.0)
        .pb(12.0)
        .gap(4.0)
        .child(
            text(copy)
                .key("island-voice-text")
                .w_full()
                .truncate()
                .font(rune::tok::TextStyle::new(12.0, 400.0))
                .color(ink(TEXT)),
        )
        .child(tiny_label(
            "island-voice-hint",
            if model.auto_send_voice {
                "Auto-sends to the active chat"
            } else {
                "Leaves a draft in the composer"
            },
            9.5,
            400.0,
            ink(MUTED),
        ))
}

pub fn island_view(s: &IslandState, _cx: &mut Cx<IslandState>) -> El {
    let model = s
        .shared
        .model
        .lock()
        .map(|model| model.clone())
        .unwrap_or_default();
    let listening = model.listening;
    let attention = model.attention.as_ref();
    let chat_open = s.open_chat.as_ref();
    // Attention cards are direct-action surfaces. Expanding them on hover
    // would move their X/Send button out from under the pointer, so they stay
    // compact until acted on or dismissed.
    let shelf_open = chat_open.is_none() && s.hovered && !listening && attention.is_none();
    let runs = model.runs.len();

    let (w, h) = if chat_open.is_some() {
        (CHAT_W, CHAT_H)
    } else if shelf_open {
        (SHELF_W, shelf_height(&model, s.add_menu))
    } else if listening {
        (376.0, 80.0)
    } else if attention.is_some() {
        (342.0, 56.0)
    } else if runs > 0 {
        (196.0, 28.0)
    } else {
        (104.0, 24.0)
    };

    // One neutral hairline in every state — no green borders, no glow. The
    // waveform and status dots carry the live signal in white.
    let border = ink(HAIRLINE);

    let mut notch = col()
        .key("stark-island")
        .w(w)
        .h(h)
        .corners([0.0, 0.0, 18.0, 18.0])
        .clip_corners([0.0, 0.0, 18.0, 18.0])
        .fill(ink("#0b0b0d"))
        .border(1.0, border)
        .border_layout()
        .motion(SPRING)
        .width_leads()
        .hit()
        .on_hover_change(hover_handler)
        .box_shadow(rune::Shadow {
            x: 0.0,
            y: 12.0,
            blur: 30.0,
            spread: -4.0,
            color: rgba(0.0, 0.0, 0.0, 0.58),
            inset: false,
        });

    if let Some(target) = chat_open {
        notch = notch.child(chat_surface(s, &model, target));
    } else if shelf_open {
        notch = notch.child(shelf_header(&model)).child(shelf_body(&model));
        if s.add_menu != AddMenu::Closed {
            notch = notch.child(add_scrim());
        }
        // The floating add layer is a sibling after the scrim, so it stays
        // above the dim/blur and keeps its own hover + press area.
        notch = notch.child(add_layer(s, &model, w));
    } else {
        let header_h = if listening {
            34.0
        } else if attention.is_some() {
            56.0
        } else if runs > 0 {
            28.0
        } else {
            24.0
        };
        notch = notch.child(compact_header(
            &model,
            listening,
            if listening { None } else { attention },
            header_h,
        ));
        if listening {
            notch = notch.child(listening_body(&model));
        }
    }
    // The overlay window is bigger than the pill so it can grow without a
    // native resize. This transparent root puts the notch at the top edge;
    // only the notch itself is a native input region.
    col()
        .key("island-root")
        .w_full()
        .h_full()
        .items_center()
        .justify_start()
        .child(notch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    fn model(listening: bool, runs: usize) -> IslandModel {
        let target = IslandTarget {
            thread_key: "chat:ws:c1".to_string(),
            title: "Writer".to_string(),
            card_id: Some("chat:ws:c1".to_string()),
            workspace_id: "ws".to_string(),
            chat_id: "c1".to_string(),
        };
        let runs: Vec<IslandRun> = (0..runs)
            .map(|i| IslandRun {
                target: IslandTarget {
                    thread_key: format!("chat:ws:c{i}"),
                    title: format!("Writer {i}"),
                    card_id: Some(format!("chat:ws:c{i}")),
                    workspace_id: "ws".to_string(),
                    chat_id: format!("c{i}"),
                },
                activity: "Editing rune_canvas.rs".to_string(),
                elapsed: "0:42".to_string(),
                stoppable: true,
            })
            .collect();
        let shelf: Vec<IslandShelfItem> = runs
            .iter()
            .map(|run| IslandShelfItem {
                key: run.target.thread_key.clone(),
                kind: IslandKind::Chat,
                title: run.target.display().to_string(),
                subtitle: run.activity.clone(),
                meta: run.elapsed.clone(),
                running: true,
                waiting: 0,
                target: run.target.clone(),
                project_id: String::new(),
            })
            .collect();
        let existing = shelf.clone();
        IslandModel {
            runs,
            shelf,
            existing,
            listening,
            armed: false,
            level: 0.7,
            voice_text: "refactor the provider panel".to_string(),
            target,
            attention: None,
            auto_send_voice: false,
            island_enabled: true,
            tick_ms: 0,
            ..IslandModel::default()
        }
    }

    fn render(model: IslandModel, hovered: bool) -> Runtime<IslandState> {
        render_with(model, |state| state.hovered = hovered)
    }

    fn render_with(
        model: IslandModel,
        setup: impl FnOnce(&mut IslandState),
    ) -> Runtime<IslandState> {
        let shared = Arc::new(IslandShared::new(None));
        *shared.model.lock().unwrap() = model;
        let mut state = IslandState::new(shared);
        setup(&mut state);
        let mut rt = Runtime::new(state, island_view, ISLAND_WINDOW, Env::new(true, 1.0));
        rt.frame(0.0);
        rt
    }

    #[test]
    fn idle_is_a_tiny_notch_with_a_mark_and_ready_dot() {
        let rt = render(model(false, 0), false);
        assert!(rt.find("stark-island").is_some());
        assert!(rt.find("island-mark").is_some());
        assert!(rt.find("island-shelf").is_none());
        let (rect, _) = rt.presented("stark-island").expect("notch laid out");
        assert!(
            rect[2] <= 104.0 && rect[3] <= 24.0,
            "idle hugs the mark: {}x{}",
            rect[2],
            rect[3]
        );
    }

    #[test]
    fn running_pill_shows_the_count_and_hover_opens_the_shelf() {
        let compact = render(model(false, 2), false);
        assert!(compact.find("island-run-count").is_some());
        assert!(compact.find("island-shelf").is_none());

        let open = render(model(false, 2), true);
        assert!(open.find("island-shelf").is_some());
        assert!(open.find("island-shelf-chat:ws:c0").is_some());
        assert!(open.find("island-add").is_some());
        // The old footer is gone: no mic, shortcut text, open arrow or brand.
        assert!(open.find("island-mic").is_none());
        assert!(open.find("island-shortcut").is_none());
        assert!(open.find("island-open").is_none());
        assert!(open.find("island-brand").is_none());
    }

    #[test]
    fn add_menu_fans_out_into_new_and_existing() {
        let rt = render_with(model(false, 1), |state| {
            state.hovered = true;
            state.add_menu = AddMenu::Radial;
        });
        assert!(rt.find("island-radial-chat").is_some());
        assert!(rt.find("island-radial-group").is_some());
        assert!(rt.find("island-radial-cloud").is_some());

        let chooser = render_with(model(false, 1), |state| {
            state.hovered = true;
            state.add_menu = AddMenu::Type(IslandKind::Chat);
        });
        assert!(chooser.find("island-add-new").is_some());
        assert!(chooser.find("island-add-existing").is_some());

        let existing = render_with(model(false, 1), |state| {
            state.hovered = true;
            state.add_menu = AddMenu::Existing(IslandKind::Chat);
        });
        assert!(existing.find("island-add-row-chat:ws:c0").is_some());
    }

    #[test]
    fn a_selected_chat_gets_the_compact_transcript_and_prompt_dock() {
        let mut m = model(false, 1);
        let target = m.shelf[0].target.clone();
        m.chat = Some(IslandChat {
            target: target.clone(),
            rows: vec![
                IslandChatRow {
                    key: "u0".to_string(),
                    role: "user".to_string(),
                    body: "ship the island".to_string(),
                    agent: "Stark".to_string(),
                    ..IslandChatRow::default()
                },
                IslandChatRow {
                    key: "a1".to_string(),
                    role: "agent".to_string(),
                    body: "On it.".to_string(),
                    agent: "Stark".to_string(),
                    streaming: true,
                    ..IslandChatRow::default()
                },
            ],
            live: true,
            status: "Writing".to_string(),
            draft: String::new(),
            draft_rev: 0,
        });
        let rt = render_with(m, |state| {
            state.hovered = true;
            state.enter_chat(target);
        });
        assert!(rt.find("island-chat").is_some());
        assert!(rt.find("island-chat-title").is_some());
        assert!(rt.find("island-chat-row-u0").is_some());
        assert!(rt.find("island-chat-row-a1").is_some());
        assert!(rt.find("island-chat-input").is_some());
        assert!(rt.find("island-chat-send").is_some());
        assert!(rt.find("island-chat-mic").is_some());
        assert!(rt.find("island-chat-back").is_some());
        assert!(rt.find("island-shelf").is_none(), "chat replaces the shelf");
    }

    #[test]
    fn a_voice_final_replaces_an_untouched_draft_but_never_user_words() {
        let mut m = model(false, 1);
        let target = m.shelf[0].target.clone();
        m.chat = Some(IslandChat {
            target: target.clone(),
            rows: Vec::new(),
            live: false,
            status: "Ready".to_string(),
            draft: String::new(),
            draft_rev: 0,
        });
        let shared = Arc::new(IslandShared::new(None));
        *shared.model.lock().unwrap() = m.clone();
        let mut state = IslandState::new(shared.clone());
        state.enter_chat(target.clone());
        let mut tx = Tx::default();
        island_poll(&mut state, 0.0, &mut tx);

        if let Some(chat) = m.chat.as_mut() {
            chat.draft = "spoken first".to_string();
            chat.draft_rev = 1;
        }
        *shared.model.lock().unwrap() = m.clone();
        island_poll(&mut state, 0.0, &mut tx);
        assert_eq!(state.composer(&target.thread_key).text, "spoken first");

        // The user edits it; the next final must not steal their words.
        state.composer(&target.thread_key).text = "mine".to_string();
        if let Some(chat) = m.chat.as_mut() {
            chat.draft = "spoken second".to_string();
            chat.draft_rev = 2;
        }
        *shared.model.lock().unwrap() = m;
        island_poll(&mut state, 0.0, &mut tx);
        assert_eq!(state.composer(&target.thread_key).text, "mine");
    }

    #[test]
    fn a_live_title_update_reaches_the_header_and_its_tab() {
        let mut m = model(false, 1);
        let mut target = m.shelf[0].target.clone();
        target.title = "New Chat".to_string();
        m.chat = Some(IslandChat {
            target: target.clone(),
            rows: Vec::new(),
            live: false,
            status: "Ready".to_string(),
            draft: String::new(),
            draft_rev: 0,
        });
        let shared = Arc::new(IslandShared::new(None));
        *shared.model.lock().unwrap() = m.clone();
        let mut state = IslandState::new(shared.clone());
        state.hovered = true;
        state.enter_chat(target);
        let mut tx = Tx::default();
        island_poll(&mut state, 0.0, &mut tx);
        // The shell renamed the chat after its first prompt.
        if let Some(chat) = m.chat.as_mut() {
            chat.target.title = "hello island".to_string();
        }
        *shared.model.lock().unwrap() = m;
        shared.bump();
        island_poll(&mut state, 0.0, &mut tx);
        assert_eq!(
            state.open_chat.as_ref().map(|open| open.title.as_str()),
            Some("hello island")
        );
        assert!(state.tabs.iter().any(|tab| tab.title == "hello island"));
    }

    #[test]
    fn sending_from_the_overlay_queues_a_prompt_and_clears_the_draft() {
        let shared = Arc::new(IslandShared::new(None));
        let mut state = IslandState::new(shared.clone());
        let target = model(false, 1).shelf[0].target.clone();
        state.enter_chat(target.clone());
        state.composer(&target.thread_key).text = "refactor the dock".to_string();
        state.send_chat(false);
        assert!(state.composer(&target.thread_key).text.is_empty());
        let commands = shared.take_commands();
        assert!(
            commands.iter().any(|cmd| matches!(
                cmd,
                IslandCommand::SendPrompt { target: sent, text }
                    if sent.thread_key == target.thread_key && text == "refactor the dock"
            )),
            "send reaches the shell: {commands:?}"
        );
    }

    #[test]
    fn the_idle_listening_wave_has_visible_peaks() {
        let rt = render(model(true, 0), false);
        let mut heights = Vec::new();
        for i in 0..7 {
            let (rect, _) = rt
                .presented(&format!("island-wave-bar-{i}"))
                .expect("wave bar");
            heights.push(rect[3]);
        }
        let low = heights.iter().cloned().fold(f32::INFINITY, f32::min);
        let high = heights.iter().cloned().fold(0.0_f32, f32::max);
        assert!(high - low >= 2.0, "the wave has peaks: {heights:?}");
    }

    #[test]
    fn listening_shows_the_live_text_stop_and_target() {
        let rt = render(model(true, 0), false);
        assert!(rt.find("island-wave").is_some());
        assert!(rt.find("island-voice-text").is_some());
        assert!(rt.find("island-voice-stop").is_some());
        assert!(rt.find("island-listening-target").is_some());
        assert!(
            rt.find("island-shelf").is_none(),
            "listening owns the surface"
        );
    }

    #[test]
    fn a_pointer_leave_collapses_after_the_grace_period() {
        let shared = Arc::new(IslandShared::new(None));
        let mut state = IslandState::new(shared);
        state.hovered = true;
        state.collapse_at = Some(crate::clock::now_millis() as f64 / 1000.0 - 0.01);
        let mut tx = Tx::default();
        assert!(island_poll(&mut state, 0.0, &mut tx));
        assert!(!state.hovered, "the leave deadline lands");
        assert!(state.collapse_at.is_none());
    }

    #[test]
    fn attention_carries_an_action() {
        let mut m = model(false, 0);
        m.attention = Some(IslandAttention {
            kind: IslandAttentionKind::Question,
            title: "Needs you".to_string(),
            body: "Approve the deployment?".to_string(),
            target: m.target.clone(),
            expires_ms: 0,
        });
        let rt = render(m, false);
        assert!(rt.find("island-attention-title").is_some());
        assert!(rt.find("island-attention-action").is_some());
        assert!(rt.find("island-attention-body").is_some());
    }

    #[test]
    fn dismissing_a_mic_error_queues_the_dismiss_command() {
        let mut m = model(false, 0);
        m.attention = Some(IslandAttention {
            kind: IslandAttentionKind::Error,
            title: "Microphone".to_string(),
            body: "Microphone interrupted".to_string(),
            target: m.target.clone(),
            expires_ms: 0,
        });
        let shared = Arc::new(IslandShared::new(None));
        *shared.model.lock().unwrap() = m;
        let mut rt = Runtime::new(
            IslandState::new(shared.clone()),
            island_view,
            ISLAND_WINDOW,
            Env::new(true, 1.0),
        );
        rt.frame(0.0);
        let (x, y) = rt.find("island-attention-action").expect("dismiss placed");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        let commands = shared.take_commands();
        assert!(
            commands
                .iter()
                .any(|cmd| *cmd == IslandCommand::DismissVoiceNote("chat:ws:c1".to_string())),
            "dismiss press reaches the shell: {commands:?}"
        );
    }
}
