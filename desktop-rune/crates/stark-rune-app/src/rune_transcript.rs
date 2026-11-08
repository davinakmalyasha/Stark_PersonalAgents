//! Real Rune transcript view — MessageStream port with virtualized rows.
//!
//! Only compiled with `--features rune-real` (needs the pinned `rune` checkout
//! + GPU at snap time; unit tests below the `cfg` stay GPU-free).
//!
//! Rune mapping, 1:1 with `crates/rune/README.md`:
//! - scroll viewport: `.scroll(ScrollSpec { stick_bottom: true })` on a
//!   height-bounded column (`.h_full().min_h(0.0)` — flex `.grow` collapses
//!   unbounded viewports to 0pt).
//! - growth stack: `.growth(GrowthSpec::default())` + lazy
//!   `.virtual_rows(count, EST, key, build)` as the viewport's first child;
//!   rows keyed by message id with internal padding (never stack gaps/margins).
//!   Eager `.virtualize` + prebuilt children is a perf bug Rune warns on;
//!   handlers find items by key, never by index.
//! - headless PNG: `stark-rune --rune-snap out.png [--rune-scene name]`
//!   runs `rune::App` headless (`--snap`); tree via `--tree`.

use std::borrow::Cow;

use rune::{col, row, text, AppSpec, Cx, El, GrowthSpec, ScrollSpec, TextEdit};
use stark_kit::transcript::{TranscriptRow, ESTIMATED_ROW_H};

/// Row content fed to both the pure window math and the Rune view.
#[derive(Debug, Clone)]
pub struct RowData {
    pub key: String,
    pub role: &'static str,
    pub body: String,
}

pub struct RuneTranscriptState {
    pub title: String,
    pub rows: Vec<RowData>,
    pub edit: TextEdit,
    pub model: String,
    pub live: bool,
    pub api_host: String,
    pub api_port: u16,
    /// Settings sheet visibility.
    pub settings_open: bool,
    /// Voice dictation affordance (mic capture binds later).
    pub voice_live: bool,
    /// Frames streamed from the run thread, drained by the `poll` hook.
    /// `None` on clone: a copied state never inherits a live socket.
    pub inbox: Option<std::sync::mpsc::Receiver<crate::stream::SseFrame>>,
}

impl std::fmt::Debug for RuneTranscriptState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuneTranscriptState")
            .field("title", &self.title)
            .field("rows", &self.rows)
            .field("edit", &self.edit.text)
            .field("model", &self.model)
            .field("live", &self.live)
            .field("api_host", &self.api_host)
            .field("api_port", &self.api_port)
            .field("settings_open", &self.settings_open)
            .field("voice_live", &self.voice_live)
            .field("inbox", &self.inbox.is_some())
            .finish()
    }
}

impl Clone for RuneTranscriptState {
    fn clone(&self) -> Self {
        Self {
            title: self.title.clone(),
            rows: self.rows.clone(),
            edit: self.edit.clone(),
            model: self.model.clone(),
            live: self.live,
            api_host: self.api_host.clone(),
            api_port: self.api_port,
            settings_open: self.settings_open,
            voice_live: self.voice_live,
            inbox: None,
        }
    }
}

pub const DEFAULT_API_HOST: &str = "127.0.0.1";
pub const DEFAULT_API_PORT: u16 = 8124;

impl RuneTranscriptState {
    /// Screenshot scene: mirrors `AppState::default` card-4 (the running chat).
    pub fn screenshot() -> Self {
        Self {
            title: "New Chat".to_string(),
            rows: vec![
                RowData {
                    key: "msg-0".to_string(),
                    role: "user",
                    body: "can you use a music api or smth, makes green green real, playing now in your tab, something like that, and play that in current youtube".to_string(),
                },
                RowData {
                    key: "msg-1".to_string(),
                    role: "agent",
                    body: "Playing now in your YouTube tab: massive green day — Cinnamon Girl (8yr-old). A 20s pre-roll ad played first — I skipped it, and the actual track is running (04:48).".to_string(),
                },
            ],
            edit: TextEdit::new(""),
            model: "Automatic Fallback".to_string(),
            live: true,
            api_host: DEFAULT_API_HOST.to_string(),
            api_port: DEFAULT_API_PORT,
            settings_open: false,
            voice_live: false,
            inbox: None,
        }
    }

    /// Stress scene: 10k rows for the virtualization parity check.
    pub fn stress() -> Self {
        let mut rows = Vec::with_capacity(10_000);
        for i in 0..10_000 {
            let (role, body) = if i % 3 == 0 {
                (
                    "user",
                    format!("prompt {i}: please summarize the workspace state"),
                )
            } else {
                (
                    "agent",
                    format!("answer {i}: applied 3 edits across 2 files, tests passing"),
                )
            };
            rows.push(RowData {
                key: format!("msg-{i}"),
                role,
                body,
            });
        }
        Self {
            title: "Stress — 10k rows".to_string(),
            rows,
            edit: TextEdit::new(""),
            model: "Automatic Fallback".to_string(),
            live: false,
            api_host: DEFAULT_API_HOST.to_string(),
            api_port: DEFAULT_API_PORT,
            settings_open: false,
            voice_live: false,
            inbox: None,
        }
    }

    pub fn transcript_rows(&self) -> Vec<TranscriptRow> {
        use stark_kit::Role;
        self.rows
            .iter()
            .map(|r| {
                let role = match r.role {
                    "user" => Role::User,
                    "error" => Role::Error,
                    "system" => Role::System,
                    _ => Role::Agent,
                };
                TranscriptRow::new(&r.key, role, r.body.len())
            })
            .collect()
    }
}

fn bg() -> rune::Color {
    rune::color::hex("#0a0a0a").unwrap_or(rune::Color::BLACK)
}

/// Shared send path (button + Enter): stage the field as a user row, mark the
/// run live, and spawn the blocking `POST /api/run` thread into `inbox`.
pub fn stage_send(s: &mut RuneTranscriptState) {
    let prompt = s.edit.text.trim().to_string();
    if prompt.is_empty() {
        return;
    }
    let n = s.rows.len();
    s.rows.push(RowData {
        key: format!("user-{n}"),
        role: "user",
        body: prompt.clone(),
    });
    s.live = true;
    s.edit.text.clear();
    s.edit.anchor = 0;
    s.edit.focus = 0;
    let req = crate::stream::RunRequest {
        prompt,
        model: s.model.clone(),
        temperature: 0.7,
        workspace_path: String::new(),
        chat_id: String::new(),
        workspace_kind: "general".to_string(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    s.inbox = Some(rx);
    let (host, port) = (s.api_host.clone(), s.api_port);
    std::thread::spawn(move || {
        let mut emit = |frame: crate::stream::SseFrame| {
            let _ = tx.send(frame);
        };
        if let Err(message) = crate::transport::post_run(&host, port, &req, &mut emit) {
            let _ = tx.send(crate::stream::SseFrame {
                event: "error".to_string(),
                data: serde_json::json!({"message": message}),
            });
        }
    });
}

/// Host access for views composed into a larger shell: handlers stay typed to
/// the shell state while reading transcript data. The standalone transcript
/// view hosts itself.
pub trait TranscriptHost {
    fn transcript(&self) -> &RuneTranscriptState;
    fn transcript_mut(&mut self) -> &mut RuneTranscriptState;
}

impl TranscriptHost for RuneTranscriptState {
    fn transcript(&self) -> &RuneTranscriptState {
        self
    }
    fn transcript_mut(&mut self) -> &mut RuneTranscriptState {
        self
    }
}

pub fn transcript_view(s: &RuneTranscriptState, _: &mut Cx<RuneTranscriptState>) -> El {
    transcript_body::<RuneTranscriptState>(s)
}

/// Transcript body shared by the standalone view and the shell: rows read
/// `s`, handlers resolve through `S: TranscriptHost`.
pub fn transcript_body<S: TranscriptHost + 'static>(s: &RuneTranscriptState) -> El {
    // Index-built rows: layout builds only the mounted window (viewport + up to
    // two viewports either side), so a 10k transcript commits in window time.
    // Handlers must find items by key, never by index (see `RowSource` docs).
    // Thread padding lives on the stack (outside rows, estimates stay valid);
    // user rows `justify_end` with content-sized bubbles like
    // `.canvas-bubble-row.is-user`; agent bubbles `grow` full width.
    let stack = col()
        .key("transcript-stack")
        .px(12.0)
        .growth(GrowthSpec::default())
        .virtual_rows(
            s.rows.len(),
            ESTIMATED_ROW_H,
            // Typed to the window state `S` (Rune checks the registration):
            // read rows through the host, never by closing over `s`.
            |s: &S, i| Cow::Borrowed(s.transcript().rows[i].key.as_str()),
            |s: &S, i| {
                let r = &s.transcript().rows[i];
                // Role palette from `canvas.css` `.canvas-bubble*`: user
                // #28282f, agent #19191e, error rose, system quiet; 22px
                // avatars, 11px/650 name headers, 12.5px wrapped bodies.
                let (fill, avatar_bg, initial, name) = match r.role {
                    "user" => ("#28282f", "#303038", "Y", "You"),
                    "error" => ("#2a1a1d", "#3a2226", "!", "Error"),
                    "system" => ("#101013", "#24242a", "S", "System"),
                    _ => ("#19191e", "#24242a", "A", "Assistant"),
                };
                let ink = |hex: &'static str| rune::color::hex(hex).unwrap_or(rune::Color::BLACK);
                let hairline = rune::Color {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.06,
                };
                let name_style = rune::tok::TextStyle::new(11.0, 650.0);
                let body_style = rune::tok::TextStyle::new(12.5, 400.0);
                let initial_style = rune::tok::TextStyle::new(9.5, 700.0);
                let bubble = col()
                    .key(format!("{}-bubble", r.key))
                    .radius(14.0)
                    .fill(ink(fill))
                    .border(1.0, hairline)
                    .px(13.0)
                    .py(10.0)
                    .child(
                        text(name.to_string())
                            .key(format!("{}-name", r.key))
                            .font(name_style),
                    )
                    .child(text(r.body.clone()).font(body_style).wrap());
                let bubble = if r.role == "user" {
                    // Content-sized (no `grow`) + `justify_end` row hugs right
                    // like `.canvas-bubble-row.is-user`. `max_w` bounds the
                    // wrap: 468 viewport − 24 stack px − 22 avatar − 8 gap.
                    // Without it the bubble measures max-content and overflows.
                    bubble.max_w(414.0)
                } else {
                    bubble.grow(1.0).min_h(0.0)
                };
                let message = row()
                    .key(r.key.clone())
                    .role("bubble")
                    .label(r.role.to_string())
                    .w_full()
                    .gap(8.0)
                    .child(
                        rune::el()
                            .key(format!("{}-avatar", r.key))
                            .size(22.0, 22.0)
                            .radius(11.0)
                            .fill(ink(avatar_bg))
                            .center()
                            .child(text(initial.to_string()).font(initial_style)),
                    )
                    .child(bubble);
                if r.role == "user" {
                    message.justify_end()
                } else {
                    message
                }
            },
        );
    // Bounded root (window height) + growing viewport: the viewport takes the
    // free space between title and composer instead of pushing the composer
    // off-screen (an `h_full` viewport in an auto-height column does that).
    let viewport = col()
        .key("transcript-viewport")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: true })
        .child(stack);
    let mut root = col()
        .key("stark-transcript")
        .w_full()
        // `grow`, not `h_full`: fills a standalone window and shares a shell
        // column with the tab bar without overflowing it.
        .grow(1.0)
        .min_h(0.0)
        .fill(bg())
        .child(
            row()
                .key("transcript-head")
                .w_full()
                .items_center()
                .px(16.0)
                .pt(14.0)
                .pb(10.0)
                .child(
                    text(s.title.clone())
                        .key("transcript-title")
                        .grow(1.0)
                        .color(rune::color::hex("#f2f2f5").unwrap_or(rune::Color::BLACK)),
                )
                .child(
                    text("Settings".to_string())
                        .key("settings-open")
                        .role("button")
                        .label("Open settings")
                        .font(rune::tok::TextStyle::new(11.0, 500.0))
                        .color(rune::color::hex("#8a8a93").unwrap_or(rune::Color::BLACK))
                        .on_press(|s: &mut S| {
                            s.transcript_mut().settings_open = true;
                        }),
                ),
        )
        .child(viewport);
    if s.live {
        root = root.child(
            text("Running".to_string())
                .key("transcript-live")
                .px(16.0)
                .color(rune::color::hex("#ededf0").unwrap_or(rune::Color::BLACK)),
        );
    }
    // Composer card: shared `.premium-surface` dock (see `composer.rs`).
    // Send (button or Enter) stages the field as a user row, marks the run
    // live, and spawns the blocking `POST /api/run` thread; frames land in
    // `inbox`, drained by the `poll` hook before every frame.
    let composer_card = super::composer::composer_card(
        &s.model,
        &s.edit,
        |s: &mut S| &mut s.transcript_mut().edit,
        s.voice_live,
        |s: &mut S| {
            let t = s.transcript_mut();
            t.voice_live = !t.voice_live;
        },
        |s: &mut S| stage_send(s.transcript_mut()),
    );
    root = root.child(
        rune::el()
            .key("composer-dock")
            .w_full()
            .px(16.0)
            .pt(6.0)
            .pb(14.0)
            .child(composer_card),
    );
    if s.settings_open {
        root = root.child(settings_sheet::<S>(s));
    }
    root
}

/// Settings sheet: dim backdrop + centred card, focus-trapped while mounted.
/// Backdrop press and Escape both dismiss (Rune `modal` machinery).
fn settings_sheet<S: TranscriptHost + 'static>(s: &RuneTranscriptState) -> El {
    let ink = |hex: &'static str| rune::color::hex(hex).unwrap_or(rune::Color::BLACK);
    let label = rune::tok::TextStyle::new(12.0, 500.0);
    let value = rune::tok::TextStyle::new(12.0, 400.0);
    let setting_row = |key: &str, name: &str, val: String| {
        row()
            .key(format!("settings-row-{key}"))
            .w_full()
            .items_center()
            .py(9.0)
            .child(
                text(name.to_string())
                    .key(format!("settings-{key}-label"))
                    .grow(1.0)
                    .font(label)
                    .color(ink("#a1a1aa")),
            )
            .child(
                text(val)
                    .key(format!("settings-{key}-value"))
                    .font(value)
                    .color(ink("#e4e4e7")),
            )
    };
    rune::el()
        .key("settings-overlay")
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
        .modal(true, |s: &mut S| {
            s.transcript_mut().settings_open = false;
        })
        .child(
            col()
                .key("settings-card")
                .w(380.0)
                .radius(16.0)
                .fill(ink("#141417"))
                .border(1.0, ink("#ffffff14"))
                .px(20.0)
                .pt(16.0)
                .pb(16.0)
                .gap(4.0)
                .child(
                    row()
                        .key("settings-head")
                        .w_full()
                        .items_center()
                        .child(
                            text("Settings".to_string())
                                .key("settings-title")
                                .grow(1.0)
                                .font(rune::tok::TextStyle::new(13.0, 700.0))
                                .color(ink("#f4f4f5")),
                        )
                        .child(
                            text("×".to_string())
                                .key("settings-close")
                                .role("button")
                                .label("Close settings")
                                .font(rune::tok::TextStyle::new(14.0, 400.0))
                                .color(ink("#8a8a93"))
                                .on_press(|s: &mut S| {
                                    s.transcript_mut().settings_open = false;
                                }),
                        ),
                )
                .child(setting_row("model", "Model", s.model.clone()))
                .child(setting_row("theme", "Theme", "Dark".to_string()))
                .child(setting_row(
                    "backend",
                    "Backend",
                    format!("{}:{}", s.api_host, s.api_port),
                )),
        )
}

fn init_screenshot() -> RuneTranscriptState {
    RuneTranscriptState::screenshot()
}

fn apply_screenshot(s: &mut RuneTranscriptState) {
    *s = RuneTranscriptState::screenshot();
}

fn apply_stress(s: &mut RuneTranscriptState) {
    *s = RuneTranscriptState::stress();
}

fn apply_settings(s: &mut RuneTranscriptState) {
    *s = RuneTranscriptState::screenshot();
    s.settings_open = true;
}

/// `poll` hook: drain run frames queued by the transport thread into rows.
/// Returns true when state changed (requests another frame).
pub fn drain_run_inbox(s: &mut RuneTranscriptState, _clock: f64, _tx: &mut rune::Tx) -> bool {
    let mut changed = false;
    while let Some(frame) = s.inbox.as_ref().and_then(|rx| rx.try_recv().ok()) {
        crate::stream::apply_frame(s, &frame);
        changed = true;
    }
    if changed {
        // Sender gone and queue empty: run over, drop the inbox so idle frames
        // stop polling a dead channel.
        let dead = s.inbox.as_ref().is_some_and(|rx| {
            matches!(
                rx.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Disconnected)
            )
        });
        if dead {
            s.inbox = None;
        }
    }
    changed
}

/// AppSpec for headless `--snap` / windowed runs.
pub fn spec() -> AppSpec<RuneTranscriptState> {
    // Parity self-check (debug only): the pure window math must cover the whole
    // screenshot scene in its 720pt viewport — the Rune `.virtualize` stack and
    // `stark-kit::transcript::realize` may never disagree on the shipped scene.
    debug_assert!({
        use stark_kit::transcript::{realize, OVERSCAN};
        let rows = RuneTranscriptState::screenshot().transcript_rows();
        let w = realize(&rows, 0.0, 720.0, OVERSCAN);
        w.start == 0 && w.end == rows.len()
    });
    AppSpec {
        scenes: &[
            ("screenshot", apply_screenshot),
            ("stress", apply_stress),
            ("settings", apply_settings),
        ],
        poll: Some(drain_run_inbox),
        ..AppSpec::new(
            "Stark Transcript",
            (468.0, 720.0),
            init_screenshot,
            transcript_view,
        )
    }
}

/// Run headless (`--snap` renders PNG with the real wgpu renderer) or windowed
/// when no snap flag is present.
pub fn run_headless_or_windowed(extra_args: Vec<String>) {
    let app = rune::App::new(spec()).args(extra_args);
    app.run();
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn transcript_commits_and_rows_are_findable() {
        let env = Env::new(true, 2.0);
        let mut rt = Runtime::new(
            RuneTranscriptState::screenshot(),
            transcript_view,
            (468.0, 720.0),
            env,
        );
        rt.frame(0.0);
        for row in ["msg-0", "msg-1"] {
            assert!(rt.find(row).is_some(), "row {row} laid out");
            let presented = rt.presented(row).unwrap();
            assert!(
                presented.0[2] > 0.0 && presented.0[3] > 0.0,
                "row {row} has size"
            );
        }
        assert!(rt.find("composer-card").is_some());
        assert!(rt.find("composer-input").is_some());
        assert!(rt.find("composer-send").is_some());
        assert!(rt.find("model-pill").is_some());
    }

    #[test]
    fn typing_in_composer_edits_live_field() {
        use rune::{Key, KeyEvent, Mods};
        let env = Env::new(true, 2.0);
        let mut init = RuneTranscriptState::screenshot();
        init.edit = TextEdit::new("");
        init.edit.focused = true;
        let mut rt = Runtime::new(init, transcript_view, (468.0, 720.0), env);
        rt.frame(0.0);
        for ch in ["h", "i"] {
            rt.key(KeyEvent {
                key: Key::Text(ch.to_string()),
                mods: Mods::default(),
            });
            rt.frame(0.0);
        }
        assert_eq!(rt.state.edit.text, "hi");
    }

    #[test]
    fn enter_submits_staged_prompt() {
        use rune::{Key, KeyEvent, Mods};
        let env = Env::new(true, 2.0);
        let mut init = RuneTranscriptState::screenshot();
        init.edit = TextEdit::new("hello agent");
        init.edit.focused = true;
        init.live = false;
        init.api_port = 1;
        let n = init.rows.len();
        let mut rt = Runtime::new(init, transcript_view, (468.0, 720.0), env);
        rt.frame(0.0);
        rt.key(KeyEvent {
            key: Key::Enter,
            mods: Mods::default(),
        });
        rt.frame(0.0);
        assert_eq!(rt.state.rows.len(), n + 1);
        assert_eq!(rt.state.rows[n].body, "hello agent");
        assert!(rt.state.live);
        assert!(rt.state.edit.text.is_empty());
    }

    #[test]
    fn send_tap_stages_prompt_and_marks_live() {
        let env = Env::new(true, 2.0);
        let mut init = RuneTranscriptState::screenshot();
        init.edit = TextEdit::new("hello agent");
        init.live = false;
        // Closed port: the spawned run thread fails into the undrained channel
        // (Runtime::new wires no poll), so staging assertions stay exact and no
        // real server is ever touched.
        init.api_port = 1;
        let n = init.rows.len();
        let mut rt = Runtime::new(init, transcript_view, (468.0, 720.0), env);
        rt.frame(0.0);
        let (x, y) = rt.find("composer-send").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert_eq!(rt.state.rows.len(), n + 1);
        assert_eq!(rt.state.rows[n].role, "user");
        assert_eq!(rt.state.rows[n].body, "hello agent");
        assert!(rt.state.live);
        assert!(rt.state.edit.text.is_empty());
        assert!(rt.find(&rt.state.rows[n].key).is_some());
        assert!(rt.state.inbox.is_some(), "run thread owns the inbox");
    }

    #[test]
    fn poll_drain_applies_queued_frames_and_drops_dead_inbox() {
        use crate::stream::SseFrame;
        let mut s = RuneTranscriptState::screenshot();
        let n = s.rows.len();
        let (tx, rx) = std::sync::mpsc::channel();
        s.inbox = Some(rx);
        tx.send(SseFrame {
            event: "delta".to_string(),
            data: serde_json::json!({"kind":"text","text":"hi"}),
        })
        .unwrap();
        tx.send(SseFrame {
            event: "done".to_string(),
            data: serde_json::json!({"status":"ok"}),
        })
        .unwrap();
        drop(tx);
        let mut run_tx = rune::Tx::default();
        assert!(drain_run_inbox(&mut s, 0.0, &mut run_tx));
        assert_eq!(s.rows.len(), n + 1);
        assert_eq!(s.rows.last().unwrap().body, "hi");
        assert!(!s.live);
        assert!(s.inbox.is_none(), "dead inbox dropped");
        assert!(!drain_run_inbox(&mut s, 1.0, &mut run_tx));
    }

    #[test]
    fn settings_opens_closes_and_traps_background() {
        use rune::{Key, KeyEvent, Mods};
        let env = Env::new(true, 2.0);
        let mut rt = Runtime::new(
            RuneTranscriptState::screenshot(),
            transcript_view,
            (468.0, 720.0),
            env,
        );
        rt.frame(0.0);
        assert!(rt.find("settings-card").is_none());
        // Open from the title bar.
        let (x, y) = rt.find("settings-open").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert!(rt.state.settings_open);
        for key in ["settings-card", "settings-title", "settings-close"] {
            assert!(rt.find(key).is_some(), "{key} mounted");
        }
        // Background is inert: tapping send dismisses the sheet (backdrop)
        // instead of staging a row.
        let n = rt.state.rows.len();
        let (sx, sy) = rt.find("composer-send").unwrap();
        rt.pointer_down(sx, sy);
        rt.pointer_up(sx, sy);
        rt.frame(0.0);
        assert!(!rt.state.settings_open, "backdrop press dismisses");
        assert_eq!(rt.state.rows.len(), n, "no row staged behind the modal");
        // Reopen, then Escape dismisses via the modal machinery.
        let (x, y) = rt.find("settings-open").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert!(rt.state.settings_open);
        rt.key(KeyEvent {
            key: Key::Escape,
            mods: Mods::default(),
        });
        rt.frame(0.0);
        assert!(!rt.state.settings_open, "Escape dismisses");
    }

    #[test]
    fn voice_toggle_flips_live_dot() {
        let env = Env::new(true, 2.0);
        let mut rt = Runtime::new(
            RuneTranscriptState::screenshot(),
            transcript_view,
            (468.0, 720.0),
            env,
        );
        rt.frame(0.0);
        assert!(!rt.state.voice_live);
        let (x, y) = rt.find("composer-voice").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert!(rt.state.voice_live);
        assert!(rt.find("composer-voice-dot").is_some());
        let (x, y) = rt.find("composer-voice").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert!(!rt.state.voice_live);
    }

    #[test]
    fn stress_frame_mounts_tail_window_only() {
        use stark_kit::transcript::{realize, OVERSCAN};
        let s = RuneTranscriptState::stress();
        let rows = s.transcript_rows();
        assert_eq!(rows.len(), 10_000);
        // stick_bottom opens at the end: the pure window at the bottom offset
        // must be a small tail window, agreeing with Rune's mounted window.
        let total: f64 = rows.iter().map(|r| r.height() as f64).sum();
        let w = realize(&rows, total, 900.0, OVERSCAN);
        assert_eq!(w.end, 10_000);
        assert!(w.end - w.start < 60, "overscanned tail only: {w:?}");
        let env = Env::new(true, 2.0);
        let mut rt = Runtime::new(s, transcript_view, (468.0, 900.0), env);
        rt.frame(0.0);
        assert!(rt.find("msg-9999").is_some(), "tail row mounted");
        assert!(rt.find("msg-0").is_none(), "head row virtualized out");
        assert!(rt.find("composer-send").is_some());
    }
}
