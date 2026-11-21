//! Library surface of the native app, shared with the web shell crate
//! (`stark-rune-web`, the `cdylib`). The binary (`src/main.rs`) keeps the
//! native CLI; lib and bin compile the same modules below.

#[cfg(feature = "rune-real")]
#[path = "agent_config.rs"]
pub mod agent_config;
#[cfg(feature = "rune-real")]
#[path = "agents.rs"]
pub mod agents;
#[cfg(feature = "rune-real")]
#[path = "ask.rs"]
pub mod ask;
#[cfg(feature = "rune-real")]
#[path = "chats.rs"]
pub mod chats;
#[cfg(feature = "rune-real")]
#[path = "clock.rs"]
pub mod clock;
#[cfg(feature = "rune-real")]
#[path = "cloud.rs"]
pub mod cloud;
#[cfg(feature = "rune-real")]
#[path = "composer.rs"]
pub mod composer;
#[cfg(feature = "rune-real")]
#[path = "group/mod.rs"]
pub mod group;
#[cfg(feature = "rune-real")]
#[path = "icons.rs"]
pub mod icons;
#[cfg(feature = "rune-real")]
#[path = "image_cache.rs"]
pub mod image_cache;
#[cfg(feature = "rune-real")]
#[path = "keys.rs"]
pub mod keys;
#[cfg(feature = "rune-real")]
#[path = "motion.rs"]
pub mod motion;
#[cfg(feature = "rune-real")]
#[path = "notes.rs"]
pub mod notes;
#[cfg(feature = "rune-real")]
#[path = "projects.rs"]
pub mod projects;
#[cfg(feature = "rune-real")]
#[path = "rooms.rs"]
pub mod rooms;
#[cfg(feature = "rune-real")]
#[path = "rune_canvas.rs"]
pub mod rune_canvas;
#[cfg(feature = "rune-real")]
#[path = "rune_transcript.rs"]
pub mod rune_transcript;
#[cfg(feature = "rune-real")]
#[path = "settings.rs"]
pub mod settings;
#[cfg(feature = "rune-real")]
#[path = "shell.rs"]
pub mod shell;
#[cfg(feature = "rune-real")]
#[path = "stream.rs"]
pub mod stream;
#[cfg(feature = "rune-real")]
#[path = "stream_accum.rs"]
pub mod stream_accum;
#[cfg(feature = "rune-real")]
#[path = "stream_presentation.rs"]
pub mod stream_presentation;
#[cfg(feature = "rune-real")]
#[path = "stt.rs"]
pub mod stt;
#[cfg(feature = "rune-real")]
#[path = "token_stats.rs"]
pub mod token_stats;
#[cfg(feature = "rune-real")]
#[path = "tool_labels.rs"]
pub mod tool_labels;
#[cfg(feature = "rune-real")]
#[path = "transport.rs"]
pub mod transport;
#[cfg(feature = "rune-real")]
#[path = "voice.rs"]
pub mod voice;
#[cfg(feature = "rune-real")]
#[path = "voice_ws.rs"]
pub mod voice_ws;

/// Lucide draws its 2-unit stroke at a 24-unit viewBox and scales it with the
/// icon. Rune's `view_box` leaves stroke widths in points, so callers pass the
/// icon's point size here to keep the drawn weight identical to React.
#[cfg(feature = "rune-real")]
pub(crate) fn lucide_stroke(size: f32) -> f32 {
    size * (2.0 / 24.0)
}

/// Lucide `ArrowUp` (`m5 12 7-7 7 7` + `M12 19V5`) at `size` points.
/// `stroke_units` is the 24-unit stroke (2.0 normally, 2.4 in the send button).
#[cfg(feature = "rune-real")]
pub(crate) fn lucide_arrow_up(size: f32, stroke_units: f32, color: rune::Color) -> rune::Shape {
    let p = rune::Path::new()
        .move_to([5.0, 12.0])
        .line_to([12.0, 5.0])
        .line_to([19.0, 12.0])
        .move_to([12.0, 19.0])
        .line_to([12.0, 5.0]);
    rune::Shape::new()
        .stroke_path(
            &p,
            size * (stroke_units / 24.0),
            rune::LineCap::Round,
            color,
        )
        .view_box(24.0, 24.0)
}

/// Lucide `X` (`M18 6 6 18` + `m6 6 12 12`) at `size` points. Round caps
/// and joins are the Lucide defaults; callers only pick the drawn weight.
#[cfg(feature = "rune-real")]
pub(crate) fn lucide_x(size: f32, stroke_units: f32, color: rune::Color) -> rune::Shape {
    let p = rune::Path::new()
        .move_to([18.0, 6.0])
        .line_to([6.0, 18.0])
        .move_to([6.0, 6.0])
        .line_to([18.0, 18.0]);
    rune::Shape::new()
        .stroke_path(
            &p,
            size * (stroke_units / 24.0),
            rune::LineCap::Round,
            color,
        )
        .view_box(24.0, 24.0)
}

/// Lucide `Check` (`M20 6 9 17l-5-5`) at `size` points.
#[cfg(feature = "rune-real")]
pub(crate) fn lucide_check(size: f32, stroke_units: f32, color: rune::Color) -> rune::Shape {
    let p = rune::Path::new()
        .move_to([20.0, 6.0])
        .line_to([9.0, 17.0])
        .line_to([4.0, 12.0]);
    rune::Shape::new()
        .stroke_path(
            &p,
            size * (stroke_units / 24.0),
            rune::LineCap::Round,
            color,
        )
        .view_box(24.0, 24.0)
}

/// Lucide `Square` drawn as a filled shape (`fill="currentColor"`):
/// an 18-unit rounded rect inset 3 units in the 24-unit box.
#[cfg(feature = "rune-real")]
pub(crate) fn lucide_square_filled(size: f32, color: rune::Color) -> rune::El {
    rune::el()
        .size(size * (18.0 / 24.0), size * (18.0 / 24.0))
        .radius(size * (2.0 / 24.0))
        .fill(color)
}

/// Lucide `FileText` (document outline + three rule lines).
#[cfg(feature = "rune-real")]
pub(crate) fn lucide_file_text(size: f32, color: rune::Color) -> rune::Shape {
    let p = rune::Path::new()
        .move_to([15.0, 2.0])
        .line_to([6.0, 2.0])
        .cubic_to([4.9, 2.0], [4.0, 2.9], [4.0, 4.0])
        .line_to([4.0, 20.0])
        .cubic_to([4.0, 21.1], [4.9, 22.0], [6.0, 22.0])
        .line_to([18.0, 22.0])
        .cubic_to([19.1, 22.0], [20.0, 21.1], [20.0, 20.0])
        .line_to([20.0, 7.0])
        .close()
        .move_to([14.0, 2.0])
        .line_to([14.0, 6.0])
        .cubic_to([14.0, 7.1], [14.9, 8.0], [16.0, 8.0])
        .line_to([20.0, 8.0])
        .move_to([16.0, 13.0])
        .line_to([8.0, 13.0])
        .move_to([16.0, 17.0])
        .line_to([8.0, 17.0]);
    rune::Shape::new()
        .stroke_path(&p, crate::lucide_stroke(size), rune::LineCap::Round, color)
        .view_box(24.0, 24.0)
}

/// Lucide `Image` (rounded frame, sun, mountain).
#[cfg(feature = "rune-real")]
pub(crate) fn lucide_image(size: f32, color: rune::Color) -> rune::Shape {
    let p = rune::Path::new()
        .move_to([5.0, 3.0])
        .line_to([19.0, 3.0])
        .cubic_to([20.1, 3.0], [21.0, 3.9], [21.0, 5.0])
        .line_to([21.0, 19.0])
        .cubic_to([21.0, 20.1], [20.1, 21.0], [19.0, 21.0])
        .line_to([5.0, 21.0])
        .cubic_to([3.9, 21.0], [3.0, 20.1], [3.0, 19.0])
        .line_to([3.0, 5.0])
        .cubic_to([3.0, 3.9], [3.9, 3.0], [5.0, 3.0])
        .close()
        .move_to([11.0, 9.0])
        .cubic_to([11.0, 10.1], [10.1, 11.0], [9.0, 11.0])
        .cubic_to([7.9, 11.0], [7.0, 10.1], [7.0, 9.0])
        .cubic_to([7.0, 7.9], [7.9, 7.0], [9.0, 7.0])
        .cubic_to([10.1, 7.0], [11.0, 7.9], [11.0, 9.0])
        .close()
        .move_to([21.0, 15.0])
        .line_to([17.9, 11.9])
        .cubic_to([17.1, 11.1], [15.8, 11.1], [15.0, 11.9])
        .line_to([6.0, 21.0]);
    rune::Shape::new()
        .stroke_path(&p, crate::lucide_stroke(size), rune::LineCap::Round, color)
        .view_box(24.0, 24.0)
}

/// Lucide `AudioLines` (the dock's voice glyph, reused by file chips).
#[cfg(feature = "rune-real")]
pub(crate) fn lucide_audio_lines(size: f32, color: rune::Color) -> rune::Shape {
    let p = rune::Path::new()
        .move_to([2.0, 10.0])
        .line_to([2.0, 13.0])
        .move_to([6.0, 6.0])
        .line_to([6.0, 17.0])
        .move_to([10.0, 3.0])
        .line_to([10.0, 21.0])
        .move_to([14.0, 8.0])
        .line_to([14.0, 15.0])
        .move_to([18.0, 5.0])
        .line_to([18.0, 18.0])
        .move_to([22.0, 10.0])
        .line_to([22.0, 13.0]);
    rune::Shape::new()
        .stroke_path(&p, crate::lucide_stroke(size), rune::LineCap::Round, color)
        .view_box(24.0, 24.0)
}

/// Lucide `Download` (tray + arrow).
#[cfg(feature = "rune-real")]
pub(crate) fn lucide_download(size: f32, color: rune::Color) -> rune::Shape {
    let p = rune::Path::new()
        .move_to([21.0, 15.0])
        .line_to([21.0, 19.0])
        .cubic_to([21.0, 20.1], [20.1, 21.0], [19.0, 21.0])
        .line_to([5.0, 21.0])
        .cubic_to([3.9, 21.0], [3.0, 20.1], [3.0, 19.0])
        .line_to([3.0, 15.0])
        .move_to([7.0, 10.0])
        .line_to([12.0, 15.0])
        .line_to([17.0, 10.0])
        .move_to([12.0, 15.0])
        .line_to([12.0, 3.0]);
    rune::Shape::new()
        .stroke_path(&p, crate::lucide_stroke(size), rune::LineCap::Round, color)
        .view_box(24.0, 24.0)
}
