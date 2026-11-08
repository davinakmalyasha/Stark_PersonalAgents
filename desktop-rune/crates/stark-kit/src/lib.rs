//! `stark-kit`: generic, themable Rune components for Stark.
//!
//! Mirrors `@opencode/ui`-style naming from the Rune `rune-kit` pattern, but
//! with Stark's byte-identical geometry. Ports the pure logic of:
//! - `frontend/src/canvas/canvasTypes.ts` (`draftId`, `makeItem`, `DEFAULT_VIEW`)
//! - `frontend/src/canvas/CanvasStage.tsx` (`clamp`, wheel zoom, `fitView`,
//!   `focusChat`, edge bezier path, auto-map sync signature)
//!
//! No raw style literals here — sizes come from `stark-theme`.

use stark_theme::layout;

/// Clamp helper — identical to `CanvasStage.tsx`.
pub fn clamp(value: f32, min: f32, max: f32) -> f32 {
    value.max(min).min(max)
}

/// Canvas camera. Mirrors `CanvasView { x, y, scale }`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CanvasView {
    pub x: f32,
    pub y: f32,
    pub scale: f32,
}

impl Default for CanvasView {
    fn default() -> Self {
        Self {
            x: layout::DEFAULT_VIEW_X,
            y: layout::DEFAULT_VIEW_Y,
            scale: layout::DEFAULT_VIEW_SCALE,
        }
    }
}

impl CanvasView {
    /// Wheel zoom about a screen-space cursor point.
    /// Port of `onWheel` in `CanvasStage.tsx`.
    pub fn wheel_zoom(&self, delta_y: f32, cursor: (f32, f32)) -> Self {
        let factor = if delta_y < 0.0 { 1.08 } else { 0.92 };
        let scale = clamp(self.scale * factor, layout::ZOOM_MIN, layout::ZOOM_MAX);
        let (cx, cy) = cursor;
        let world_x = (cx - self.x) / self.scale;
        let world_y = (cy - self.y) / self.scale;
        Self {
            scale,
            x: cx - world_x * scale,
            y: cy - world_y * scale,
        }
    }

    /// Button zoom about the viewport centre. Port of `zoomBy`.
    pub fn zoom_by(&self, factor: f32, viewport: (f32, f32)) -> Self {
        let scale = clamp(self.scale * factor, layout::ZOOM_MIN, layout::ZOOM_MAX);
        let (w, h) = viewport;
        let cx = w / 2.0;
        let cy = h / 2.0;
        let world_x = (cx - self.x) / self.scale;
        let world_y = (cy - self.y) / self.scale;
        Self {
            scale,
            x: cx - world_x * scale,
            y: cy - world_y * scale,
        }
    }

    pub fn screen_to_world(&self, sx: f32, sy: f32) -> (f32, f32) {
        ((sx - self.x) / self.scale, (sy - self.y) / self.scale)
    }

    pub fn world_to_screen(&self, wx: f32, wy: f32) -> (f32, f32) {
        (wx * self.scale + self.x, wy * self.scale + self.y)
    }
}

fn card_size(compact: bool) -> (f32, f32) {
    if compact {
        (layout::CARD_COMPACT_W, layout::CARD_COMPACT_H)
    } else {
        (layout::CARD_W, layout::CARD_H)
    }
}

/// Fit all items into the viewport. Port of `fitView`.
/// `items`: (x, y, compact). `viewport`: (w, h). `is_map`: top padding 120 vs 60.
pub fn fit_view(
    items: &[(f32, f32, bool)],
    viewport: (f32, f32),
    is_map: bool,
) -> Option<CanvasView> {
    if items.is_empty() {
        return None;
    }
    let (vw, vh) = viewport;
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for (x, y, compact) in items {
        let (w, h) = card_size(*compact);
        min_x = min_x.min(*x);
        min_y = min_y.min(*y);
        max_x = max_x.max(*x + w);
        max_y = max_y.max(*y + h);
    }
    if !min_x.is_finite() {
        return None;
    }
    let padding_x = if vw < 720.0 { 36.0 } else { 90.0 };
    let padding_top = if is_map { 120.0 } else { 60.0 };
    let padding_bottom = 70.0;
    let width = (max_x - min_x).max(1.0);
    let height = (max_y - min_y).max(1.0);
    let scale = clamp(
        ((vw - padding_x * 2.0) / width).min((vh - padding_top - padding_bottom) / height),
        layout::FIT_ZOOM_MIN,
        layout::FIT_ZOOM_MAX,
    );
    let center_x = (min_x + max_x) / 2.0;
    let center_y = (min_y + max_y) / 2.0;
    Some(CanvasView {
        scale,
        x: vw / 2.0 - center_x * scale,
        y: padding_top + (vh - padding_top - padding_bottom) / 2.0 - center_y * scale,
    })
}

/// Focus one card. Port of `focusChat`.
pub fn focus_card(view: &CanvasView, target: (f32, f32, bool), viewport: (f32, f32)) -> CanvasView {
    let (tx, ty, compact) = target;
    let (w, h) = card_size(compact);
    let (vw, vh) = viewport;
    let scale = clamp(view.scale, 0.65, 1.15);
    CanvasView {
        scale,
        x: vw / 2.0 - (tx + w / 2.0) * scale,
        y: vh / 2.0 - (ty + h / 2.0) * scale,
    }
}

/// Edge bezier between a parent bottom-centre and child top-centre.
/// Port of the `<path d={...}>` computation in `CanvasStage.tsx`.
pub fn edge_path(parent: (f32, f32, bool), child: (f32, f32, bool)) -> String {
    let (px, py, p_compact) = parent;
    let (cx, cy, c_compact) = child;
    let (pw, ph) = card_size(p_compact);
    let (cw, _) = card_size(c_compact);
    let sx = px + pw / 2.0;
    let sy = py + ph;
    let tx = cx + cw / 2.0;
    let ty = cy;
    let bend = (34.0f32).max((ty - sy) * 0.48);
    format!(
        "M {sx} {sy} C {sx} {}, {tx} {}, {tx} {ty}",
        sy + bend,
        ty - bend
    )
}

/// Canvas item id. Port of `draftId` in `canvasTypes.ts`.
pub fn draft_id(kind: &str, workspace_id: &str, id: &str) -> String {
    match kind {
        "chat" => format!("chat:{workspace_id}:{id}"),
        "project" => format!("project:{workspace_id}:{id}"),
        _ => format!("addon:{workspace_id}:{id}"),
    }
}

/// Chat roles in the transcript (mirrors `ChatMessage.role`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Agent,
    Error,
    System,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Agent => "agent",
            Role::Error => "error",
            Role::System => "system",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_matches_ts() {
        assert_eq!(clamp(2.0, 0.45, 1.8), 1.8);
        assert_eq!(clamp(0.1, 0.45, 1.8), 0.45);
        assert_eq!(clamp(1.0, 0.45, 1.8), 1.0);
    }

    #[test]
    fn wheel_zoom_preserves_cursor_world_point() {
        let v = CanvasView {
            x: 48.0,
            y: 48.0,
            scale: 1.0,
        };
        let cursor = (400.0, 300.0);
        let (wx, wy) = v.screen_to_world(cursor.0, cursor.1);
        let next = v.wheel_zoom(-100.0, cursor);
        let (wx2, wy2) = next.screen_to_world(cursor.0, cursor.1);
        assert!((wx - wx2).abs() < 0.01);
        assert!((wy - wy2).abs() < 0.01);
        assert!((next.scale - 1.08).abs() < 0.001);
    }

    #[test]
    fn fit_view_single_card_centres() {
        let view = fit_view(&[(0.0, 0.0, false)], (1280.0, 800.0), true).unwrap();
        assert!(view.scale >= layout::FIT_ZOOM_MIN && view.scale <= layout::FIT_ZOOM_MAX);
        // Card centre should map near viewport centre horizontally.
        let (sx, _) = view.world_to_screen(234.0, 210.0);
        assert!((sx - 640.0).abs() < 2.0);
    }

    #[test]
    fn edge_path_bend_floor() {
        let d = edge_path((0.0, 0.0, false), (0.0, 430.0, false));
        // sy=420, ty=430 -> bend = max(34, 4.8) = 34
        assert!(d.starts_with("M 234 420 C 234 454, 234 396, 234 430"));
    }

    #[test]
    fn draft_ids_match_ts() {
        assert_eq!(draft_id("chat", "ws1", "c1"), "chat:ws1:c1");
        assert_eq!(draft_id("project", "ws1", "p1"), "project:ws1:p1");
        assert_eq!(draft_id("addon", "ws1", "design"), "addon:ws1:design");
    }
}
