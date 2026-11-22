//! Stark byte-identical design tokens, extracted from the former React frontend.
//!
//! Sources (`:root`, premium surfaces, send button, house and canvas layout):
//! - index tokens and `.premium-surface` / `.send-button`
//! - `.house-root`, hairlines, dock and cards
//! - canvas stage, grid, cards `468x420` / compact `384x392`
//!
//! Rune mapping: these become `rune::Theme::from_json` + `rune-kit Kit`
//! (`Palette/Elevation/Type/Radius/Motion/Glyphs`). No raw literals in app code.

/// Core surfaces (monochrome, near-black).
pub mod color {
    /// `.house-root` background.
    pub const HOUSE_ROOT: &str = "#0a0a0a";
    /// App shell background (`App.tsx` root + `index.css` body).
    pub const APP_BG: &str = "#0c0c0c";
    /// Canvas map background (`.canvas-stage.is-map` fallback).
    pub const CANVAS_MAP_BG: &str = "#08080a";
    /// Canvas default background (`.canvas-stage`).
    pub const CANVAS_BG: &str = "#0a0a0c";
    /// Sidebar panel (`HouseSidebar` / `w-[248px] bg-[#0f0f0f]`).
    pub const SIDEBAR_BG: &str = "#0f0f0f";
    /// Primary text.
    pub const TEXT_PRIMARY: &str = "#fafafa";
    /// Secondary text (`--text-secondary`).
    pub const TEXT_SECONDARY: &str = "#8a8a8a";
    /// Hairline border (`rgba(255,255,255,0.08)`).
    pub const HAIRLINE: &str = "rgba(255,255,255,0.08)";
    /// Active-card border (`rgba(255,255,255,0.30)`).
    pub const CARD_ACTIVE_BORDER: &str = "rgba(255,255,255,0.30)";
    /// White send button.
    pub const SEND_BG: &str = "#ffffff";
    /// Send foreground.
    pub const SEND_FG: &str = "#0a0a0d";
    /// PWA theme color / manifest background.
    pub const PWA_THEME: &str = "#0c0c0e";
}

/// Geometry — must stay in sync with `canvas.css` + `HouseChat.tsx`.
pub mod layout {
    /// Full chat card width.
    pub const CARD_W: f32 = 468.0;
    /// Full chat card height (`.is-chat` fixed).
    pub const CARD_H: f32 = 420.0;
    /// Compact (child depth>0) card width.
    pub const CARD_COMPACT_W: f32 = 384.0;
    /// Compact card height.
    pub const CARD_COMPACT_H: f32 = 392.0;
    /// Card radius / compact radius.
    pub const CARD_RADIUS: f32 = 18.0;
    pub const CARD_RADIUS_COMPACT: f32 = 16.0;
    /// Home chat card max width (`md:max-w-[740px]`).
    pub const HOME_CHAT_MAX_W: f32 = 740.0;
    /// Sidebar width (`w-[248px]`).
    pub const SIDEBAR_W: f32 = 248.0;
    /// Subagent pane width.
    pub const SUBAGENT_PANE_W: f32 = 420.0;
    /// Canvas zoom limits (`CanvasStage.tsx` clamp).
    pub const ZOOM_MIN: f32 = 0.45;
    pub const ZOOM_MAX: f32 = 1.8;
    /// Fit-view zoom clamp (`fitView`).
    pub const FIT_ZOOM_MIN: f32 = 0.45;
    pub const FIT_ZOOM_MAX: f32 = 1.15;
    /// Grid dot spacing (background / map).
    pub const GRID_BG: f32 = 24.0;
    pub const GRID_MAP: f32 = 26.0;
    /// Default view (`DEFAULT_VIEW` in `canvasTypes.ts`).
    pub const DEFAULT_VIEW_X: f32 = 48.0;
    pub const DEFAULT_VIEW_Y: f32 = 48.0;
    pub const DEFAULT_VIEW_SCALE: f32 = 1.0;
}

/// Typography.
pub mod type_ {
    pub const FONT_UI: &str = "Inter";
    pub const FONT_DISPLAY: &str = "Plus Jakarta Sans";
    pub const FONT_MONO: &str = "ui-monospace,SFMono-Regular,Consolas,monospace";
    /// Card title 15px / compact 13.5px, subtitle 11.5px / 10.5px.
    pub const CARD_TITLE: f32 = 15.0;
    pub const CARD_TITLE_COMPACT: f32 = 13.5;
    /// Composer input 15px (`prompt-input`), card input 10.5px.
    pub const COMPOSER_TEXT: f32 = 15.0;
    /// Chat header title 13px.
    pub const CHAT_TITLE: f32 = 13.0;
}

/// Motion — one easing language (`cubic-bezier(.16,1,.3,1)`).
pub mod motion {
    /// Standard entrance (`canvas-node-in .42s`, `house-rise .42s`).
    pub const EASE_OUT: &str = "cubic-bezier(0.16,1,0.3,1)";
    /// Running border sweep period.
    pub const BORDER_SWEEP_SECS: f32 = 2.6;
    /// Typing dot period.
    pub const TYPING_SECS: f32 = 1.1;
}

/// Serialize the token set as the `theme.json` payload that
/// `rune::Theme::from_json` consumes. Keys mirror the CSS source names so a
/// reviewer can diff token-for-token against `index.css` / `house.css`.
pub fn theme_json() -> String {
    format!(
        "{{\
\"house_root\":\"{house}\",\
\"app_bg\":\"{app}\",\
\"canvas_map_bg\":\"{map}\",\
\"canvas_bg\":\"{canvas}\",\
\"sidebar_bg\":\"{side}\",\
\"text_primary\":\"{tp}\",\
\"text_secondary\":\"{ts}\",\
\"hairline\":\"{hair}\",\
\"send_bg\":\"{send}\",\
\"send_fg\":\"{sendfg}\",\
\"card_w\":{cw},\"card_h\":{ch},\
\"card_compact_w\":{ccw},\"card_compact_h\":{cch},\
\"card_radius\":{cr},\"card_radius_compact\":{crc},\
\"home_chat_max_w\":{hc},\"sidebar_w\":{sw},\
\"font_ui\":\"{fui}\",\"font_mono\":\"{fmono}\",\
\"ease_out\":\"{ease}\"\
}}",
        house = color::HOUSE_ROOT,
        app = color::APP_BG,
        map = color::CANVAS_MAP_BG,
        canvas = color::CANVAS_BG,
        side = color::SIDEBAR_BG,
        tp = color::TEXT_PRIMARY,
        ts = color::TEXT_SECONDARY,
        hair = color::HAIRLINE,
        send = color::SEND_BG,
        sendfg = color::SEND_FG,
        cw = layout::CARD_W,
        ch = layout::CARD_H,
        ccw = layout::CARD_COMPACT_W,
        cch = layout::CARD_COMPACT_H,
        cr = layout::CARD_RADIUS,
        crc = layout::CARD_RADIUS_COMPACT,
        hc = layout::HOME_CHAT_MAX_W,
        sw = layout::SIDEBAR_W,
        fui = type_::FONT_UI,
        fmono = type_::FONT_MONO,
        ease = motion::EASE_OUT,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_identical_constants() {
        // Guard against drift from the frozen CSS.
        assert_eq!(color::HOUSE_ROOT, "#0a0a0a");
        assert_eq!(color::APP_BG, "#0c0c0c");
        assert_eq!(color::CANVAS_MAP_BG, "#08080a");
        assert_eq!(color::SIDEBAR_BG, "#0f0f0f");
        assert_eq!(layout::CARD_W, 468.0);
        assert_eq!(layout::CARD_H, 420.0);
        assert_eq!(layout::CARD_COMPACT_W, 384.0);
        assert_eq!(layout::CARD_COMPACT_H, 392.0);
        assert_eq!(layout::CARD_RADIUS, 18.0);
        assert_eq!(layout::ZOOM_MIN, 0.45);
        assert_eq!(layout::ZOOM_MAX, 1.8);
        assert_eq!(layout::DEFAULT_VIEW_X, 48.0);
    }

    #[test]
    fn theme_json_round_trips() {
        let json = theme_json();
        assert!(json.contains("\"house_root\":\"#0a0a0a\""));
        assert!(json.contains("\"card_w\":468"));
        assert!(json.contains("\"sidebar_w\":248"));
        assert!(json.contains("cubic-bezier(0.16,1,0.3,1)"));
    }
}
