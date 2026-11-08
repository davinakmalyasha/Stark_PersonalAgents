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

use rune::{col, row, text, AppSpec, Cx, Drag, El, LineCap, Path, Phase, Shape};

pub const MAP_W: f32 = 1280.0;
pub const MAP_H: f32 = 900.0;
pub const MAP_SIDEBAR_W: f32 = 248.0;
pub const DOT_STEP: f32 = 26.0;

#[derive(Debug, Clone)]
pub struct MapCard {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub preview: String,
    pub x: f32,
    pub y: f32,
    pub compact: bool,
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

#[derive(Debug, Clone, Default)]
pub struct CanvasState {
    pub cards: Vec<MapCard>,
    pub edges: Vec<(String, String)>,
    pub press_home: Option<(f32, f32)>,
}

impl CanvasState {
    /// Screenshot scene: the four home cards + one spawn edge.
    pub fn map() -> Self {
        Self {
            cards: vec![
                MapCard {
                    id: "chat:ws-general:chat-1".to_string(),
                    title: "New Conversation".to_string(),
                    subtitle: "General · Orchestrator".to_string(),
                    preview: "452 ÷ 5 = 90.4 (exactly 90 remainder 2)".to_string(),
                    x: 36.0,
                    y: 36.0,
                    compact: false,
                },
                MapCard {
                    id: "chat:ws-general:chat-2".to_string(),
                    title: "New Conversation".to_string(),
                    subtitle: "General · Orchestrator".to_string(),
                    preview: "No messages yet.".to_string(),
                    x: 520.0,
                    y: 36.0,
                    compact: false,
                },
                MapCard {
                    id: "chat:ws-general:chat-3".to_string(),
                    title: "arithmetic 329+3823".to_string(),
                    subtitle: "General · spawned".to_string(),
                    preview: "Good to meet you, Davin.".to_string(),
                    x: 60.0,
                    y: 480.0,
                    compact: true,
                },
                MapCard {
                    id: "chat:ws-general:chat-4".to_string(),
                    title: "New Chat".to_string(),
                    subtitle: "General · running".to_string(),
                    preview: "Playing now in your YouTube tab…".to_string(),
                    x: 560.0,
                    y: 480.0,
                    compact: true,
                },
            ],
            edges: vec![(
                "chat:ws-general:chat-1".to_string(),
                "chat:ws-general:chat-3".to_string(),
            )],
            press_home: None,
        }
    }

    pub fn card_mut(&mut self, id: &str) -> Option<&mut MapCard> {
        self.cards.iter_mut().find(|c| c.id == id)
    }

    pub fn card(&self, id: &str) -> Option<&MapCard> {
        self.cards.iter().find(|c| c.id == id)
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

fn card_view(card: &MapCard) -> El {
    let (w, h) = card.size();
    let id = card.id.clone();
    let drag_id = card.id.clone();
    col()
        .key(card.id.clone())
        .absolute(card.x, card.y)
        .size(w, h)
        .radius(if card.compact { 16.0 } else { 18.0 })
        .fill(ink("#0b0b0e"))
        .border(1.0, ink("#ffffff14"))
        .child(
            // Drag handle: header only, like `[data-canvas-drag-handle]`.
            row()
                .key(format!("{id}-head"))
                .w_full()
                .items_center()
                .gap(10.0)
                .px(14.0)
                .pt(14.0)
                .pb(10.0)
                .on_drag(move |s: &mut CanvasState, drag: &mut Drag, _tx| {
                    let home = s.press_home;
                    let Some(card) = s.card_mut(&drag_id) else {
                        return;
                    };
                    match drag.phase {
                        Phase::Down => {
                            s.press_home = Some((card.x, card.y));
                        }
                        Phase::Move | Phase::Up => {
                            if let Some((hx, hy)) = home {
                                card.x = hx + drag.dx;
                                card.y = hy + drag.dy;
                            }
                            if matches!(drag.phase, Phase::Up) {
                                s.press_home = None;
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
                                .font(rune::tok::TextStyle::new(15.0, 600.0))
                                .color(ink("#f2f2f5")),
                        )
                        .child(
                            text(card.subtitle.clone())
                                .key(format!("{id}-subtitle"))
                                .font(rune::tok::TextStyle::new(11.0, 400.0))
                                .color(ink("#8a8a93")),
                        ),
                ),
        )
        .child(
            text(card.preview.clone())
                .key(format!("{id}-preview"))
                .px(16.0)
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .wrap()
                .color(ink("#92959e")),
        )
}

pub fn canvas_view(s: &CanvasState, _: &mut Cx<CanvasState>) -> El {
    let dot = rune::Color {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 0.075,
    };
    let mut canvas = col()
        .key("canvas-world")
        .grow(1.0)
        .h_full()
        .fill(ink("#08080a"));
    // Dotted grid tile (26pt, like `canvas.css .canvas-grid`).
    let mut gy = DOT_STEP;
    while gy < MAP_H {
        let mut gx = DOT_STEP;
        while gx < MAP_W - MAP_SIDEBAR_W {
            canvas = canvas.child(
                rune::el()
                    .key(format!("dot-{gx:.0}x{gy:.0}"))
                    .absolute(gx, gy)
                    .size(2.0, 2.0)
                    .radius(1.0)
                    .fill(dot),
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
    canvas = canvas.child(
        rune::el()
            .key("canvas-edges")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .inert()
            .shape(shape),
    );
    for card in &s.cards {
        canvas = canvas.child(card_view(card));
    }

    row()
        .key("stark-map")
        .w_full()
        .h_full()
        .fill(ink("#0a0a0a"))
        .child(
            col()
                .key("map-sidebar")
                .size(MAP_SIDEBAR_W, MAP_H)
                .fill(ink("#0f0f0f"))
                .px(16.0)
                .pt(14.0)
                .gap(8.0)
                .child(
                    text("General".to_string())
                        .key("map-filter")
                        .font(rune::tok::TextStyle::new(15.0, 600.0))
                        .color(ink("#fafafa")),
                )
                .child(
                    text("Search chats...".to_string())
                        .key("map-search")
                        .font(rune::tok::TextStyle::new(12.0, 400.0))
                        .color(ink("#8a8a8a")),
                )
                .child(
                    text("New Chat · 45m".to_string())
                        .key("map-row-0")
                        .font(rune::tok::TextStyle::new(12.0, 500.0))
                        .color(ink("#d4d4d8")),
                )
                .child(
                    text("Use personal_os action=… · 58m".to_string())
                        .key("map-row-1")
                        .font(rune::tok::TextStyle::new(12.0, 500.0))
                        .color(ink("#8a8a8a")),
                ),
        )
        .child(canvas)
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
            "map-sidebar",
        ] {
            assert!(rt.find(key).is_some(), "{key} laid out");
        }
        let (rect, _) = rt.presented("chat:ws-general:chat-1").unwrap();
        assert!((rect[3] - 420.0).abs() < 2.0, "full card height 420");
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
