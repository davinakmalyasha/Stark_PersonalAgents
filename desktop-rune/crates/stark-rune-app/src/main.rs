//! `stark-rune`: Stark desktop on Rune (byte-identical rewrite).
//!
//! Branch strategy: this crate lives on `rune-rewrite` only. `main` keeps the
//! frozen Tauri+React `frontend/` until pixel parity is proven via headless
//! goldens. Nothing here imports `frontend/` — the contract is the backend
//! HTTP API (`stark-server` on `127.0.0.1:8124`) plus in-process `stark-*`
//! crates for the future no-bridge build.
//!
//! Rune mapping (1:1 when the real `rune` dep is enabled):
//! - `El` below mirrors `rune`'s element (`key/role/label/checked/focusable`).
//! - `view()` mirrors `fn view(s:&State)->El` (`col/row/text/el`).
//! - Headless mirrors Rune's `render PNG + dump element tree + scripted input`.
//!   Here PNG is an SVG golden (no GPU needed in CI); element-tree dump and
//!   scripted input are byte-stable and agent-checkable.

use stark_kit::{draft_id, Role};
use stark_theme::{color, layout};

#[cfg(feature = "rune-real")]
mod rune_transcript;

// ---------------------------------------------------------------------------
// Backend contract (mirrors `frontend/src/services/api/*` + `types/index.ts`)
// ---------------------------------------------------------------------------

/// Base path. Desktop talks to `127.0.0.1:8124`; Vite PWA proxies `/api`.
pub const API_BASE: &str = "/api";
/// Resolved dev default (Rust server). Python legacy is `:8123`.
pub const DEV_API_ORIGIN: &str = "http://127.0.0.1:8124";

/// Frozen route table (subset the Rune UI needs first: transcript/canvas).
pub const ROUTES: &[(&str, &str)] = &[
    ("POST", "/api/run"),
    ("GET", "/api/stream/{chat_id}"),
    ("POST", "/api/abort"),
    ("GET", "/api/runs/active"),
    ("POST", "/api/steer"),
    ("GET", "/api/state"),
    ("GET", "/api/history"),
    ("GET", "/api/workspaces"),
    ("GET", "/api/workspaces/{id}/chats"),
    ("POST", "/api/workspaces/{id}/chats"),
    ("GET", "/api/ui/state"),
    ("PUT", "/api/ui/state"),
];

/// SSE event names (`streamPipeline` in `api/stream.ts`).
pub const SSE_EVENTS: &[&str] = &["state", "delta", "done", "error", "warning", "turn_aborted"];

// ---------------------------------------------------------------------------
// Minimal Rune-like element tree (swap for `rune::El` when online)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct El {
    pub key: String,
    pub role: String,
    pub label: String,
    pub text: String,
    pub children: Vec<El>,
}

impl El {
    pub fn new(key: &str, role: &str, label: &str) -> Self {
        Self {
            key: key.to_string(),
            role: role.to_string(),
            label: label.to_string(),
            text: String::new(),
            children: Vec::new(),
        }
    }

    pub fn with_text(mut self, text: &str) -> Self {
        self.text = text.to_string();
        self
    }

    pub fn child(mut self, el: El) -> Self {
        self.children.push(el);
        self
    }

    pub fn count(&self) -> usize {
        1 + self.children.iter().map(El::count).sum::<usize>()
    }

    pub fn dump(&self, indent: usize, out: &mut String) {
        let pad = "  ".repeat(indent);
        out.push_str(&format!(
            "{pad}{} role={} label={:?}",
            self.key, self.role, self.label
        ));
        if !self.text.is_empty() {
            let short: String = self.text.chars().take(60).collect();
            out.push_str(&format!(" text={short:?}"));
        }
        out.push('\n');
        for c in &self.children {
            c.dump(indent + 1, out);
        }
    }

    pub fn dump_string(&self) -> String {
        let mut s = String::new();
        self.dump(0, &mut s);
        s
    }
}

// ---------------------------------------------------------------------------
// App state (mirrors the screenshot + `House.tsx`/`CanvasStage.tsx`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ChatCard {
    pub workspace_id: String,
    pub chat_id: String,
    pub title: String,
    pub subtitle: String,
    pub x: f32,
    pub y: f32,
    pub compact: bool,
    pub running: bool,
    pub messages: Vec<(Role, String)>,
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub sidebar_filter: String,
    pub chats: Vec<(String, String)>, // (title, age)
    pub cards: Vec<ChatCard>,
    pub composer_text: String,
    pub composer_mode: String, // "Local" | "Cloud"
}

impl Default for AppState {
    /// Exact scene from the target screenshot.
    fn default() -> Self {
        Self {
            sidebar_filter: "General".to_string(),
            chats: vec![
                ("New Chat".to_string(), "45m".to_string()),
                ("Use personal_os action=…".to_string(), "58m".to_string()),
            ],
            cards: vec![
                ChatCard {
                    workspace_id: "ws-general".to_string(),
                    chat_id: "chat-1".to_string(),
                    title: "New Conversation".to_string(),
                    subtitle: "Orchestrator".to_string(),
                    x: 420.0,
                    y: 110.0,
                    compact: false,
                    running: false,
                    messages: vec![
                        (Role::User, "hey how much 452/5".to_string()),
                        (
                            Role::Agent,
                            "452 ÷ 5 = 90.4 (exactly 90 remainder 2). Want me to have Dev confirm that one out?"
                                .to_string(),
                        ),
                    ],
                },
                ChatCard {
                    workspace_id: "ws-general".to_string(),
                    chat_id: "chat-2".to_string(),
                    title: "New Conversation".to_string(),
                    subtitle: "Orchestrator".to_string(),
                    x: 830.0,
                    y: 130.0,
                    compact: false,
                    running: false,
                    messages: vec![],
                },
                ChatCard {
                    workspace_id: "ws-general".to_string(),
                    chat_id: "chat-3".to_string(),
                    title: "arithmetic 329+3823".to_string(),
                    subtitle: "Orchestrator".to_string(),
                    x: 430.0,
                    y: 380.0,
                    compact: true,
                    running: false,
                    messages: vec![
                        (Role::User, "hi davin".to_string()),
                        (
                            Role::Agent,
                            "Good to meet you, Davin. I'm your orchestrator — ask work to the right specialist."
                                .to_string(),
                        ),
                    ],
                },
                ChatCard {
                    workspace_id: "ws-general".to_string(),
                    chat_id: "chat-4".to_string(),
                    title: "New Chat".to_string(),
                    subtitle: "Orchestrator".to_string(),
                    x: 830.0,
                    y: 370.0,
                    compact: true,
                    running: true,
                    messages: vec![
                        (
                            Role::User,
                            "can you use a music api or smth, makes green green real, playing now in your tab, something like that, and play that in current youtube"
                                .to_string(),
                        ),
                        (
                            Role::Agent,
                            "Playing now in your YouTube tab: massive green day — Cinnamon Girl (8yr-old). A 20s pre-roll ad played first — I skipped it, and the actual track is running (04:48)."
                                .to_string(),
                        ),
                    ],
                },
            ],
            composer_text: String::new(),
            composer_mode: "Local".to_string(),
        }
    }
}

/// Scripted input for headless agent checks (mirrors Rune's scripted input).
#[derive(Debug, Clone)]
pub enum ScriptInput {
    TypeComposer(String),
    SwitchMode(String),
    ToggleRunning(String),
}

pub fn apply_input(state: &mut AppState, input: &ScriptInput) {
    match input {
        ScriptInput::TypeComposer(t) => state.composer_text = t.clone(),
        ScriptInput::SwitchMode(m) => state.composer_mode = m.clone(),
        ScriptInput::ToggleRunning(chat_id) => {
            if let Some(c) = state.cards.iter_mut().find(|c| &c.chat_id == chat_id) {
                c.running = !c.running;
            }
        }
    }
}

/// Build the Rune view tree. When the real `rune` dep lands, each node maps to
/// `col()/row()/text()/el()` with `Kit` tokens; structure stays identical.
pub fn view(s: &AppState) -> El {
    let mut root = El::new("stark-root", "window", "Stark");
    let mut sidebar = El::new("sidebar", "nav", &s.sidebar_filter).child(El::new(
        "search",
        "search",
        "Search chats...",
    ));
    for (i, (title, age)) in s.chats.iter().enumerate() {
        sidebar = sidebar.child(El::new(&format!("chat-row-{i}"), "row", title).with_text(age));
    }
    root = root.child(sidebar);

    let mut canvas = El::new("canvas", "canvas", "Agent canvas");
    for card in &s.cards {
        let id = draft_id("chat", &card.workspace_id, &card.chat_id);
        let mut el = El::new(&id, "card", &card.title).child(El::new(
            &format!("{id}-subtitle"),
            "label",
            &card.subtitle,
        ));
        for (j, (role, text)) in card.messages.iter().enumerate() {
            el = el
                .child(El::new(&format!("{id}-msg-{j}"), "bubble", role.as_str()).with_text(text));
        }
        if card.running {
            el = el.child(El::new(&format!("{id}-live"), "status", "Running"));
        }
        canvas = canvas.child(el);
    }
    root = root.child(canvas);

    let composer = El::new("composer", "composer", "Ask anything")
        .child(El::new(
            "composer-mode",
            "segment",
            &format!("mode={}", s.composer_mode),
        ))
        .child(El::new("composer-send", "button", "Send"));
    root.child(composer)
}

/// Write a byte-stable SVG golden approximating the layout with exact tokens.
/// SVG (not PNG) keeps CI GPU-free; the real Rune headless PNG replaces this
/// file 1:1 once `wgpu` rendering lands.
pub fn write_svg_golden(s: &AppState, path: &str) -> std::io::Result<()> {
    let w = 1280.0;
    let h = 720.0;
    let mut svg = String::new();
    svg.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">\n"
    ));
    svg.push_str(&format!(
        "<rect width=\"100%\" height=\"100%\" fill=\"{}\"/>\n",
        color::HOUSE_ROOT
    ));
    // Sidebar
    svg.push_str(&format!(
        "<rect x=\"0\" y=\"0\" width=\"{}\" height=\"{h}\" fill=\"{}\"/>\n",
        layout::SIDEBAR_W,
        color::SIDEBAR_BG
    ));
    svg.push_str(&format!(
        "<text x=\"16\" y=\"52\" fill=\"{}\" font-family=\"Inter\" font-size=\"15\" font-weight=\"600\">{}</text>\n",
        color::TEXT_PRIMARY,
        xml_escape(&s.sidebar_filter)
    ));
    for (i, (title, age)) in s.chats.iter().enumerate() {
        let y = 120.0 + i as f32 * 40.0;
        svg.push_str(&format!(
            "<text x=\"16\" y=\"{y}\" fill=\"{}\" font-family=\"Inter\" font-size=\"12\">{}</text>\n",
            color::TEXT_SECONDARY,
            xml_escape(title)
        ));
        svg.push_str(&format!(
            "<text x=\"200\" y=\"{y}\" fill=\"#555555\" font-family=\"Inter\" font-size=\"10\">{}</text>\n",
            xml_escape(age)
        ));
    }
    // Canvas dot grid (map spacing, faint white)
    let step = layout::GRID_MAP;
    let mut dots = String::new();
    let mut gx = step;
    while gx < w {
        let mut gy = 70.0;
        while gy < h - 130.0 {
            if gx > layout::SIDEBAR_W + 8.0 {
                dots.push_str(&format!(
                    "<circle cx=\"{gx}\" cy=\"{gy}\" r=\"1\" fill=\"rgba(255,255,255,0.075)\"/>\n"
                ));
            }
            gy += step;
        }
        gx += step;
    }
    svg.push_str(&dots);
    // Cards
    for card in &s.cards {
        let (cw, ch) = if card.compact {
            (layout::CARD_COMPACT_W, layout::CARD_COMPACT_H)
        } else {
            (layout::CARD_W, layout::CARD_H)
        };
        let r = if card.compact {
            layout::CARD_RADIUS_COMPACT
        } else {
            layout::CARD_RADIUS
        };
        let border = if card.running {
            "#ffffff"
        } else {
            "rgba(255,255,255,0.085)"
        };
        svg.push_str(&format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{cw}\" height=\"{ch}\" rx=\"{r}\" fill=\"#0b0b0e\" stroke=\"{border}\" stroke-width=\"1\"/>\n",
            card.x, card.y
        ));
        svg.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" fill=\"{}\" font-family=\"Inter\" font-size=\"{}\" font-weight=\"600\">{}</text>\n",
            card.x + 16.0,
            card.y + 30.0,
            color::TEXT_PRIMARY,
            if card.compact { 13.5 } else { 15.0 },
            xml_escape(&card.title)
        ));
        svg.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" fill=\"{}\" font-family=\"Inter\" font-size=\"11\">{}</text>\n",
            card.x + 16.0,
            card.y + 48.0,
            color::TEXT_SECONDARY,
            xml_escape(&card.subtitle)
        ));
    }
    // Composer
    svg.push_str(&format!(
        "<rect x=\"360\" y=\"600\" width=\"640\" height=\"96\" rx=\"18\" fill=\"rgba(16,16,19,0.95)\" stroke=\"{}\" stroke-width=\"1\"/>\n",
        color::HAIRLINE
    ));
    svg.push_str(
        "<text x=\"380\" y=\"630\" fill=\"#74747d\" font-family=\"Inter\" font-size=\"15\">Ask anything</text>\n",
    );
    svg.push_str(&format!(
        "<circle cx=\"1140\" cy=\"664\" r=\"17\" fill=\"{}\"/>\n",
        color::SEND_BG
    ));
    svg.push_str("</svg>\n");
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, svg)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn print_usage() {
    eprintln!("stark-rune --headless [--dump-tree <file>] [--golden <file>] [--script <type:<text>|mode:<Local|Cloud>|toggle:<chat_id>>]");
    eprintln!("stark-rune --rune-snap <png> [--rune-scene <screenshot|stress>] [--rune-scale <n>]  (requires --features rune-real; real wgpu PNG)");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(pos) = args.iter().position(|a| a == "--rune-snap") {
        #[cfg(not(feature = "rune-real"))]
        {
            let _ = pos;
            eprintln!("--rune-snap needs --features rune-real (pinned rune checkout + GPU).");
            std::process::exit(2);
        }
        #[cfg(feature = "rune-real")]
        {
            let png = args.get(pos + 1).cloned().unwrap_or_else(|| {
                eprintln!("--rune-snap <png> is required");
                std::process::exit(2);
            });
            let mut scene = String::from("screenshot");
            let mut scale = String::from("2");
            let mut k = 0;
            while k < args.len() {
                if args[k] == "--rune-scene" && k + 1 < args.len() {
                    scene = args[k + 1].clone();
                    k += 2;
                    continue;
                }
                if args[k] == "--rune-scale" && k + 1 < args.len() {
                    scale = args[k + 1].clone();
                    k += 2;
                    continue;
                }
                k += 1;
            }
            rune_transcript::run_headless_or_windowed(vec![
                "--snap".to_string(),
                png,
                "--scene".to_string(),
                scene,
                "--scale".to_string(),
                scale,
            ]);
            return;
        }
    }
    if !args.iter().any(|a| a == "--headless") {
        eprintln!("Stark Rune rewrite (branch `rune-rewrite`). Byte-identical UI lives here; `main` keeps Tauri+React.");
        eprintln!("Run headless: stark-rune --headless --dump-tree tree.txt --golden golden.svg");
        std::process::exit(2);
    }
    let mut state = AppState::default();
    // Minimal script parser for agent-driven checks.
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--script" && i + 1 < args.len() {
            let spec = &args[i + 1];
            if let Some(text) = spec.strip_prefix("type:") {
                apply_input(&mut state, &ScriptInput::TypeComposer(text.to_string()));
            } else if let Some(mode) = spec.strip_prefix("mode:") {
                apply_input(&mut state, &ScriptInput::SwitchMode(mode.to_string()));
            } else if let Some(id) = spec.strip_prefix("toggle:") {
                apply_input(&mut state, &ScriptInput::ToggleRunning(id.to_string()));
            }
            i += 2;
            continue;
        }
        i += 1;
    }
    let tree = view(&state);
    let mut dump = String::new();
    dump.push_str(&format!(
        "# stark-rune headless tree nodes={} routes={} sse={}\n",
        tree.count(),
        ROUTES.len(),
        SSE_EVENTS.len()
    ));
    dump.push_str(&tree.dump_string());
    let mut dump_path: Option<String> = None;
    let mut golden_path: Option<String> = None;
    let mut j = 0;
    while j < args.len() {
        if args[j] == "--dump-tree" && j + 1 < args.len() {
            dump_path = Some(args[j + 1].clone());
            j += 2;
            continue;
        }
        if args[j] == "--golden" && j + 1 < args.len() {
            golden_path = Some(args[j + 1].clone());
            j += 2;
            continue;
        }
        j += 1;
    }
    match dump_path {
        Some(p) => {
            if let Some(parent) = std::path::Path::new(&p).parent() {
                if !parent.as_os_str().is_empty() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        eprintln!("dump-tree mkdir failed: {e}");
                        std::process::exit(1);
                    }
                }
            }
            if let Err(e) = std::fs::write(&p, &dump) {
                eprintln!("dump-tree write failed: {e}");
                std::process::exit(1);
            }
            println!("wrote {p}");
        }
        None => print!("{dump}"),
    }
    if let Some(g) = golden_path {
        if let Err(e) = write_svg_golden(&state, &g) {
            eprintln!("golden write failed: {e}");
            std::process::exit(1);
        }
        println!("wrote {g}");
    }
    let _ = print_usage;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_matches_screenshot_scene() {
        let s = AppState::default();
        assert_eq!(s.chats.len(), 2);
        assert_eq!(s.cards.len(), 4);
        let tree = view(&s);
        let dump = tree.dump_string();
        assert!(dump.contains("New Conversation"));
        assert!(dump.contains("arithmetic 329+3823"));
        assert!(dump.contains("composer"));
        // 1 root + 1 sidebar + search + 2 rows + 1 canvas + 4 cards + subtitles/messages/status + composer(3)
        assert!(tree.count() >= 20);
    }

    #[test]
    fn script_input_is_deterministic() {
        let mut s = AppState::default();
        apply_input(&mut s, &ScriptInput::TypeComposer("hello".into()));
        apply_input(&mut s, &ScriptInput::SwitchMode("Cloud".into()));
        apply_input(&mut s, &ScriptInput::ToggleRunning("chat-1".into()));
        assert_eq!(s.composer_text, "hello");
        assert_eq!(s.composer_mode, "Cloud");
        assert!(
            s.cards
                .iter()
                .find(|c| c.chat_id == "chat-1")
                .unwrap()
                .running
        );
        let dump = view(&s).dump_string();
        assert!(dump.contains("mode=Cloud"));
    }

    #[test]
    fn routes_cover_transcript_contract() {
        let routes: Vec<String> = ROUTES.iter().map(|(m, p)| format!("{m} {p}")).collect();
        for need in [
            "POST /api/run",
            "GET /api/stream/{chat_id}",
            "POST /api/steer",
            "GET /api/runs/active",
        ] {
            assert!(routes.iter().any(|r| r == need), "missing {need}");
        }
        for ev in ["state", "delta", "done"] {
            assert!(SSE_EVENTS.contains(&ev));
        }
    }
}
