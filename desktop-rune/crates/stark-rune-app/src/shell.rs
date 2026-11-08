//! Unified shell — transcript and canvas map under one `AppSpec` with route tabs.
//!
//! Mirrors `frontend/src/AppShell.tsx` hash routes (`home` map + `coding`
//! transcript): one window, one state, tab bar switching. Sub-views stay
//! mounted per route (no cross-route element keys), and both states survive
//! switching — a running stream keeps draining while the map is up.

use rune::{col, row, text, AppSpec, Cx, El};

use crate::rune_canvas::{canvas_body, CanvasHost, CanvasState};
use crate::rune_transcript::{
    drain_run_inbox, transcript_body, RuneTranscriptState, TranscriptHost,
};

/// Shell routes. `Map` is the home canvas; `Chat` is the transcript room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Route {
    #[default]
    Map,
    Chat,
}

#[derive(Debug, Clone)]
pub struct ShellState {
    pub route: Route,
    pub transcript: RuneTranscriptState,
    pub canvas: CanvasState,
}

impl Default for ShellState {
    fn default() -> Self {
        Self {
            route: Route::Map,
            transcript: RuneTranscriptState::screenshot(),
            canvas: CanvasState::map(),
        }
    }
}

impl TranscriptHost for ShellState {
    fn transcript(&self) -> &RuneTranscriptState {
        &self.transcript
    }
    fn transcript_mut(&mut self) -> &mut RuneTranscriptState {
        &mut self.transcript
    }
}

impl CanvasHost for ShellState {
    fn canvas_mut(&mut self) -> &mut CanvasState {
        &mut self.canvas
    }
}

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

fn tab(s: &ShellState, route: Route, label: &str, key: &str) -> El {
    let active = s.route == route;
    let mut el = text(label.to_string())
        .key(key.to_string())
        .role("tab")
        .font(rune::tok::TextStyle::new(12.0, 600.0))
        .px(14.0)
        .py(8.0)
        .radius(8.0)
        .color(ink(if active { "#f4f4f5" } else { "#71717a" }))
        .on_press(move |s: &mut ShellState| {
            s.route = route;
        });
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

pub fn shell_view(s: &ShellState, _: &mut Cx<ShellState>) -> El {
    let body = match s.route {
        Route::Map => canvas_body::<ShellState>(&s.canvas),
        Route::Chat => transcript_body::<ShellState>(&s.transcript),
    };
    col()
        .key("stark-shell")
        .w_full()
        .h_full()
        .fill(ink("#0a0a0a"))
        .child(
            row()
                .key("shell-tabs")
                .w_full()
                .items_center()
                .gap(4.0)
                .px(12.0)
                .pt(10.0)
                .pb(6.0)
                .child(tab(s, Route::Map, "Map", "tab-map"))
                .child(tab(s, Route::Chat, "Chat", "tab-chat")),
        )
        .child(body)
}

/// `poll` drains the transcript inbox whatever route is up, so a run started
/// in Chat keeps streaming while the Map is shown.
pub fn shell_poll(s: &mut ShellState, clock: f64, tx: &mut rune::Tx) -> bool {
    drain_run_inbox(&mut s.transcript, clock, tx)
}

fn init_shell() -> ShellState {
    ShellState::default()
}

fn apply_map(s: &mut ShellState) {
    s.route = Route::Map;
}

fn apply_chat(s: &mut ShellState) {
    s.route = Route::Chat;
}

pub fn shell_spec() -> AppSpec<ShellState> {
    let mut spec = AppSpec::new("Stark", (1280.0, 900.0), init_shell, shell_view);
    spec.scenes = &[("map", apply_map), ("chat", apply_chat)];
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
    fn tabs_switch_routes_and_preserve_state() {
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(ShellState::default(), shell_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        // Map first: canvas keys present, transcript rows absent.
        assert!(rt.find("canvas-world").is_some());
        assert!(rt.find("transcript-viewport").is_none());
        // Type in Chat without leaving Map? Switch first, then type.
        let (x, y) = rt.find("tab-chat").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert_eq!(rt.state.route, Route::Chat);
        assert!(rt.find("transcript-viewport").is_some());
        assert!(rt.find("canvas-world").is_none());
        // Map state survives the round trip.
        let (x, y) = rt.find("tab-map").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert_eq!(rt.state.route, Route::Map);
        assert!(rt.find("chat:ws-general:chat-1").is_some());
    }

    #[test]
    fn shell_send_and_poll_stream_while_on_map() {
        use crate::stream::SseFrame;
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(ShellState::default(), shell_view, (1280.0, 900.0), env);
        rt.frame(0.0);
        // Stage a run from Chat, flip to Map, drain via shell poll.
        let (x, y) = rt.find("tab-chat").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        rt.state.transcript.api_port = 1;
        rt.state.transcript.edit.text = "hi".to_string();
        crate::rune_transcript::stage_send(&mut rt.state.transcript);
        assert!(rt.state.transcript.live);
        let (x, y) = rt.find("tab-map").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        // Simulate the run thread delivering frames; poll applies them on Map.
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
}
