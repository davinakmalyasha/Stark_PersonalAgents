//! Shared composer card — the `.premium-surface` prompt dock.
//!
//! Used docked in the transcript view and floating over the canvas map.
//! Voice button and live `text_input` binding arrive next; the input is static
//! text for now.

use crate::keys::EnterPress;
use rune::{col, row, spacer, text, text_input, El, TextEdit, Tx};

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

/// Boxed press handler (keeps the card signature small).
pub type Press<S> = std::rc::Rc<dyn Fn(&mut S)>;
/// Boxed press handler that needs the transaction (file dialogs).
pub type PressTx<S> = std::rc::Rc<dyn Fn(&mut S, &mut Tx)>;
/// Boxed pick handler (model rows carry their id).
pub type Pick<S> = std::rc::Rc<dyn Fn(&mut S, String)>;
/// Boxed file-drop handler (OS paths).
pub type DropFiles<S> = std::rc::Rc<dyn Fn(&mut S, &[std::path::PathBuf])>;
/// Boxed drag-hover handler (`is-dragging` highlight).
pub type DropHover<S> = std::rc::Rc<dyn Fn(&mut S, bool)>;

/// Level meter: five bars lit by bucket, plus the capture note (no-device
/// reason) when present. Mounted beside the voice toggle while live.
fn attach_button<S: 'static>(attach: PressTx<S>) -> El {
    // `.tool-button` (Plus 18): the reference UI's file picker.
    let plus = |color: rune::Color| {
        rune::Shape::new()
            .line(
                [12.0, 5.0],
                [12.0, 19.0],
                crate::lucide_stroke(18.0),
                rune::LineCap::Round,
                color,
            )
            .line(
                [5.0, 12.0],
                [19.0, 12.0],
                crate::lucide_stroke(18.0),
                rune::LineCap::Round,
                color,
            )
            .view_box(24.0, 24.0)
    };
    rune::el()
        .key("composer-attach")
        .role("button")
        .focusable(true)
        .label("Attach files")
        .size(30.0, 30.0)
        .radius(10.0)
        .center()
        .hover(ink("#ffffff12"))
        .hover_group()
        .on_press_tx_enter(move |s: &mut S, tx: &mut Tx| attach(s, tx))
        .child(crate::motion::hover_icon(
            18.0,
            plus(ink("#b4b4bc")),
            plus(ink("#ededf1")),
        ))
}

/// Deck thumbnail (`LocalFileThumb`/`RestoredFileThumb`): 30×30 picture chip
/// or the matching audio/file icon.
fn attachment_thumb(
    file: &super::chats::ChatAttachment,
    id: &str,
    workspace_id: &str,
    chat_id: &str,
) -> El {
    let icon = if file.is_image {
        crate::lucide_image(16.0, ink("#c9c9d1"))
    } else if file.is_audio() {
        crate::lucide_audio_lines(16.0, ink("#d4d4da"))
    } else {
        crate::lucide_file_text(16.0, ink("#c4c4cb"))
    };
    let mut chip = rune::el()
        .key(format!("composer-file-thumb-{id}"))
        .size(30.0, 30.0)
        .radius(9.0)
        .clip_rounded(9.0)
        .fill(if file.is_image {
            ink("#ffffff12")
        } else {
            ink("#ffffff14")
        })
        .center();
    if file.is_image {
        if let Some(image) = crate::image_cache::attachment(file, 30.0, workspace_id, chat_id) {
            return chip.child(
                rune::el()
                    .size(30.0, 30.0)
                    .pixels_fit(image.pixels(), rune::SurfaceFit::Fill)
                    .inert(),
            );
        }
    }
    chip = chip.child(rune::el().size(16.0, 16.0).inert().shape(icon));
    chip
}

/// `attachment-card`: picture/icon chip, name + kind · size, remove button.
fn attachment_card<S: 'static>(
    file: &super::chats::ChatAttachment,
    index: usize,
    workspace_id: &str,
    chat_id: &str,
    remove: Pick<S>,
) -> El {
    let id = if file.id.is_empty() {
        format!("local-{index}")
    } else {
        file.id.clone()
    };
    let name_style = rune::tok::TextStyle::new(12.0, 500.0);
    let meta_style = rune::tok::TextStyle::new(10.0, 400.0);
    let label = format!("{} · {}", file.kind_label(), file.size_label());
    let target = if file.id.is_empty() {
        format!("local:{index}")
    } else {
        file.id.clone()
    };
    row()
        .key(format!("composer-file-{id}"))
        .transition(crate::motion::attachment_enter())
        .min_w(178.0)
        .max_w(236.0)
        .items_center()
        .gap(8.0)
        .px(8.0)
        .py(8.0)
        .radius(12.0)
        .border(1.0, ink("#ffffff17"))
        .border_layout()
        .fill(ink("#0a0a0c70"))
        .child(attachment_thumb(file, &id, workspace_id, chat_id))
        .child(
            col()
                .key(format!("composer-file-copy-{id}"))
                .grow(1.0)
                .min_w(0.0)
                .child(
                    text(file.name.clone())
                        .min_w(0.0)
                        .truncate()
                        .font(name_style)
                        .color(ink("#e4e4e7")),
                )
                .child(text(label).font(meta_style).color(ink("#71717a"))),
        )
        .child(
            rune::el()
                .key(format!("composer-file-remove-{id}"))
                .role("button")
                .focusable(true)
                .label(format!("Remove {}", file.name))
                .size(21.0, 21.0)
                .radius(8.0)
                .center()
                .hover(ink("#ffffff17"))
                .hover_group()
                .on_press_enter(move |s: &mut S| remove(s, target.clone()))
                .child(crate::motion::hover_icon(
                    13.0,
                    crate::lucide_x(13.0, 2.0, ink("#777782")),
                    crate::lucide_x(13.0, 2.0, ink("#eeeeee")),
                )),
        )
}

/// `.attachment-add`: dashed tile matching a card's height.
fn attachment_add<S: 'static>(attach: PressTx<S>, hover: &crate::motion::HoverSet) -> El {
    let border = hover.border("composer-attachment-add-border", ink("#ffffff2b"));
    let set_border = border.clone();
    rune::el()
        .key("composer-attachment-add")
        .role("button")
        .focusable(true)
        .label("Add files")
        .min_w(38.0)
        .min_h(48.0)
        .radius(8.0)
        .border(1.0, ink("#ffffff2b"))
        .border_dash(4.0, 3.0)
        .border_layout()
        .center()
        .hover(ink("#ffffff10"))
        .live_border(border.smooth(crate::motion::HOVER_SPRING))
        .hover_group()
        .on_hover_change(move |_s: &mut S, inside, tx| {
            set_border.set(if inside {
                ink("#ffffff52")
            } else {
                ink("#ffffff2b")
            });
            tx.live_only();
        })
        .on_press_tx_enter(move |s: &mut S, tx: &mut Tx| attach(s, tx))
        .child({
            let plus = |color: rune::Color| {
                rune::Shape::new()
                    .line(
                        [12.0, 5.0],
                        [12.0, 19.0],
                        crate::lucide_stroke(17.0),
                        rune::LineCap::Round,
                        color,
                    )
                    .line(
                        [5.0, 12.0],
                        [19.0, 12.0],
                        crate::lucide_stroke(17.0),
                        rune::LineCap::Round,
                        color,
                    )
                    .view_box(24.0, 24.0)
            };
            crate::motion::hover_icon(17.0, plus(ink("#777782")), plus(ink("#eeeeee")))
        })
}

/// Lucide `AudioLines` (16pt) inside the voice toggle.
fn voice_icon(color: rune::Color) -> rune::Shape {
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
        .stroke_path(&p, crate::lucide_stroke(16.0), rune::LineCap::Round, color)
        .view_box(24.0, 24.0)
}

/// Orbit arc for the armed live-mode button: a ~100° white arc on the 38pt
/// orbit circle, spun by a rotation loop. Same tessellation trick as the
/// usage orb's progress arc.
fn orbit_arc(cx: f32, cy: f32, r: f32) -> Vec<[f32; 2]> {
    let steps = 20usize;
    let sweep = 100.0_f32.to_radians();
    let start = -std::f32::consts::FRAC_PI_2;
    (0..=steps)
        .map(|i| {
            let a = start + sweep * i as f32 / steps as f32;
            [cx + r * a.cos(), cy + r * a.sin()]
        })
        .collect()
}

/// Live-mode toggle. The dock only arms: white disc + black icon + a moving
/// orbit ring while armed, calm grey while off. Capture itself starts only
/// from the island's mic (or Ctrl+Space while armed). `S` is the owning
/// view's state type.
fn voice_button<S: 'static>(
    voice: &crate::voice::VoiceMeter,
    on_voice: impl Fn(&mut S) + 'static,
) -> El {
    let active = voice.armed || voice.live;
    let mut button = rune::el()
        .key("composer-voice")
        .role("button")
        .focusable(true)
        .label(if active {
            "Live mode on — press to turn off"
        } else {
            "Turn live mode on"
        })
        .size(32.0, 32.0)
        .radius(11.0)
        .center()
        .on_press_enter(on_voice)
        .press_scale(0.96);
    if active {
        // Armed: white disc, black AudioLines. The orbit carries the "on"
        // signal so the mark itself stays calm and readable. The ring layers
        // ride inside the button (like the usage orb) so the toolbar path
        // never changes and the press target stays put; they overflow the
        // 32pt disc by 3pt a side and draw over the toolbar padding.
        button = button
            .fill(ink("#ffffff"))
            .child(
                rune::el()
                    .key("composer-voice-track")
                    .absolute(-3.0, -3.0)
                    .size(38.0, 38.0)
                    .inert()
                    .shape(
                        rune::Shape::new()
                            .fill(ring_fill(19.0, 19.0, 17.0, 18.4), ink("#ffffff24")),
                    ),
            )
            .child(
                rune::el()
                    .key("composer-voice-arc")
                    .absolute(-3.0, -3.0)
                    .size(38.0, 38.0)
                    .inert()
                    .shape(rune::Shape::new().stroke_with(
                        orbit_arc(19.0, 19.0, 17.7),
                        false,
                        2.0,
                        rune::LineCap::Round,
                        ink("#ffffff"),
                    ))
                    .looping(rune::LoopSpec::rotation(1.2)),
            )
            .child(
                rune::el()
                    .size(16.0, 16.0)
                    .inert()
                    .shape(voice_icon(ink("#111113"))),
            );
    } else {
        button = button
            .fill(ink("#303035"))
            // `.prompt-voice-button:hover` replaces the fill and lifts the
            // icon; both are opaque, so the overlay replaces them exactly.
            .hover(ink("#404047"))
            .hover_group()
            .child(
                rune::el()
                    .key("composer-voice-dot")
                    .size(16.0, 16.0)
                    .inert()
                    .child(crate::motion::hover_icon(
                        16.0,
                        voice_icon(ink("#b6b6be")),
                        voice_icon(ink("#f5f5f6")),
                    )),
            );
    }
    button
}

/// Plain data for the toolbar (borrowed per frame; no handlers).
pub struct ComposerMeta<'a> {
    /// `"local"` | `"cloud"` (`runTarget`).
    pub target: &'a str,
    /// Usage totals for the orb + popover.
    pub usage: &'a super::rune_transcript::UsageTotals,
    /// Usage popover open.
    pub usage_open: bool,
    /// Live provider label (`OpenCode Go`, …).
    pub provider_label: &'a str,
    /// Amber pill (no key or provider error).
    pub provider_warn: bool,
    /// Thread model (`Automatic Fallback` default).
    pub model: &'a str,
    /// Model options; empty hides the dropup (the map dock never passes
    /// any — like `HouseChat` — while the sheet dock does, like `ChatView`).
    pub models: &'a [super::rune_transcript::ModelOption],
    /// Model menu open.
    pub model_open: bool,
    /// Dock attachments (`files` + `restored` in the reference UI — one deck here).
    pub attachments: &'a [super::chats::ChatAttachment],
    /// Amber validation/upload line under the input.
    pub attachments_error: Option<&'a str>,
    /// A file drag hovers the dock (`is-dragging`).
    pub attachments_dragging: bool,
    /// Chat identity for attachment thumbnails (API URL on the web).
    pub workspace_id: &'a str,
    pub chat_id: &'a str,
    /// Stable hover tracks (send scale, border transitions).
    pub hover: &'a crate::motion::HoverSet,
}

/// Run-target toggle (`radiogroup`): Local active is zinc-on-zinc-800,
/// Cloud active is sky with a pulsing cloud; inactive sides are dim zinc.
/// Flipping only re-marks the next prompt (the transfer lands next — sends
/// behave identically either way, same as the reference UI).
/// Lucide `CloudOff` / `Cloud` at a 12pt box for the run-target toggle.
fn cloud_icon(off: bool, color: rune::Color) -> rune::Shape {
    if off {
        crate::icons::cloud_off(12.0, color)
    } else {
        crate::icons::cloud(12.0, color)
    }
}

fn target_toggle<S: 'static>(
    target: &str,
    on_local: impl Fn(&mut S) + 'static,
    on_cloud: impl Fn(&mut S) + 'static,
) -> El {
    let cloud_active = target == "cloud";
    let mut cloud = rune::el()
        .key("composer-target-cloud-icon")
        .size(12.0, 12.0)
        .inert()
        .shape(cloud_icon(
            !cloud_active,
            ink(if cloud_active { "#e4e4e7" } else { "#71717a" }),
        ));
    if cloud_active {
        // The reference UI pulses the cloud icon (`animate-pulse`); same rhythm here.
        cloud = cloud.looping(rune::LoopSpec::opacity(2.0, 0.0, 0.4, 1.0));
    }
    row()
        .key("composer-target")
        .items_center()
        .radius(8.0)
        .fill(ink("#00000000"))
        .border(1.0, ink("#27272a"))
        .border_layout()
        .child(
            text("Local".to_string())
                .key("composer-target-local")
                .role("button")
                .focusable(true)
                .label("Run on Local — click to target Cloud (VPS)")
                .font({
                    // `text-[11px]` at the reference 1.5 line-height.
                    let mut style = rune::tok::TextStyle::new(11.0, 500.0);
                    style.line_height = 16.5;
                    style
                })
                .px(8.0)
                .py(6.0)
                .radius(6.0)
                .fill(ink(if cloud_active { "#00000000" } else { "#27272a" }))
                .color(ink(if cloud_active { "#71717a" } else { "#f4f4f5" }))
                .on_press_enter(on_local),
        )
        .child(
            row()
                .key("composer-target-cloud")
                .items_center()
                .gap(4.0)
                .role("button")
                .focusable(true)
                .label("Run on Cloud (VPS) — preference saved per chat")
                .px(8.0)
                .py(6.0)
                .radius(6.0)
                .fill(ink(if cloud_active {
                    "#a1a1aa66"
                } else {
                    "#00000000"
                }))
                .hover_group()
                .on_press_enter(on_cloud)
                .child(cloud)
                .child({
                    let mut label = text("Cloud".to_string())
                        .font({
                            let mut style = rune::tok::TextStyle::new(11.0, 500.0);
                            style.line_height = 16.5;
                            style
                        })
                        .color(ink(if cloud_active { "#e4e4e7" } else { "#71717a" }));
                    if !cloud_active {
                        label = label.group_color(ink("#d4d4d8"));
                    }
                    label
                }),
        )
}

/// While `running` the send circle becomes the Stop square
/// (`index.css .send-button.stop`: white circle, dark square, "Stop
/// execution") and presses `on_cancel` — the reference UI's `isRunning && empty → Stop`
/// simplified: a live run always stops first, a second prompt waits its turn.
/// A smooth circle as four cubic arcs (Rune has no arc primitive; the usage
/// ring is a scaled SVG circle, so a tessellated track shows as dots).
fn circle_path(cx: f32, cy: f32, r: f32) -> rune::Path {
    const K: f32 = 0.552_284_8;
    let k = r * K;
    rune::Path::new()
        .move_to([cx, cy - r])
        .cubic_to([cx + k, cy - r], [cx + r, cy - k], [cx + r, cy])
        .cubic_to([cx + r, cy + k], [cx + k, cy + r], [cx, cy + r])
        .cubic_to([cx - k, cy + r], [cx - r, cy + k], [cx - r, cy])
        .cubic_to([cx - r, cy - k], [cx - k, cy - r], [cx, cy - r])
        .close()
}

/// A translucent annulus as a single fill: Rune blends translucent strokes
/// twice wherever neighbouring segments meet, which brightens a stroked ring
/// (~13% track renders at ~24%). Outer ring clockwise, inner reversed, with
/// The slit tucked under the 12 o'clock progress cap.
fn ring_fill(cx: f32, cy: f32, r_in: f32, r_out: f32) -> Vec<[f32; 2]> {
    const N: usize = 96;
    let mut pts = Vec::with_capacity(N * 2);
    let at = |i: usize, r: f32| {
        let a = -std::f32::consts::FRAC_PI_2 + (i as f32 / N as f32) * 2.0 * std::f32::consts::PI;
        [cx + r * a.cos(), cy + r * a.sin()]
    };
    for i in 0..N {
        pts.push(at(i, r_out));
    }
    for i in (0..N).rev() {
        pts.push(at(i, r_in));
    }
    pts
}

/// Usage orb (`usage-orb`): 31px circle, tabular number = context percent,
/// ring track underneath. The reference UI scales a 40-unit SVG circle (r16, stroke 3)
/// into a 29pt box inset 1pt, so the drawn radius is 11.6 and the stroke
/// 2.175. The progress arc is tessellated (round caps, 3px-equivalent).
fn usage_orb<S: 'static>(
    usage: &super::rune_transcript::UsageTotals,
    on_usage: impl Fn(&mut S) + 'static,
) -> El {
    const RADIUS: f32 = 11.6;
    const WIDTH: f32 = 2.175;
    let pct = usage.context_percent();
    let ring_ink = if pct >= 95.0 {
        "#f87171"
    } else if pct >= 85.0 {
        "#a1a1aa"
    } else {
        "#e5e5e8"
    };
    let number_ink = if pct >= 95.0 {
        "#f87171"
    } else if pct >= 85.0 {
        "#a1a1aa"
    } else {
        "#dadade"
    };
    // Tessellated arc from 12 o'clock, clockwise, over `pct` of the circle.
    let mut pts = Vec::new();
    let steps = 64usize;
    let frac = (pct / 100.0).clamp(0.0, 1.0);
    let n = (frac * steps as f64).round() as usize;
    for i in 0..=n.min(steps) {
        let a =
            -std::f32::consts::FRAC_PI_2 + (i as f32 / steps as f32) * 2.0 * std::f32::consts::PI;
        pts.push([15.5 + RADIUS * a.cos(), 15.5 + RADIUS * a.sin()]);
    }
    let mut orb = rune::el()
        .key("composer-usage")
        .role("button")
        .focusable(true)
        .label(format!("Context {:.0}% of 1M", pct))
        .size(31.0, 31.0)
        .radius(15.5)
        .center()
        .hover(ink("#ffffff0e"))
        .hover_group()
        .on_press_enter(move |s: &mut S| on_usage(s))
        .child(
            text(format!("{:.0}", pct))
                .font(crate::motion::tabular(rune::tok::TextStyle::new(
                    9.0, 400.0,
                )))
                .color(ink(number_ink))
                .group_color(ink("#ffffff")),
        );
    // Track ring (`.usage-ring-track`): always visible, even at 0%.
    orb = orb.child(
        rune::el()
            .key("composer-usage-track")
            .absolute(0.0, 0.0)
            .size(31.0, 31.0)
            .inert()
            .shape(rune::Shape::new().fill(
                ring_fill(15.5, 15.5, RADIUS - WIDTH / 2.0, RADIUS + WIDTH / 2.0),
                ink("#ffffff21"),
            )),
    );
    if n == 0 && frac <= 0.0 {
        // `strokeDasharray: "0 100"` with a round cap paints a single dot.
        orb = orb.child(
            rune::el()
                .key("composer-usage-ring")
                .absolute(0.0, 0.0)
                .size(31.0, 31.0)
                .inert()
                .shape(rune::Shape::new().fill_path(
                    &circle_path(15.5, 15.5 - RADIUS, WIDTH / 2.0),
                    ink(ring_ink),
                )),
        );
    } else if !pts.is_empty() {
        orb = orb.child(
            rune::el()
                .key("composer-usage-ring")
                .absolute(0.0, 0.0)
                .size(31.0, 31.0)
                .inert()
                .shape(rune::Shape::new().stroke_with(
                    pts,
                    false,
                    WIDTH,
                    rune::LineCap::Round,
                    ink(ring_ink),
                )),
        );
    }
    orb
}

/// Usage popover: Cost / Tokens / Context / Cache rows + View details.
/// The reference UI portals it above the orb (or below when the window is short); the
/// anchored overlay reproduces that placement and flips at the edges.
fn usage_popover<S: 'static>(
    usage: &super::rune_transcript::UsageTotals,
    on_stats: Press<S>,
) -> El {
    let line = |key: &str, label: &str, value: String| {
        row()
            .key(format!("composer-usage-{key}"))
            .w_full()
            .items_center()
            .child(
                text(label.to_string())
                    .grow(1.0)
                    .font(rune::tok::TextStyle::new(11.0, 400.0))
                    .color(ink("#a2a2aa")),
            )
            .child(
                text(value)
                    .font(crate::motion::tabular(rune::tok::TextStyle::new(
                        11.0, 500.0,
                    )))
                    .color(ink("#e8e8eb")),
            )
    };
    let cache_rate = if usage.input > 0 {
        (usage.cache_hit as f64 / usage.input as f64) * 100.0
    } else {
        0.0
    };
    col()
        .key("composer-usage-popover")
        .anchored_fit("composer-usage", rune::Side::Top, rune::Align::End, 9.0)
        .anchored_round()
        .w(190.0)
        .gap(3.0)
        .transition(crate::motion::popover_enter())
        .radius(12.0)
        .fill(ink("#252529"))
        .border(1.0, ink("#ffffff1f"))
        .px(10.0)
        .py(8.0)
        .child(
            text("Usage".to_string())
                .font(rune::tok::TextStyle::new(12.0, 600.0))
                .color(ink("#f0f0f2")),
        )
        .child(line("cost", "Cost", format!("${:.5}", usage.cost)))
        .child(line(
            "tokens",
            "Tokens",
            (usage.input + usage.output).to_string(),
        ))
        .child(line("context", "Context", usage.active.to_string()))
        .child(line("cache", "Cache", format!("{cache_rate:.1}%")))
        .child(
            text("View details".to_string())
                .key("composer-usage-details")
                .role("button")
                .focusable(true)
                .label("Open usage details")
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .w_full()
                .py(8.0)
                .color(ink("#e8e8eb"))
                .on_press_enter(move |s: &mut S| on_stats(s)),
        )
}

/// Provider pill (`provider-trigger`): live gateway label, amber when the
/// key is missing or the provider reports an error. Opens the agents &
/// providers modal, like the reference UI.
fn provider_pill<S: 'static>(
    label: &str,
    warn: bool,
    on_config: Press<S>,
    hover: &crate::motion::HoverSet,
) -> El {
    let border = hover.border("composer-provider-border", ink("#ffffff12"));
    let set_border = border.clone();
    row()
        .key("composer-provider")
        .role("button")
        .focusable(true)
        .label("Configure agents and providers")
        .items_center()
        .px(8.0)
        .py(7.0)
        .radius(9.0)
        .fill(rune::Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.04,
        })
        .border(1.0, ink("#ffffff12"))
        .border_layout()
        // `.provider-trigger:hover` replaces the 4% wash with 8% over the
        // dock. Rune's hover overlay composites over the pill's own fill, so
        // use the opaque result of 8% white over the dock instead.
        .hover(crate::motion::hover_replace(
            rune::Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.04,
            },
            rune::Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.08,
            },
            ink("#101013"),
        ))
        .live_border(border.smooth(crate::motion::HOVER_SPRING))
        .hover_group()
        .on_hover_change(move |_s: &mut S, inside, tx| {
            set_border.set(if inside {
                ink("#ffffff1f")
            } else {
                ink("#ffffff12")
            });
            tx.live_only();
        })
        .on_press_enter(move |s: &mut S| on_config(s))
        .child(
            text(if label.is_empty() {
                "Provider".to_string()
            } else {
                label.to_string()
            })
            .font({
                // `.provider-trigger`: font-size 11px, line-height 1.
                let mut style = rune::tok::TextStyle::new(11.0, 400.0);
                style.line_height = 11.0;
                style
            })
            .color(ink(if warn { "#a1a1aa" } else { "#c9c9d1" }))
            .group_color(ink("#ededf1")),
        )
}

/// Model dropup (`provider-dropup`): trigger with the current model +
/// chevron, menu with Automatic Fallback head row + one row per model, check
/// on the selected. Only mounted when options exist.
fn model_dropup<S: 'static>(
    model: &str,
    models: &[super::rune_transcript::ModelOption],
    open: bool,
    menu: Press<S>,
    pick: Pick<S>,
    hover: &crate::motion::HoverSet,
) -> El {
    let border = hover.border("composer-model-border", ink("#ffffff12"));
    let set_border = border.clone();
    let close = menu.clone();
    let mut wrap = col().key("composer-model").gap(4.0);
    let current = if model.is_empty() {
        "Automatic Fallback".to_string()
    } else {
        models
            .iter()
            .find(|m| m.id == model)
            .map(|m| m.name.clone())
            .unwrap_or_else(|| model.to_string())
    };
    wrap = wrap.child(
        row()
            .key("composer-model-trigger")
            .items_center()
            .gap(4.0)
            .role("button")
            .focusable(true)
            .label("Model")
            .px(8.0)
            .py(7.0)
            .radius(9.0)
            .fill(rune::Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.04,
            })
            .border(1.0, ink("#ffffff12"))
            .hover(crate::motion::hover_replace(
                rune::Color {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.04,
                },
                rune::Color {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.08,
                },
                ink("#101013"),
            ))
            .live_border(border.smooth(crate::motion::HOVER_SPRING))
            .hover_group()
            .on_hover_change(move |_s: &mut S, inside, tx| {
                set_border.set(if inside {
                    ink("#ffffff1f")
                } else {
                    ink("#ffffff12")
                });
                tx.live_only();
            })
            .on_press_enter(move |s: &mut S| menu(s))
            .child(
                text(current)
                    .font(rune::tok::TextStyle::new(11.0, 400.0))
                    .color(ink("#c9c9d1"))
                    .group_color(ink("#ededf1")),
            )
            .child({
                let chevron =
                    |color: rune::Color| crate::icons::lucide(&["m6 9 6 6 6-6"], 10.0, 2.0, color);
                rune::el()
                    .size(10.0, 10.0)
                    .inert()
                    .rotate(if open { std::f32::consts::PI } else { 0.0 })
                    .rotate_motion(crate::motion::ROTATE)
                    .child(crate::motion::hover_icon(
                        10.0,
                        chevron(ink("#71717a")),
                        chevron(ink("#e4e4e7")),
                    ))
            }),
    );
    if open {
        let mut menu_col = col()
            .key("composer-model-menu")
            .role("menu")
            .label("Model")
            .focus_group()
            .focus_return("composer-model-trigger")
            // The reference UI `.model-menu`: width 264, max-height 280, absolute above
            // The trigger with an 8 pt gap. `anchored_fit` flips it below and
            // bounds it at small window heights.
            .anchored_fit(
                "composer-model-trigger",
                rune::Side::Top,
                rune::Align::Start,
                8.0,
            )
            .w(264.0)
            .max_h(280.0)
            .gap(2.0)
            .transition(crate::motion::menu_enter())
            .radius(9.0)
            .fill(ink("#17171b"))
            .border(1.0, ink("#ffffff14"))
            .px(6.0)
            .py(4.0)
            .modal(true, move |s: &mut S| close(s));
        let head_selected = model.is_empty() || model == "Automatic Fallback";
        menu_col = menu_col.child(model_row::<S>(
            "Automatic Fallback",
            "Automatic Fallback",
            head_selected,
            pick.clone(),
        ));
        for m in models {
            menu_col = menu_col.child(model_row::<S>(&m.id, &m.name, m.id == model, pick.clone()));
        }
        wrap = wrap.child(menu_col);
    }
    wrap
}

/// One model menu row with a check on the selected (`provider-item`).
fn model_row<S: 'static>(id: &str, name: &str, selected: bool, pick: Pick<S>) -> El {
    let target = id.to_string();
    let mut el = row()
        .key(format!("composer-model-{target}"))
        .w_full()
        .items_center()
        .gap(6.0)
        .px(8.0)
        .py(6.0)
        .radius(6.0)
        .fill(if selected {
            ink("#a1a1aa24")
        } else {
            ink("#00000000")
        })
        .role("menuitem")
        .label(format!("Use {name}"))
        .focusable(true)
        .focus_selected(selected)
        // `.model-menu button:hover` is the same violet wash as `.selected`;
        // one highlight serves pointer and arrow keys, as NSMenu does.
        .focus_highlight(ink("#a1a1aa24"));
    if !selected {
        el = el.hover(ink("#a1a1aa24")).hover_group();
    }
    el.on_press(move |s: &mut S| pick(s, target.clone()))
        .child(
            text(name.to_string())
                .grow(1.0)
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .color(ink(if selected { "#f4f4f5" } else { "#d4d4d8" }))
                .group_color(ink("#f4f4f5")),
        )
        .child(
            text(if selected { "✓" } else { "" }.to_string())
                .font(rune::tok::TextStyle::new(11.0, 600.0))
                .color(ink("#f4f4f5")),
        )
}

/// Premium-surface composer (`PromptDock`): 15px input, then the toolbar —
/// run-target toggle, spacer, usage orb (+popover), provider pill, voice
/// toggle + meter, send/stop. `w_full` — callers set the width. `on_send`
/// runs on send-button press and field submit; each view wires its own state
/// type through `edit_of`. The attach control opens the native picker, and
/// drag/drop lanes feed the same draft list.
/// Toolbar handlers, boxed so the card takes six params, not ten.
pub struct ComposerButtons<S: 'static> {
    pub voice: Press<S>,
    pub cancel: Press<S>,
    pub local: Press<S>,
    pub cloud: Press<S>,
    pub usage: Press<S>,
    pub send: Press<S>,
    pub model_menu: Press<S>,
    pub model_pick: Pick<S>,
    /// Usage popover "View details" → stats modal.
    pub stats: Press<S>,
    /// Provider pill → provider keys and OpenCode Go allowance.
    pub config: Press<S>,
    /// Attach control (opens the native file dialog).
    pub attach: PressTx<S>,
    /// Remove one deck card (`local:<index>` or the upload's id).
    pub remove_attachment: Pick<S>,
    /// Files dropped on the dock (mapped to the active thread by the view).
    pub drop_files: DropFiles<S>,
    /// Drag enters/leaves the dock (`is-dragging` highlight).
    pub drop_hover: DropHover<S>,
}

pub fn composer_card<S: 'static>(
    edit: &TextEdit,
    edit_of: std::rc::Rc<dyn for<'a> Fn(&'a mut S) -> &'a mut TextEdit>,
    voice: crate::voice::VoiceMeter,
    running: bool,
    meta: ComposerMeta<'_>,
    buttons: ComposerButtons<S>,
) -> El {
    let voice_live = voice.live;
    let voice_armed = voice.armed;
    let submit = {
        let on_send = buttons.send.clone();
        move |s: &mut S, _tx: &mut Tx| on_send(s)
    };
    let key_edit = edit_of.clone();
    let key_send = buttons.send.clone();
    let input = text_input(edit, {
        // `.prompt-input`: 15px at line-height 1.55, padding 3px top.
        let mut style = rune::tok::TextStyle::new(15.0, 400.0);
        style.line_height = 15.0 * 1.55;
        style
    })
    .key("composer-input")
    .w_full()
    .pt(3.0)
    .pb(8.0)
    .placeholder(
        if voice_live {
            "Listening…"
        } else if voice_armed {
            "Live mode on — talk from the island"
        } else if !meta.attachments.is_empty() {
            // PromptDock's deck note; the deck clears it after send.
            "Add a note about these files…"
        } else {
            "Ask anything"
        },
        ink("#74747deb"),
    )
    .caret(ink("#f2f2f4"), crate::motion::TEXT_SELECTION)
    .on_edit(move |s: &mut S| edit_of(s))
    .on_input_key(move |s: &mut S, key: &rune::KeyEvent, _tx: &mut Tx| {
        if key.key != rune::Key::Enter || key.mods.shift {
            return false;
        }
        // A composition's Return belongs to the input method.
        if key_edit(s).preedit.is_some() {
            return false;
        }
        key_send(s);
        true
    })
    .on_submit(submit);
    // Send / Stop (`.send-button`): the later `index.css` block wins — a
    // white 34 pt circle with dark glyph, hover `scale(1.04)`, disabled at
    // 72%, and the running empty state as a dark square.
    let has_text = !edit.text.trim().is_empty();
    let has_content = has_text || !meta.attachments.is_empty();
    let send_scale = meta.hover.scale("composer-send-scale", 1.0);
    let mut send_btn = rune::el()
        .role("button")
        .focusable(true)
        .label(if running { "Stop execution" } else { "Send" })
        .size(34.0, 34.0)
        .radius(17.0)
        .center();
    if running && !has_content {
        let set_scale = send_scale.clone();
        send_btn = send_btn
            .key("composer-stop")
            .fill(ink("#ffffff"))
            .on_press_enter({
                let cancel = buttons.cancel.clone();
                move |s: &mut S| cancel(s)
            })
            // `.pressable:active { transform: scale(.96) }`; `press_scale`
            // only records after a press handler exists.
            .press_scale(0.96)
            .live_scale(send_scale.smooth(crate::motion::HOVER_SPRING))
            .on_hover_change(move |_s: &mut S, inside, tx| {
                set_scale.set(if inside { 1.04 } else { 1.0 });
                tx.live_only();
            })
            .child(
                rune::el()
                    .size(14.0, 14.0)
                    .inert()
                    .center()
                    .child(crate::lucide_square_filled(14.0, ink("#111113"))),
            );
    } else if has_content {
        let set_scale = send_scale.clone();
        send_btn = send_btn
            .key("composer-send")
            .fill(ink("#ffffff"))
            .on_press_enter({
                let on_send = buttons.send.clone();
                move |s: &mut S| on_send(s)
            })
            .press_scale(0.96)
            .live_scale(send_scale.smooth(crate::motion::HOVER_SPRING))
            .on_hover_change(move |_s: &mut S, inside, tx| {
                set_scale.set(if inside { 1.04 } else { 1.0 });
                tx.live_only();
            })
            .child(
                rune::el()
                    .size(16.0, 16.0)
                    .inert()
                    .shape(crate::lucide_arrow_up(16.0, 2.4, ink("#111113"))),
            );
    } else {
        send_btn = send_btn
            .key("composer-send")
            .fill(ink("#ffffff"))
            .opacity(0.72)
            .disabled(true)
            .child(
                rune::el()
                    .size(16.0, 16.0)
                    .inert()
                    .shape(crate::lucide_arrow_up(16.0, 2.4, ink("#111113"))),
            );
    }
    let dragging = meta.attachments_dragging;
    let border_color = if dragging {
        // `.premium-surface.is-dragging`: white .55.
        rune::Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.55,
        }
    } else if voice_live {
        // `.premium-surface.is-listening`: violet .58 wins over hover.
        ink("#a1a1aa94")
    } else {
        // `.premium-surface` rests at white .08; hover lifts to .14.
        rune::Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.08,
        }
    };
    let hover_border = meta.hover.border("composer-card-border", border_color);
    let mut card = col()
        .key("composer-card")
        .w_full()
        .radius(18.0)
        .fill(ink("#101013"))
        .linear_gradient(
            145.0 * std::f32::consts::PI / 180.0,
            [(0.0, ink("#161618fa")), (1.0, ink("#0e0e10fc"))],
        )
        .border(1.0, border_color)
        .border_layout();
    // `.prompt-dock:focus-within` removes the drop shadow. Listening and
    // dragging have their own later shadows and keep them.
    if !voice_live && !dragging {
        card = card.focus_within_flat();
    }
    if dragging {
        // `.premium-surface.is-dragging`: lifted ring + scale.
        card = card.scale(1.008).box_shadow(rune::Shadow {
            x: 0.0,
            y: 0.0,
            blur: 0.0,
            spread: 3.0,
            color: rune::Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.08,
            },
            inset: false,
        });
        card = card.shadow(
            22.0,
            60.0,
            rune::Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.38,
            },
        );
    } else if voice_live {
        // `.premium-surface.is-listening`: violet ring + glow + soft drop.
        card = card
            .box_shadow(rune::Shadow {
                x: 0.0,
                y: 0.0,
                blur: 0.0,
                spread: 1.0,
                color: ink("#a1a1aa1f"),
                inset: false,
            })
            .box_shadow(rune::Shadow {
                x: 0.0,
                y: 0.0,
                blur: 34.0,
                spread: 0.0,
                color: ink("#a1a1aa24"),
                inset: false,
            })
            .shadow(
                14.0,
                42.0,
                rune::Color {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.26,
                },
            );
    } else {
        card = card
            .shadow(
                14.0,
                42.0,
                rune::Color {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.34,
                },
            )
            // `.premium-surface`: `0 14px 42px … , inset 0 1px 0 .025`.
            .box_shadow(crate::motion::inset_top_light(0.025));
    }
    if !voice_live && !dragging {
        // `.premium-surface:hover` border .14, over the .08 rest.
        let set_border = hover_border.clone();
        card = card
            .live_border(hover_border.smooth(crate::motion::HOVER_SPRING))
            .on_hover_change(move |_s: &mut S, inside, tx| {
                set_border.set(rune::Color {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: if inside { 0.14 } else { 0.08 },
                });
                tx.live_only();
            });
    }
    // The reference UI's dock is its own drop target (`onDragEnter/Leave/Over/Drop`).
    card = card
        .accepts_files(|_| true)
        .on_drop_files({
            let drop_files = buttons.drop_files.clone();
            move |s: &mut S, files: &[std::path::PathBuf], _tx: &mut Tx| drop_files(s, files)
        })
        .on_drop_hover({
            let drop_hover = buttons.drop_hover.clone();
            move |s: &mut S, over: bool, _tx: &mut Tx| drop_hover(s, over)
        });
    // `.attachment-deck`: horizontal strip, scrollbar hidden.
    if !meta.attachments.is_empty() {
        let mut deck = row()
            .key("composer-attachment-deck")
            .role("group")
            .label("Files ready to send")
            .w_full()
            .scroll_x()
            .items_center()
            .gap(8.0)
            .px(12.0)
            .pt(12.0)
            .pb(2.0);
        for (index, file) in meta.attachments.iter().enumerate() {
            deck = deck.child(attachment_card(
                file,
                index,
                meta.workspace_id,
                meta.chat_id,
                buttons.remove_attachment.clone(),
            ));
        }
        deck = deck.child(attachment_add(buttons.attach.clone(), meta.hover));
        card = card.child(deck);
    }
    // The reference UI's `.prompt-dock` has no padding of its own: the input wrapper
    // is 54pt (10/4 + 40pt textarea) and the toolbar 54pt (border + 9/10
    // + 34pt controls), so the bordered dock is exactly 110pt.
    card = card.child(
        col()
            .key("composer-input-wrap")
            .w_full()
            .px(12.0)
            .pt(10.0)
            .pb(4.0)
            .child(rune::auto_grow(input, 1, 8).min_h(40.0).max_h(180.0)),
    );
    if let Some(error) = meta.attachments_error {
        // `mx-4 mb-2 text-[11px] text-amber-300/90`.
        card = card.child(
            text(error.to_string())
                .key("composer-attachments-error")
                .w_full()
                .px(16.0)
                .pb(8.0)
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .color(ink("#a1a1aae6")),
        );
    }
    card = card.child(
        col()
            .key("composer-toolbar-wrap")
            .w_full()
            .px(12.0)
            // Toolbar top hairline (`.prompt-toolbar border-top` — Rune
            // draws all-side borders only, so a 1pt rule stands in).
            .child(
                rune::el()
                    .key("composer-div")
                    .w_full()
                    .h(1.0)
                    .fill(rune::Color {
                        r: 1.0,
                        g: 1.0,
                        b: 1.0,
                        a: 0.07,
                    }),
            )
            .child(
                row()
                    .key("composer-toolbar")
                    .w_full()
                    .items_center()
                    .px(2.0)
                    .pt(9.0)
                    .pb(10.0)
                    .gap(8.0)
                    .child(
                        row()
                            .key("composer-left")
                            .items_center()
                            .gap(4.0)
                            .child(attach_button(buttons.attach.clone()))
                            .child(target_toggle::<S>(
                                meta.target,
                                {
                                    let local = buttons.local.clone();
                                    move |s: &mut S| local(s)
                                },
                                {
                                    let cloud = buttons.cloud.clone();
                                    move |s: &mut S| cloud(s)
                                },
                            )),
                    )
                    .child(spacer())
                    .child(if meta.attachments.is_empty() {
                        // `hidden sm:block` count; empty means a zero-width
                        // flex item, so cancel one of its two gaps.
                        rune::el()
                            .key("composer-attachment-count")
                            .size(0.0, 0.0)
                            .ml(-8.0)
                            .inert()
                    } else {
                        text(format!(
                            "{} attachment{}",
                            meta.attachments.len(),
                            if meta.attachments.len() > 1 { "s" } else { "" }
                        ))
                        .key("composer-attachment-count")
                        .font(rune::tok::TextStyle::new(10.0, 400.0))
                        .color(ink("#52525b"))
                    })
                    .child(usage_orb::<S>(meta.usage, {
                        let usage = buttons.usage.clone();
                        move |s: &mut S| usage(s)
                    }))
                    .child(provider_pill(
                        meta.provider_label,
                        meta.provider_warn,
                        buttons.config.clone(),
                        meta.hover,
                    ))
                    .child(if meta.models.is_empty() {
                        // Map dock: no model list (HouseChat never passes
                        // `onModelChange`). The negative margin cancels
                        // The flex gap so the right cluster still packs
                        // provider → voice at 8pt like the reference UI.
                        rune::el()
                            .key("composer-model-empty")
                            .size(0.0, 0.0)
                            .ml(-8.0)
                            .inert()
                    } else {
                        model_dropup::<S>(
                            meta.model,
                            meta.models,
                            meta.model_open,
                            buttons.model_menu.clone(),
                            buttons.model_pick.clone(),
                            meta.hover,
                        )
                    })
                    .child(voice_button(&voice, {
                        let voice = buttons.voice.clone();
                        move |s: &mut S| voice(s)
                    }))
                    .child(send_btn),
            ),
    );
    let mut card = card;
    if meta.usage_open {
        card = card.child(usage_popover::<S>(meta.usage, buttons.stats.clone()));
    }
    card
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Cx, Env, Runtime};
    use std::rc::Rc;

    fn models() -> Vec<super::super::rune_transcript::ModelOption> {
        vec![
            super::super::rune_transcript::ModelOption {
                id: "fast".to_string(),
                name: "Fast".to_string(),
            },
            super::super::rune_transcript::ModelOption {
                id: "deep".to_string(),
                name: "Deep".to_string(),
            },
        ]
    }

    fn view_model_menu(top: &f32, _: &mut Cx<f32>) -> El {
        let models = models();
        let hover = crate::motion::HoverSet::default();
        col()
            .w_full()
            .h_full()
            .pt(*top)
            .pl(40.0)
            .child(model_dropup::<f32>(
                "",
                &models,
                true,
                Rc::new(|_: &mut f32| {}),
                Rc::new(|_: &mut f32, _: String| {}),
                &hover,
            ))
    }

    #[test]
    fn anchored_model_menu_does_not_grow_the_dock_and_flips() {
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(250.0, view_model_menu, (600.0, 400.0), env);
        rt.frame(0.0);
        let (menu, _) = rt.presented("composer-model-menu").expect("menu mounted");
        let (trigger, _) = rt
            .presented("composer-model-trigger")
            .expect("trigger mounted");
        let (dock, _) = rt.presented("composer-model").expect("dock mounted");
        assert!((menu[2] - 264.0).abs() < 0.51, "menu width {}", menu[2]);
        assert!(
            (menu[0] - trigger[0]).abs() < 0.51,
            "start-aligned: menu {} trigger {}",
            menu[0],
            trigger[0]
        );
        assert!(
            menu[1] + menu[3] <= trigger[1] - 7.5,
            "8 pt above the trigger: menu {}..{} trigger {}",
            menu[1],
            menu[1] + menu[3],
            trigger[1]
        );
        assert!(
            (dock[3] - trigger[3]).abs() < 0.51,
            "absolute menu must not grow the wrapper: dock {} trigger {}",
            dock[3],
            trigger[3]
        );

        let mut rt = Runtime::new(20.0, view_model_menu, (600.0, 400.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let (menu, _) = rt.presented("composer-model-menu").expect("menu mounted");
        let (trigger, _) = rt
            .presented("composer-model-trigger")
            .expect("trigger mounted");
        assert!(
            menu[1] >= trigger[1] + trigger[3] + 7.5,
            "flips below at the top edge: menu {} trigger bottom {}",
            menu[1],
            trigger[1] + trigger[3]
        );
    }
}
