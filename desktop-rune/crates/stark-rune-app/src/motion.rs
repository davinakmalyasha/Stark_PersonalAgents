//! Shared CSS timing tokens from `frontend/src/index.css`.
//!
//! Two families live here, because Rune splits what CSS calls a "transition":
//!
//! - [`Curve`]s drive the paint-only **hover tracks** (`hover`, `group_color`,
//!   `group_opacity`). Tailwind's default is 150 ms
//!   `cubic-bezier(.4,0,.2,1)`.
//! - [`Spring`]s drive **presence transitions** (message entrances, menus,
//!   panels) and [`Live`] hover smoothing. CSS keyframes are time-based, so a
//!   spring is matched by duration and verified at settle; under Reduce Motion
//!   its spatial offset snaps while the opacity still fades.
//!
//! [`HoverSet`] owns the per-element `Live` tracks for properties CSS can
//! transition but Rune has no hover binding for: scale, offset and border
//! colour. `on_hover_change` publishes the target; `tx.live_only()` keeps the
//! pointer from rebuilding the view.
//!
//! The later `index.css` cascade wins over the earlier base rules: user
//! messages are 200 ms, agent messages 240 ms, generic fades 140 ms, modal
//! panels 240 ms, menus 160 ms, trace shells 340 ms, sidebar rows 260 ms.
//!
//! Every presence transition here removes with [`Spring::NONE`]: React
//! unmounts menus and modals in the frame they close, so the removal effect is
//! identity and the exit lands at once. Only accordions keep a two-way glide,
//! and those use the growth stack instead of `Transition`.

#![allow(dead_code)] // later motion passes consume the unused tokens

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rune::{color, Color, Curve, Easing, El, Live, Spring, SpringPair, Transition};

/// `transition-colors` / `transition-all` on interactive controls.
pub const HOVER: Curve = Curve::tween(0.15, Easing::cubic(0.4, 0.0, 0.2, 1.0));
/// The same 150 ms transition for `Live` bindings. Rune's `Live::smooth`
/// takes a spring, so this lands on the CSS duration rather than a curve.
pub const HOVER_SPRING: Spring = Spring::smooth(0.15, 0.0);

/// `caret-blink 0.9s steps(1)`: a near-step opacity cycle. The two stops at
/// 50% make the transition land in one frame while keeping `Cycle`'s
/// wall-clock sampling, unlike `LoopSpec::opacity`'s smooth fade.
pub const CARET: rune::Cycle = rune::Cycle {
    period: 0.9,
    phase: 0.0,
    stops: &[(0.0, 1.0), (0.5, 1.0), (0.5001, 0.0), (1.0, 0.0)],
    still: 1.0,
    fps: 60.0,
};
/// `--stark-dur-1` (140 ms) with `--stark-ease-standard`.
pub const FAST: Curve = Curve::tween(0.14, Easing::cubic(0.2, 0.8, 0.2, 1.0));
/// `--stark-dur-2` (200 ms) `--stark-ease-out`, for short reveals.
pub const ROW_FADE: Curve = Curve::tween(0.20, Easing::cubic(0.16, 1.0, 0.3, 1.0));
/// `--stark-dur-3` (280 ms) `--stark-ease-out`, for panels and surfaces.
pub const PANEL_FADE: Curve = Curve::tween(0.28, Easing::cubic(0.16, 1.0, 0.3, 1.0));

/// `.msg-user-enter` — 200 ms.
pub const MSG_USER: Spring = Spring::smooth(0.20, 0.0);
/// `.msg-agent-enter` — 240 ms.
pub const MSG_AGENT: Spring = Spring::smooth(0.24, 0.0);
/// `.animate-fade-in` — 140 ms.
pub const FADE: Spring = Spring::smooth(0.14, 0.0);
/// `.modal-panel-enter` — 240 ms.
pub const PANEL: Spring = Spring::smooth(0.24, 0.0);
/// `.house-chat-card` / `.canvas-stage.is-map` — 280 ms.
pub const SURFACE: Spring = Spring::smooth(0.28, 0.0);
/// `.modal-backdrop-enter` — 200 ms.
pub const BACKDROP: Spring = Spring::smooth(0.20, 0.0);
/// `.provider-menu-in` / `.gp-menu-in` — 160 ms.
pub const MENU: Spring = Spring::smooth(0.16, 0.0);
/// `.usage-popover` — 180 ms.
pub const POPOVER: Spring = Spring::smooth(0.18, 0.0);
/// `.trace-shell` — 340 ms.
pub const TRACE: Spring = Spring::smooth(0.34, 0.0);
/// `.trace-group` — 280 ms.
pub const TRACE_GROUP: Spring = Spring::smooth(0.28, 0.0);
/// `.trace-collapsible` opacity — 180 ms.
pub const TRACE_BODY: Spring = Spring::smooth(0.18, 0.0);
/// `.trace-collapsible` grid-row height — 280 ms.
pub const TRACE_FOLD: Spring = Spring::smooth(0.28, 0.0);
/// `.activity-tool-panel` — 180 ms.
pub const TOOL_PANEL: Spring = Spring::smooth(0.18, 0.0);
/// `.retry-status` — 200 ms.
pub const RETRY: Spring = Spring::smooth(0.20, 0.0);
/// `.subagent-live-card` / delegation entrance — 300 ms.
pub const DELEGATION: Spring = Spring::smooth(0.30, 0.0);
/// `.attachment-card` — 260 ms.
pub const ATTACH: Spring = Spring::smooth(0.26, 0.0);
/// Chevron and disclosure rotation — 180 ms.
pub const ROTATE: Spring = Spring::smooth(0.18, 0.0);
/// `.chat-row` / `.house-sidebar-row` entrance — 260 ms.
pub const SIDEBAR_IN: Spring = Spring::smooth(0.26, 0.0);

/// `.chat-row` / `.house-sidebar-row` entrance: fade and a 4 pt drop.
pub fn sidebar_row_enter() -> Transition {
    enter(
        SIDEBAR_IN,
        Transition::offset(0.0, -4.0).combined(Transition::opacity()),
    )
}
/// `.is-deleting` / `.house-row-out` — 220 ms.
pub const SIDEBAR_OUT: Spring = Spring::smooth(0.22, 0.0);
/// `.house-drawer-enter` — 280 ms.
pub const DRAWER: Spring = Spring::smooth(0.28, 0.0);

/// An insertion spring with an instant removal. The removal effect is identity,
/// so a menu or modal disappears in the frame it unmounts, like React.
fn enter(spring: Spring, effect: Transition) -> Transition {
    Transition::asymmetric(effect, Transition::none())
        .springs(SpringPair::new(spring, Spring::NONE))
}

/// A transcript row's first appearance. `role` is `"user"`, `"agent"` or
/// `"fade"`; the virtual list only calls this for a key it has never shown.
pub fn row_enter(role: &str) -> Transition {
    match role {
        "user" => enter(
            MSG_USER,
            Transition::offset(14.0, 0.0).combined(Transition::opacity()),
        ),
        "agent" => enter(
            MSG_AGENT,
            Transition::offset(0.0, 8.0).combined(Transition::opacity()),
        ),
        _ => enter(
            FADE,
            Transition::offset(0.0, 2.0).combined(Transition::opacity()),
        ),
    }
}

/// `.modal-panel-enter`: fade, 10 pt rise, and a 0.98 settle.
pub fn modal_enter() -> Transition {
    enter(
        PANEL,
        Transition::offset(0.0, 10.0)
            .combined(Transition::scale(0.98))
            .combined(Transition::opacity()),
    )
}

/// `.modal-backdrop-enter`: opacity only, 200 ms.
pub fn backdrop_enter() -> Transition {
    enter(BACKDROP, Transition::opacity())
}

/// `.provider-menu-in`: fade and a 4 pt rise, 160 ms.
pub fn menu_enter() -> Transition {
    enter(
        MENU,
        Transition::offset(0.0, 4.0).combined(Transition::opacity()),
    )
}

/// `.usage-popover`: fade and a 4 pt rise, 180 ms.
pub fn popover_enter() -> Transition {
    enter(
        POPOVER,
        Transition::offset(0.0, 4.0).combined(Transition::opacity()),
    )
}

/// `.house-chat-card` / `.canvas-stage.is-map`: fade and a 6 pt rise.
pub fn surface_enter() -> Transition {
    enter(
        SURFACE,
        Transition::offset(0.0, 6.0).combined(Transition::opacity()),
    )
}

/// `.house-drawer-enter`: fade and a 10 pt left push.
pub fn drawer_enter() -> Transition {
    enter(
        DRAWER,
        Transition::offset(-10.0, 0.0).combined(Transition::opacity()),
    )
}

/// `.trace-enter`: fade and a 6 pt rise over 340 ms.
pub fn trace_enter() -> Transition {
    enter(
        TRACE,
        Transition::offset(0.0, 6.0).combined(Transition::opacity()),
    )
}

/// `.trace-group`: fade and a 6 pt rise over 280 ms.
pub fn trace_group_enter() -> Transition {
    enter(
        TRACE_GROUP,
        Transition::offset(0.0, 6.0).combined(Transition::opacity()),
    )
}

/// `.activity-tool-panel`: fade and a 2 pt drop over 180 ms.
pub fn tool_panel_enter() -> Transition {
    enter(
        TOOL_PANEL,
        Transition::offset(0.0, -2.0).combined(Transition::opacity()),
    )
}

/// `.retry-status`: fade and a 3 pt rise over 200 ms.
pub fn retry_enter() -> Transition {
    enter(
        RETRY,
        Transition::offset(0.0, 3.0).combined(Transition::opacity()),
    )
}

/// `.subagent-live-card`: fade and a 4 pt rise over 300 ms.
pub fn delegation_enter() -> Transition {
    enter(
        DELEGATION,
        Transition::offset(0.0, 4.0).combined(Transition::opacity()),
    )
}

/// `.attachment-card`: fade, 8 pt rise and a 0.96 settle over 260 ms.
pub fn attachment_enter() -> Transition {
    enter(
        ATTACH,
        Transition::offset(0.0, 8.0)
            .combined(Transition::scale(0.96))
            .combined(Transition::opacity()),
    )
}

/// `.canvas-menu-in`: fade, 3 pt rise and a 0.985 settle over 160 ms.
pub fn canvas_menu_enter() -> Transition {
    enter(
        MENU,
        Transition::offset(0.0, -3.0)
            .combined(Transition::scale(0.985))
            .combined(Transition::opacity()),
    )
}

/// A plain fade with a caller-chosen spring (attachment cards, lightboxes).
pub fn fade_in(spring: Spring) -> Transition {
    enter(spring, Transition::opacity())
}

/// CSS replaces the whole background on `hover:bg-*`; Rune paints `hover` as an
/// overlay *over* the element's own fill. For a transparent rest fill the two
/// are identical, so this returns `target` unchanged. For a filled row it
/// returns the opaque composite of `target` over the surface, which the hover
/// overlay paints as a true replacement.
///
/// `backdrop` is the opaque surface behind the row (the sidebar or card fill).
pub fn hover_replace(base: Color, target: Color, backdrop: Color) -> Color {
    if base.a <= 0.0 || target.a >= 1.0 {
        return target;
    }
    // `Color` is linear; CSS composites in sRGB. Encode before mixing so the
    // replacement lands on the same byte as the browser, then decode back.
    let enc = color::srgb_encode;
    let dec = color::srgb_decode;
    let mix = |top: f32, bottom: f32| enc(top) * target.a + enc(bottom) * (1.0 - target.a);
    Color {
        r: dec(mix(target.r, backdrop.r)).clamp(0.0, 1.0),
        g: dec(mix(target.g, backdrop.g)).clamp(0.0, 1.0),
        b: dec(mix(target.b, backdrop.b)).clamp(0.0, 1.0),
        a: 1.0,
    }
}

/// Two stacked shape colours, the lower one resting and the upper one revealed
/// by the nearest hover group. Rune's `Shape` carries its own stroke colours,
/// so `group_color` cannot tint it; this is the compositing-equivalent pattern
/// already used by the rail and Settings icons.
pub fn hover_icon(size: f32, base: rune::Shape, hovered: rune::Shape) -> El {
    rune::el()
        .size(size, size)
        .inert()
        .child(rune::el().size(size, size).inert().shape(base))
        .child(
            rune::el()
                .size(size, size)
                .absolute(0.0, 0.0)
                .inert()
                .group_opacity(0.0, 1.0)
                .shape(hovered),
        )
}

/// Registry of per-element hover tracks.
///
/// Rune has no built-in hover transform or border track, but `Live` values
/// provide the same thing: a stable handle per key that `live_scale`,
/// `live_offset` and `live_border` bind to, with [`Live::smooth`] supplying
/// the CSS transition curve. The registry is behind an `Arc<Mutex<_>>` so a
/// view (which only sees `&State`) can obtain or reuse a handle without
/// rebuilding, and `on_hover_change` can publish into it with `tx.live_only()`.
#[derive(Clone, Default)]
pub struct HoverSet {
    inner: Arc<Mutex<HoverInner>>,
}

#[derive(Default)]
struct HoverInner {
    scale: HashMap<String, Live<f32>>,
    offset: HashMap<String, Live<[f32; 2]>>,
    border: HashMap<String, Live<Color>>,
    fill: HashMap<String, Live<Color>>,
}

impl HoverSet {
    /// Stable scale track for `key`, starting at `rest` the first time it is
    /// requested. Later calls with a different `rest` keep the original track
    /// so an element rebuilt mid-hover does not reset.
    pub fn scale(&self, key: impl Into<String>, rest: f32) -> Live<f32> {
        let mut inner = self.inner.lock().expect("hover set poisoned");
        inner
            .scale
            .entry(key.into())
            .or_insert_with(|| Live::new(rest))
            .clone()
    }

    /// Stable offset track for `key` (parent-space points).
    pub fn offset(&self, key: impl Into<String>, rest: [f32; 2]) -> Live<[f32; 2]> {
        let mut inner = self.inner.lock().expect("hover set poisoned");
        inner
            .offset
            .entry(key.into())
            .or_insert_with(|| Live::new(rest))
            .clone()
    }

    /// Stable border-colour track for `key`.
    pub fn border(&self, key: impl Into<String>, rest: Color) -> Live<Color> {
        let mut inner = self.inner.lock().expect("hover set poisoned");
        inner
            .border
            .entry(key.into())
            .or_insert_with(|| Live::new(rest))
            .clone()
    }

    /// Stable fill-colour track for `key`. CSS replaces an opaque surface
    /// colour on hover; `live_fill` does the same without an overlay.
    pub fn fill(&self, key: impl Into<String>, rest: Color) -> Live<Color> {
        let mut inner = self.inner.lock().expect("hover set poisoned");
        inner
            .fill
            .entry(key.into())
            .or_insert_with(|| Live::new(rest))
            .clone()
    }

    /// Drop tracks whose element is gone. Call after a history/card refresh so
    /// a long-lived thread does not accumulate handles for deleted chats.
    pub fn retain(&self, keep: impl Fn(&str) -> bool) {
        let mut inner = self.inner.lock().expect("hover set poisoned");
        inner.scale.retain(|key, _| keep(key));
        inner.offset.retain(|key, _| keep(key));
        inner.border.retain(|key, _| keep(key));
        inner.fill.retain(|key, _| keep(key));
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        let inner = self.inner.lock().expect("hover set poisoned");
        inner.scale.len() + inner.offset.len() + inner.border.len() + inner.fill.len()
    }
}

/// Parse a token colour. Test-only convenience; runtime code passes `Color`s.
pub fn ink(hex: &str) -> Color {
    color::hex(hex).unwrap_or(Color::BLACK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_and_agent_entrances_match_the_css_keyframes() {
        let user = row_enter("user");
        assert_eq!(user.insertion.offset.x, 14.0);
        assert_eq!(user.insertion.offset.y, 0.0);
        assert_eq!(user.insertion.opacity, 0.0);
        assert_eq!(user.springs.enter, MSG_USER);
        assert_eq!(user.springs.exit, Spring::NONE);
        assert_eq!(user.removal, rune::Effect::IDENTITY);

        let agent = row_enter("agent");
        assert_eq!(agent.insertion.offset.x, 0.0);
        assert_eq!(agent.insertion.offset.y, 8.0);
        assert_eq!(agent.springs.enter, MSG_AGENT);
        assert_eq!(agent.springs.exit, Spring::NONE);

        let fade = row_enter("system");
        assert_eq!(fade.insertion.offset.y, 2.0);
        assert_eq!(fade.springs.enter, FADE);
    }

    #[test]
    fn modal_and_menu_entrances_remove_in_the_frame_they_close() {
        for transition in [
            modal_enter(),
            menu_enter(),
            popover_enter(),
            backdrop_enter(),
        ] {
            assert_eq!(transition.springs.exit, Spring::NONE);
            assert_eq!(transition.removal, rune::Effect::IDENTITY);
            assert_eq!(transition.insertion.opacity, 0.0);
        }
        assert_eq!(modal_enter().insertion.offset.y, 10.0);
        assert_eq!(modal_enter().insertion.scale, 0.98);
        assert_eq!(menu_enter().insertion.offset.y, 4.0);
    }

    #[test]
    fn trace_menu_and_surface_entrances_match_the_css_keyframes() {
        let trace = trace_enter();
        assert_eq!(trace.insertion.offset.y, 6.0);
        assert_eq!(trace.springs.enter, TRACE);
        assert_eq!(trace.springs.exit, Spring::NONE);

        let group = trace_group_enter();
        assert_eq!(group.insertion.offset.y, 6.0);
        assert_eq!(group.springs.enter, TRACE_GROUP);

        let panel = tool_panel_enter();
        assert_eq!(panel.insertion.offset.y, -2.0);
        assert_eq!(panel.springs.enter, TOOL_PANEL);

        let delegation = delegation_enter();
        assert_eq!(delegation.insertion.offset.y, 4.0);
        assert_eq!(delegation.springs.enter, DELEGATION);

        let retry = retry_enter();
        assert_eq!(retry.insertion.offset.y, 3.0);
        assert_eq!(retry.springs.enter, RETRY);

        let attach = attachment_enter();
        assert_eq!(attach.insertion.offset.y, 8.0);
        assert_eq!(attach.insertion.scale, 0.96);
        assert_eq!(attach.springs.enter, ATTACH);

        let menu = canvas_menu_enter();
        assert_eq!(menu.insertion.offset.y, -3.0);
        assert_eq!(menu.insertion.scale, 0.985);
        assert_eq!(menu.springs.enter, MENU);

        let sidebar = sidebar_row_enter();
        assert_eq!(sidebar.insertion.offset.y, -4.0);
        assert_eq!(sidebar.springs.enter, SIDEBAR_IN);

        for transition in [
            trace, group, panel, delegation, retry, attach, menu, sidebar,
        ] {
            assert_eq!(transition.springs.exit, Spring::NONE);
            assert_eq!(transition.removal, rune::Effect::IDENTITY);
        }
    }

    #[test]
    fn hover_replacement_composites_the_target_over_the_surface() {
        let surface = ink("#0f0f0f");
        let base = Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.025,
        };
        let target = Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.05,
        };
        let got = hover_replace(base, target, surface);
        // 5% white over #0f0f0f is #1b1b1b in sRGB.
        let want = ink("#1b1b1b");
        assert!((got.r - want.r).abs() < 0.005, "{got:?}");
        assert!((got.g - want.g).abs() < 0.005);
        assert!((got.b - want.b).abs() < 0.005);
        assert_eq!(got.a, 1.0);
    }

    #[test]
    fn transparent_rest_fills_keep_the_css_overlay() {
        let target = Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.07,
        };
        let transparent = Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.0,
        };
        assert_eq!(
            hover_replace(transparent, target, ink("#0f0f0f")),
            target,
            "a transparent row lets the real surface show through"
        );
    }

    #[test]
    fn caret_and_hover_tracks_match_their_css_clocks() {
        assert_eq!(CARET.period, 0.9);
        assert_eq!(CARET.stops[1], (0.5, 1.0));
        assert_eq!(CARET.stops[2], (0.5001, 0.0));
        assert_eq!(HOVER_SPRING, Spring::smooth(0.15, 0.0));
        assert_eq!(TRACE_BODY, Spring::smooth(0.18, 0.0));
        assert_eq!(TRACE_FOLD, Spring::smooth(0.28, 0.0));
    }

    #[test]
    fn hover_set_reuses_one_track_per_key_and_prunes() {
        let set = HoverSet::default();
        let a = set.scale("card-a", 1.0);
        let again = set.scale("card-a", 2.0);
        assert_eq!(again.get(), 1.0, "the first rest value wins");
        a.set(1.08);
        assert_eq!(again.get(), 1.08, "handles share one Live source");

        let off = set.offset("card-a", [0.0, 0.0]);
        off.set([0.0, -1.0]);
        assert_eq!(set.offset("card-a", [9.0, 9.0]).get(), [0.0, -1.0]);

        let border = set.border("pill", ink("#ffffff12"));
        border.set(ink("#ffffff29"));
        assert_eq!(set.border("pill", Color::BLACK).get(), ink("#ffffff29"));

        let fill = set.fill("circle", ink("#ffffff"));
        fill.set(ink("#e8e8ea"));
        assert_eq!(set.fill("circle", Color::BLACK).get(), ink("#e8e8ea"));

        assert_eq!(set.len(), 4);
        set.retain(|key| key == "card-a");
        assert_eq!(set.len(), 2, "the pill/circle tracks are pruned");
        let fresh = set.border("pill", ink("#ffffff12"));
        assert_eq!(fresh.get(), ink("#ffffff12"), "a pruned key starts over");
    }
}
