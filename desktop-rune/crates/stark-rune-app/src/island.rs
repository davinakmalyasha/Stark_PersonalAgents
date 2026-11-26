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

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rune::{col, el, row, text, Cx, El, OverlayHandle, Tx};

use crate::shell::ShellState;

/// How far the island grows. The cockpit is compact on purpose: it is a
/// glance-and-jump surface, not a second canvas.
pub const ISLAND_WINDOW: (f32, f32) = (520.0, 420.0);

const HAIRLINE: &str = "#ffffff14";
const TEXT: &str = "#f4f4f5";
const MUTED: &str = "#8a8a93";
const GREEN: &str = "#3ddc97";
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
    pub listening: bool,
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
            listening: false,
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
    ToggleListen,
    StopListen,
    OpenMain,
    OpenChat(IslandTarget),
    StopRun(String),
    SendDraft(String),
    DismissVoiceNote(String),
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

/// The overlay's own state. Hover is intentionally local so pointer travel
/// never rebuilds the whole shell.
pub struct IslandState {
    pub shared: Arc<IslandShared>,
    pub hovered: bool,
    pub collapse_at: Option<f64>,
    seen_revision: u64,
}

impl IslandState {
    pub fn new(shared: Arc<IslandShared>) -> Self {
        Self {
            shared,
            hovered: false,
            collapse_at: None,
            seen_revision: 0,
        }
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

    pub fn take_commands(&self) -> Vec<IslandCommand> {
        self.shared.take_commands()
    }
}

pub fn island_poll(s: &mut IslandState, _clock: f64, tx: &mut Tx) -> bool {
    let revision = s.shared.revision.load(Ordering::Relaxed);
    let mut changed = false;
    if revision != s.seen_revision {
        s.seen_revision = revision;
        changed = true;
    }
    if let Some(at) = s.collapse_at {
        // The overlay poll clock is elapsed-since-start; hover deadlines use
        // the same wall clock as the handler so a leave always lands.
        let now = crate::clock::now_millis() as f64 / 1000.0;
        if now >= at {
            s.collapse_at = None;
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
        prewarm: true,
        ..Default::default()
    })
    .poll(island_poll)
    .animate_reveal()
}

fn hover_handler(s: &mut IslandState, inside: bool, tx: &mut Tx) {
    if inside {
        s.hovered = true;
        s.collapse_at = None;
    } else if s.hovered {
        // A short grace lets the pointer cross the rounded edge without the
        // cockpit snapping shut under it.
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

fn chevron_glyph(size: f32, color: rune::Color) -> El {
    el().size(size, size)
        .inert()
        .shape(crate::icons::lucide(&["m6 9 6 6 6-6"], size, 2.0, color))
}

fn bot_mark(color: rune::Color) -> El {
    el().key("island-mark")
        .size(19.0, 19.0)
        .inert()
        .shape(crate::shell::icon_group(19.0, color))
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
                .fill(ink(GREEN).alpha(dim))
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

fn status_copy(model: &IslandModel) -> String {
    match model.runs.len() {
        0 => "Idle".to_string(),
        1 => "1 running".to_string(),
        n => format!("{n} running"),
    }
}

fn attention_icon(kind: IslandAttentionKind) -> El {
    match kind {
        IslandAttentionKind::Done => check_glyph(14.0, ink(GREEN)),
        IslandAttentionKind::Draft => mic_glyph(14.0, ink(GREEN)),
        IslandAttentionKind::Error => alert_glyph(14.0, ink(RED)),
        IslandAttentionKind::Question => help_glyph(14.0, ink(TEXT)),
    }
}

fn header(
    model: &IslandModel,
    cockpit: bool,
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
        // The cockpit repeats attention as a full-width row in the body, so
        // its header stays the Stark status bar. The compact island uses the
        // attention card itself as the header.
        if !cockpit {
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
    }

    head = head.child(bot_mark(if cockpit { ink(TEXT) } else { ink("#c7c7cf") }));
    if cockpit {
        head = head
            .child(tiny_label("island-brand", "Stark", 12.5, 600.0, ink(TEXT)))
            .child(
                row()
                    .items_center()
                    .gap(5.0)
                    .child(pulse_dot(
                        5.0,
                        if model.runs.is_empty() {
                            ink("#5a5a62")
                        } else {
                            ink(GREEN)
                        },
                        !model.runs.is_empty(),
                    ))
                    .child(tiny_label(
                        "island-status",
                        &status_copy(model),
                        10.5,
                        500.0,
                        ink(MUTED),
                    )),
            )
            .child(rune::el().grow(1.0))
            .child(arrow_glyph(14.0, ink(MUTED)));
    } else if !model.runs.is_empty() {
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
            .child(pulse_dot(6.0, ink(GREEN), true));
    } else {
        head = head
            .child(rune::el().grow(1.0))
            .child(pulse_dot(5.0, ink("#5a5a62"), false));
    }
    head
}

fn cockpit_height(model: &IslandModel) -> f32 {
    let rows = model.runs.len().min(3);
    let more = model.runs.len().saturating_sub(3);
    let attention = if model.attention.is_some() { 56.0 } else { 0.0 };
    let list = if rows == 0 {
        if model.attention.is_some() {
            0.0
        } else {
            44.0
        }
    } else {
        20.0 + rows as f32 * 42.0 + if more > 0 { 22.0 } else { 0.0 }
    };
    44.0 + 8.0 + list + attention + 46.0 + 8.0
}

fn run_row(run: &IslandRun) -> El {
    let target = run.target.clone();
    let stop_key = run.target.thread_key.clone();
    let mut row = row()
        .key(format!("island-run-{}", run.target.thread_key))
        .w_full()
        .h(42.0)
        .px(12.0)
        .items_center()
        .gap(8.0)
        .hover(rgba(1.0, 1.0, 1.0, 0.045))
        .on_press(move |s: &mut IslandState| s.shared.push(open_cmd(&target)))
        .child(pulse_dot(6.0, ink(GREEN), true))
        .child(
            el().size(15.0, 15.0)
                .inert()
                .shape(crate::shell::icon_group(15.0, ink("#b9b9c2"))),
        )
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(1.0)
                .child(tiny_label(
                    "island-run-title",
                    run.target.display(),
                    12.0,
                    600.0,
                    ink(TEXT),
                ))
                .child(tiny_label(
                    "island-run-activity",
                    &run.activity,
                    10.5,
                    400.0,
                    ink(MUTED),
                )),
        )
        .child(
            text(run.elapsed.clone())
                .key("island-run-elapsed")
                .font(rune::tok::TextStyle::new(10.5, 400.0).mono())
                .color(ink("#777782")),
        );
    if run.stoppable {
        row = row.child(action_button(
            "island-run-stop",
            "Stop agent",
            rgba(1.0, 1.0, 1.0, 0.08),
            stop_glyph(8.0, ink(TEXT)),
            move |s| {
                s.shared.push(IslandCommand::StopRun(stop_key.clone()));
            },
        ));
    } else {
        row = row.child(chevron_glyph(12.0, ink(MUTED)));
    }
    row
}

fn attention_row(attention: &IslandAttention) -> El {
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

fn cockpit(model: &IslandModel) -> El {
    let mut body = col()
        .key("island-cockpit")
        .w_full()
        .gap(2.0)
        .px(0.0)
        .pb(0.0);

    if model.runs.is_empty() && model.attention.is_none() {
        body = body.child(
            row()
                .key("island-empty")
                .w_full()
                .h(44.0)
                .px(12.0)
                .items_center()
                .gap(8.0)
                .child(mic_glyph(14.0, ink(MUTED)))
                .child(tiny_label(
                    "island-empty-copy",
                    "No agents running · Ctrl+Space to talk",
                    10.5,
                    400.0,
                    ink(MUTED),
                )),
        );
    } else {
        body = body.child(
            text("RUNNING".to_string())
                .key("island-running-label")
                .w_full()
                .px(12.0)
                .h(20.0)
                .font(rune::tok::TextStyle::new(9.0, 600.0))
                .color(ink("#66666f")),
        );
        for run in model.runs.iter().take(3) {
            body = body.child(run_row(run));
        }
        let more = model.runs.len().saturating_sub(3);
        if more > 0 {
            let target = model.target.clone();
            body = body.child(
                row()
                    .key("island-more")
                    .w_full()
                    .h(22.0)
                    .px(12.0)
                    .items_center()
                    .child(tiny_label(
                        "island-more-copy",
                        &format!("+{more} more"),
                        10.0,
                        500.0,
                        ink(MUTED),
                    ))
                    .on_press(move |s: &mut IslandState| s.shared.push(open_cmd(&target))),
            );
        }
    }

    if let Some(attention) = &model.attention {
        body = body.child(attention_row(attention));
    }

    body.child(
        row()
            .key("island-footer")
            .w_full()
            .h(46.0)
            .px(12.0)
            .items_center()
            .gap(8.0)
            .border_sides([1.0, 0.0, 0.0, 0.0], ink(HAIRLINE))
            .border_layout()
            .child(action_button(
                "island-mic",
                "Start listening",
                rgba(1.0, 1.0, 1.0, 0.10),
                mic_glyph(13.0, ink(TEXT)),
                |s| s.shared.push(IslandCommand::ToggleListen),
            ))
            .child(tiny_label(
                "island-shortcut",
                "Ctrl+Space to talk",
                10.0,
                400.0,
                ink(MUTED),
            ))
            .child(rune::el().grow(1.0))
            .child(action_button(
                "island-open",
                "Open Stark",
                rgba(1.0, 1.0, 1.0, 0.08),
                arrow_glyph(12.0, ink(TEXT)),
                |s| s.shared.push(IslandCommand::OpenMain),
            )),
    )
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
    // Attention cards are direct-action surfaces. Expanding them on hover
    // would move their X/Send button out from under the pointer, so they stay
    // compact until acted on or dismissed.
    let cockpit_open = s.hovered && !listening && attention.is_none();
    let runs = model.runs.len();

    let (w, h) = if cockpit_open {
        (420.0, cockpit_height(&model).min(360.0))
    } else if listening {
        (376.0, 80.0)
    } else if attention.is_some() {
        (342.0, 56.0)
    } else if runs > 0 {
        (196.0, 28.0)
    } else {
        (104.0, 24.0)
    };

    let border = if listening {
        ink(GREEN).alpha(0.42)
    } else if runs > 0 {
        ink(GREEN).alpha(0.22)
    } else {
        ink(HAIRLINE)
    };

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
    if listening {
        notch = notch.box_shadow(rune::Shadow {
            x: 0.0,
            y: 0.0,
            blur: 28.0,
            spread: 0.0,
            color: ink(GREEN).alpha(0.18),
            inset: false,
        });
    }

    let header_h = if cockpit_open {
        44.0
    } else if listening {
        34.0
    } else if attention.is_some() {
        56.0
    } else if runs > 0 {
        28.0
    } else {
        24.0
    };
    notch = notch.child(header(&model, cockpit_open, listening, attention, header_h));
    if cockpit_open {
        notch = notch.child(cockpit(&model));
    } else if listening {
        notch = notch.child(listening_body(&model));
    }
    // The overlay window is bigger than the pill so the cockpit can grow
    // without a native resize. This transparent root puts the notch at the
    // top edge; only the notch itself is a native input region.
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
        IslandModel {
            runs: (0..runs)
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
                .collect(),
            listening,
            level: 0.7,
            voice_text: "refactor the provider panel".to_string(),
            target,
            attention: None,
            auto_send_voice: false,
            island_enabled: true,
            tick_ms: 0,
        }
    }

    fn render(model: IslandModel, hovered: bool) -> Runtime<IslandState> {
        let shared = Arc::new(IslandShared::new(None));
        *shared.model.lock().unwrap() = model;
        let mut state = IslandState::new(shared);
        state.hovered = hovered;
        let mut rt = Runtime::new(state, island_view, (520.0, 420.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt
    }

    #[test]
    fn idle_is_a_tiny_notch_with_a_mark_and_ready_dot() {
        let rt = render(model(false, 0), false);
        assert!(rt.find("stark-island").is_some());
        assert!(rt.find("island-mark").is_some());
        assert!(rt.find("island-cockpit").is_none());
        let (rect, _) = rt.presented("stark-island").expect("notch laid out");
        assert!(
            rect[2] <= 104.0 && rect[3] <= 24.0,
            "idle hugs the mark: {}x{}",
            rect[2],
            rect[3]
        );
    }

    #[test]
    fn running_pill_shows_the_count_and_hover_opens_the_cockpit() {
        let compact = render(model(false, 2), false);
        assert!(compact.find("island-run-count").is_some());
        assert!(compact.find("island-cockpit").is_none());

        let open = render(model(false, 2), true);
        assert!(open.find("island-cockpit").is_some());
        assert!(open.find("island-run-chat:ws:c0").is_some());
        assert!(open.find("island-run-stop").is_some());
        assert!(open.find("island-mic").is_some());
        assert!(open.find("island-open").is_some());
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
            rt.find("island-cockpit").is_none(),
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
}
