//! Virtualized transcript window — port of `MessageStream.tsx` rendering math.
//!
//! Rune mapping (1:1 when the real `rune` dep builds):
//! - rows are a growth stack: `.growth(GrowthSpec)` + `.virtualize(EST)` as the
//!   scroll viewport's first child, `.scroll(ScrollSpec { stick_bottom })`.
//! - rows carry unique keys (`message.id`); vertical rhythm is internal row
//!   padding, never stack gaps/margins (unvisited estimates must stay valid).
//! - measured corrections preserve the visible key; bottom stays exact;
//!   render coordinates rebase near the window; extents use `f64`.
//!
//! This module is the pure, GPU-free half: height estimation, window
//! computation, `stick_bottom`. The Rune view layer consumes it directly.

use super::Role;

/// Estimated row height used before a row is measured.
/// Mirrors Rune's `.virtualize(estimated_row_height)`.
pub const ESTIMATED_ROW_H: f32 = 96.0;
/// Overscan rows above/below the viewport (realized but off-screen).
pub const OVERSCAN: usize = 4;
/// Internal row padding (top+bottom). Rows must not use external margins.
pub const ROW_PAD_Y: f32 = 14.0;

/// One transcript row. `key` must be unique (message id).
#[derive(Debug, Clone)]
pub struct TranscriptRow {
    pub key: String,
    pub role: Role,
    /// Estimated height until measured.
    pub estimated_h: f32,
    /// Measured height once laid out (`None` = unvisited).
    pub measured_h: Option<f32>,
    /// Collapsed trace groups hide detail rows (zero height, still keyed).
    pub collapsed: bool,
}

impl TranscriptRow {
    pub fn new(key: &str, role: Role, content_len: usize) -> Self {
        Self {
            key: key.to_string(),
            role,
            estimated_h: estimate_row_height(role, content_len),
            measured_h: None,
            collapsed: false,
        }
    }

    pub fn height(&self) -> f32 {
        if self.collapsed {
            0.0
        } else {
            self.measured_h.unwrap_or(self.estimated_h)
        }
    }
}

/// Deterministic height estimate from role + content length.
/// Calibrated to the frozen CSS: user bubble `12.5px/1.55` + `10px` padding,
/// agent `activity-timeline` rows + trace shell + response block.
pub fn estimate_row_height(role: Role, content_len: usize) -> f32 {
    // ~48 chars per wrapped line at card width; 20px per line.
    let lines = ((content_len.max(1) as f32) / 48.0).ceil();
    let base = match role {
        Role::User => 44.0,
        Role::Agent => 118.0,
        Role::Error => 64.0,
        Role::System => 36.0,
    };
    base + ROW_PAD_Y + lines * 20.0
}

/// Realized window into the transcript: `[start, end)` row indices plus the
/// `y` offset of `start` in content points (f64 extents, rebased near window).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RealizedWindow {
    pub start: usize,
    pub end: usize,
    pub start_y: f64,
    pub total_h: f64,
}

/// Compute the overscanned window for `scroll_offset`/`viewport_h`.
/// Unvisited rows use estimates; measured rows use real heights.
/// Bottom clamps exact (never overshoots `total_h`).
pub fn realize(
    rows: &[TranscriptRow],
    scroll_offset: f64,
    viewport_h: f64,
    overscan: usize,
) -> RealizedWindow {
    if rows.is_empty() || viewport_h <= 0.0 {
        return RealizedWindow {
            start: 0,
            end: 0,
            start_y: 0.0,
            total_h: 0.0,
        };
    }
    // Prefix extents in f64 (Rune: scroll extents use double precision).
    let mut edges: Vec<f64> = Vec::with_capacity(rows.len() + 1);
    edges.push(0.0);
    for r in rows {
        let last = *edges.last().unwrap();
        edges.push(last + r.height() as f64);
    }
    let total_h = *edges.last().unwrap();
    let offset = scroll_offset.clamp(0.0, total_h.max(0.0));
    // First row whose bottom edge passes the offset.
    let mut first = rows.len();
    for (i, _) in rows.iter().enumerate() {
        if edges[i + 1] > offset {
            first = i;
            break;
        }
    }
    if first == rows.len() {
        first = rows.len().saturating_sub(1);
    }
    // Last row whose top edge is above offset+viewport.
    let bottom = offset + viewport_h;
    let mut last = first;
    for (i, _) in rows.iter().enumerate() {
        if edges[i] < bottom {
            last = i + 1;
        }
    }
    let start = first.saturating_sub(overscan);
    let end = (last + overscan).min(rows.len());
    RealizedWindow {
        start,
        end,
        start_y: edges[start],
        total_h,
    }
}

/// `stick_bottom` follow-the-end: while pinned, appending rows keeps the
/// viewport glued to the bottom (streaming transcript). Any user scroll-up
/// unpins; `scroll_to_bottom` re-pins.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StickBottom {
    pub pinned: bool,
}

impl StickBottom {
    pub fn pinned() -> Self {
        Self { pinned: true }
    }
    /// Wheel/drag scroll: unpin unless already at the bottom edge.
    pub fn on_user_scroll(&mut self, scroll_offset: f64, total_h: f64, viewport_h: f64) {
        let at_bottom = scroll_offset + viewport_h >= total_h - 1.0;
        if !at_bottom {
            self.pinned = false;
        }
    }
    pub fn scroll_to_bottom(&mut self, total_h: f64, viewport_h: f64) -> f64 {
        self.pinned = true;
        (total_h - viewport_h).max(0.0)
    }
    /// After new rows arrive, keep the offset glued if pinned.
    pub fn follow(&self, total_h: f64, viewport_h: f64, current: f64) -> f64 {
        if self.pinned {
            (total_h - viewport_h).max(0.0)
        } else {
            current
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(n: usize) -> Vec<TranscriptRow> {
        (0..n)
            .map(|i| {
                let role = if i % 3 == 0 { Role::User } else { Role::Agent };
                TranscriptRow::new(&format!("msg-{i}"), role, 120 + (i % 7) * 40)
            })
            .collect()
    }

    #[test]
    fn estimates_differ_by_role() {
        let u = estimate_row_height(Role::User, 100);
        let a = estimate_row_height(Role::Agent, 100);
        assert!(a > u, "agent rows carry trace chrome: {a} vs {u}");
        assert!(estimate_row_height(Role::System, 10) < u);
    }

    #[test]
    fn window_realizes_subset_of_10k() {
        let rows = fixture(10_000);
        let w = realize(&rows, 0.0, 800.0, OVERSCAN);
        assert!(w.end - w.start < 40, "only overscanned window: {:?}", w);
        assert_eq!(w.start, 0);
        // Total extent is the sum of estimates.
        let expected: f64 = rows.iter().map(|r| r.height() as f64).sum();
        assert!((w.total_h - expected).abs() < 0.01);
    }

    #[test]
    fn measured_correction_keeps_bottom_exact() {
        let mut rows = fixture(200);
        // Measure the first 20 rows 30px taller (code blocks expand).
        for r in rows.iter_mut().take(20) {
            r.measured_h = Some(r.estimated_h + 30.0);
        }
        let w = realize(&rows, 1e12, 800.0, OVERSCAN);
        assert_eq!(w.end, 200);
        let expected: f64 = rows.iter().map(|r| r.height() as f64).sum();
        assert!((w.total_h - expected).abs() < 0.01);
        assert!(w.start_y <= w.total_h);
    }

    #[test]
    fn stick_bottom_follows_stream_then_unpins_on_scroll_up() {
        let mut stick = StickBottom::pinned();
        let total = 5000.0;
        let view = 800.0;
        let glued = stick.follow(total + 400.0, view, 4200.0);
        assert_eq!(glued, 4600.0);
        stick.on_user_scroll(1000.0, total + 400.0, view);
        assert!(!stick.pinned);
        // Unpinned: offset untouched by new rows.
        assert_eq!(stick.follow(total + 800.0, view, 1000.0), 1000.0);
        // Re-pin.
        assert_eq!(stick.scroll_to_bottom(total + 800.0, view), 5000.0);
    }

    #[test]
    fn collapsed_trace_rows_take_no_space_but_keep_keys() {
        let mut rows = fixture(10);
        rows[3].collapsed = true;
        assert_eq!(rows[3].height(), 0.0);
        let w = realize(&rows, 0.0, 10_000.0, 0);
        assert_eq!(w.end - w.start, 10, "keys stay mounted");
    }
}
