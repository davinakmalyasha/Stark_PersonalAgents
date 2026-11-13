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
#[path = "shell.rs"]
pub mod shell;
#[cfg(feature = "rune-real")]
#[path = "stream.rs"]
pub mod stream;
#[cfg(feature = "rune-real")]
#[path = "token_stats.rs"]
pub mod token_stats;
#[cfg(feature = "rune-real")]
#[path = "transport.rs"]
pub mod transport;
#[cfg(feature = "rune-real")]
#[path = "voice.rs"]
pub mod voice;

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
