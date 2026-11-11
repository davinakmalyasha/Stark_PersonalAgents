//! Shared composer card — the `.premium-surface` prompt dock.
//!
//! Used docked in the transcript view and floating over the canvas map.
//! Voice button and live `text_input` binding arrive next; the input is static
//! text for now.

use rune::{col, row, spacer, text, text_input, El, TextEdit, Tx};

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

/// Boxed press handler (keeps the card signature small).
pub type Press<S> = std::rc::Rc<dyn Fn(&mut S)>;
/// Boxed pick handler (model rows carry their id).
pub type Pick<S> = std::rc::Rc<dyn Fn(&mut S, String)>;

/// Level meter: five bars lit by bucket, plus the capture note (no-device
/// reason) when present. Mounted beside the voice toggle while live.
fn attach_button() -> El {
    // `.tool-button` (Plus 18): React's file picker. Rune has no attachment
    // pipeline yet, so the control is present but inert — the dock's shape
    // still matches HouseChat.
    let color = ink("#b4b4bc");
    let plus = rune::Shape::new()
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
        .view_box(24.0, 24.0);
    rune::el()
        .key("composer-attach")
        .role("button")
        .label("Attach files")
        .size(30.0, 30.0)
        .radius(10.0)
        .center()
        .inert()
        .child(rune::el().size(18.0, 18.0).inert().shape(plus))
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

/// Voice toggle button. `S` is the owning view's state type.
fn voice_button<S: 'static>(live: bool, on_voice: impl Fn(&mut S) + 'static) -> El {
    let mut button = rune::el()
        .key("composer-voice")
        .role("button")
        .label(if live {
            "Stop voice input"
        } else {
            "Start voice input"
        })
        .size(32.0, 32.0)
        .radius(11.0)
        .center()
        .on_press(on_voice);
    if live {
        button = button
            .linear_gradient(
                135.0 * std::f32::consts::PI / 180.0,
                [(0.0, ink("#7860d9")), (1.0, ink("#aa8dff"))],
            )
            .child(
                rune::el()
                    .key("composer-voice-dot")
                    .size(8.0, 8.0)
                    .radius(4.0)
                    .fill(ink("#ffffff"))
                    .looping(rune::LoopSpec::opacity(1.2, 0.0, 0.4, 1.0)),
            );
    } else {
        button = button.fill(ink("#303035")).child(
            rune::el()
                .key("composer-voice-dot")
                .size(16.0, 16.0)
                .inert()
                .shape(voice_icon(ink("#b6b6be"))),
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
}

/// Run-target toggle (`radiogroup`): Local active is zinc-on-zinc-800,
/// Cloud active is sky with a pulsing cloud; inactive sides are dim zinc.
/// Flipping only re-marks the next prompt (the transfer lands next — sends
/// behave identically either way, same as React).
/// Lucide `CloudOff` / `Cloud` at a 12pt box for the run-target toggle.
fn cloud_icon(off: bool, color: rune::Color) -> rune::Shape {
    // Lucide v1.17 `cloud-off` / `cloud`.
    let mut p = if off {
        rune::Path::new()
            .move_to([10.94, 5.27])
            .cubic_to([13.23, 5.93], [15.03, 7.72], [15.71, 10.0])
            .line_to([17.5, 10.0])
            .cubic_to([18.97, 10.0], [20.35, 10.72], [21.19, 11.93])
            .cubic_to([22.03, 13.13], [22.23, 14.68], [21.72, 16.06])
            .move_to([18.8, 18.81])
            .cubic_to([18.38, 18.94], [17.94, 19.0], [17.5, 19.0])
            .line_to([9.0, 19.0])
            .cubic_to([5.77, 19.0], [2.96, 16.79], [2.2, 13.65])
            .cubic_to([1.44, 10.51], [2.92, 7.26], [5.79, 5.78])
    } else {
        rune::Path::new()
            .move_to([17.5, 19.0])
            .line_to([9.0, 19.0])
            .cubic_to([5.4, 19.0], [2.38, 16.26], [2.03, 12.67])
            .cubic_to([1.69, 9.09], [4.12, 5.82], [7.66, 5.13])
            .cubic_to([11.2, 4.44], [14.68, 6.55], [15.71, 10.0])
            .line_to([17.5, 10.0])
            .cubic_to([19.99, 10.0], [22.0, 12.01], [22.0, 14.5])
            .cubic_to([22.0, 16.99], [19.99, 19.0], [17.5, 19.0])
            .close()
    };
    if off {
        p = p.move_to([2.0, 2.0]).line_to([22.0, 22.0]);
    }
    rune::Shape::new()
        .stroke_path(&p, crate::lucide_stroke(12.0), rune::LineCap::Round, color)
        .view_box(24.0, 24.0)
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
            ink(if cloud_active { "#7dd3fc" } else { "#71717a" }),
        ));
    if cloud_active {
        // React pulses the cloud icon (`animate-pulse`); same rhythm here.
        cloud = cloud.looping(rune::LoopSpec::opacity(1.4, 0.0, 0.4, 1.0));
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
                .label("Run on Local — click to target Cloud (VPS)")
                .font({
                    // `text-[11px]` at Tailwind's 1.5 line-height.
                    let mut style = rune::tok::TextStyle::new(11.0, 500.0);
                    style.line_height = 16.5;
                    style
                })
                .px(8.0)
                .py(6.0)
                .radius(6.0)
                .fill(ink(if cloud_active { "#00000000" } else { "#27272a" }))
                .color(ink(if cloud_active { "#71717a" } else { "#f4f4f5" }))
                .on_press(on_local),
        )
        .child(
            row()
                .key("composer-target-cloud")
                .items_center()
                .gap(4.0)
                .role("button")
                .label("Run on Cloud (VPS) — preference saved per chat")
                .px(8.0)
                .py(6.0)
                .radius(6.0)
                .fill(ink(if cloud_active {
                    "#0c4a6e66"
                } else {
                    "#00000000"
                }))
                .on_press(on_cloud)
                .child(cloud)
                .child(
                    text("Cloud".to_string())
                        .font({
                            let mut style = rune::tok::TextStyle::new(11.0, 500.0);
                            style.line_height = 16.5;
                            style
                        })
                        .color(ink(if cloud_active { "#7dd3fc" } else { "#71717a" })),
                ),
        )
}

/// While `running` the send circle becomes the Stop square
/// (`index.css .send-button.stop`: white circle, dark square, "Stop
/// execution") and presses `on_cancel` — React's `isRunning && empty → Stop`
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
/// the slit tucked under the 12 o'clock progress cap.
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
/// ring track underneath. React scales a 40-unit SVG circle (r16, stroke 3)
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
        "#fbbf24"
    } else {
        "#e5e5e8"
    };
    let number_ink = if pct >= 95.0 {
        "#f87171"
    } else if pct >= 85.0 {
        "#fbbf24"
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
        .label(format!("Context {:.0}% of 1M", pct))
        .size(31.0, 31.0)
        .radius(15.5)
        .center()
        .on_press(move |s: &mut S| on_usage(s))
        .child(
            text(format!("{:.0}", pct))
                .font({
                    let mut style = rune::tok::TextStyle::new(9.0, 400.0);
                    style.features = "'tnum' 1";
                    style
                })
                .color(ink(number_ink)),
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
/// React hovers (portal above the orb); Rune taps the orb and the panel
/// expands under the toolbar (no portal/hover targeting headlessly —
/// position differs, content identical).
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
                    .font(rune::tok::TextStyle::new(11.0, 500.0))
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
        .w_full()
        .gap(3.0)
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
                .label("Open usage details")
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .w_full()
                .py(8.0)
                .color(ink("#e8e8eb"))
                .on_press(move |s: &mut S| on_stats(s)),
        )
}

/// Provider pill (`provider-trigger`): live gateway label, amber when the
/// key is missing or the provider reports an error. Opens the agents &
/// providers modal, like React.
fn provider_pill<S: 'static>(label: &str, warn: bool, on_config: Press<S>) -> El {
    text(if label.is_empty() {
        "Provider".to_string()
    } else {
        label.to_string()
    })
    .key("composer-provider")
    .role("button")
    .label("Configure agents and providers")
    .font({
        // `.provider-trigger`: font-size 11px, line-height 1.
        let mut style = rune::tok::TextStyle::new(11.0, 400.0);
        style.line_height = 11.0;
        style
    })
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
    .color(ink(if warn { "#fbbf24" } else { "#c9c9d1" }))
    .on_press(move |s: &mut S| on_config(s))
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
) -> El {
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
            .on_press(move |s: &mut S| menu(s))
            .child(
                text(current)
                    .font(rune::tok::TextStyle::new(11.0, 400.0))
                    .color(ink("#c9c9d1")),
            )
            .child(
                text(if open { "▴" } else { "▾" }.to_string())
                    .font(rune::tok::TextStyle::new(10.0, 400.0))
                    .color(ink("#71717a")),
            ),
    );
    if open {
        let mut menu_col = col()
            .key("composer-model-menu")
            .w_full()
            .gap(2.0)
            .radius(9.0)
            .fill(ink("#17171b"))
            .border(1.0, ink("#ffffff14"))
            .px(6.0)
            .py(4.0);
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
    row()
        .key(format!("composer-model-{target}"))
        .w_full()
        .items_center()
        .gap(6.0)
        .px(8.0)
        .py(6.0)
        .radius(6.0)
        .fill(ink(if selected { "#ffffff14" } else { "#00000000" }))
        .role("button")
        .label(format!("Use {name}"))
        .on_press(move |s: &mut S| pick(s, target.clone()))
        .child(
            text(name.to_string())
                .grow(1.0)
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .color(ink("#d4d4d8")),
        )
        .child(
            text(if selected { "✓" } else { "" }.to_string())
                .font(rune::tok::TextStyle::new(11.0, 600.0))
                .color(ink("#e4e4e7")),
        )
}

/// Premium-surface composer (`PromptDock`): 15px input, then the toolbar —
/// run-target toggle, spacer, usage orb (+popover), provider pill, voice
/// toggle + meter, send/stop. `w_full` — callers set the width. `on_send`
/// runs on send-button press and field submit; each view wires its own state
/// type through `edit_of`. No attach button / drop overlay: file pickers and
/// drag-drop have no Rune target (documented, not faked).
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
    /// Provider pill → agents & providers modal.
    pub config: Press<S>,
}

pub fn composer_card<S: 'static>(
    edit: &TextEdit,
    edit_of: impl for<'a> Fn(&'a mut S) -> &'a mut TextEdit + 'static,
    voice: (bool, f32, Option<&str>),
    running: bool,
    meta: ComposerMeta<'_>,
    buttons: ComposerButtons<S>,
) -> El {
    let (voice_live, _voice_level, _voice_note) = voice;
    let submit = {
        let on_send = buttons.send.clone();
        move |s: &mut S, _tx: &mut Tx| on_send(s)
    };
    // Send / Stop (`.send-button`): violet when a prompt is staged, white at
    // 72% while empty+idle, rose while running an empty prompt.
    let has_text = !edit.text.trim().is_empty();
    let mut send_btn = rune::el()
        .role("button")
        .label(if running { "Stop execution" } else { "Send" })
        .size(34.0, 34.0)
        .radius(17.0)
        .center();
    if running && !has_text {
        send_btn = send_btn
            .key("composer-stop")
            .fill(ink("#5d3544"))
            .on_press({
                let cancel = buttons.cancel.clone();
                move |s: &mut S| cancel(s)
            })
            .child(
                rune::el()
                    .size(14.0, 14.0)
                    .inert()
                    .center()
                    .child(crate::lucide_square_filled(14.0, ink("#ffbac9"))),
            );
    } else if has_text {
        send_btn = send_btn
            .key("composer-send")
            .linear_gradient(
                135.0 * std::f32::consts::PI / 180.0,
                [(0.0, ink("#aa8dff")), (1.0, ink("#7860d9"))],
            )
            .on_press({
                let on_send = buttons.send.clone();
                move |s: &mut S| on_send(s)
            })
            .child(
                rune::el()
                    .size(16.0, 16.0)
                    .inert()
                    .shape(crate::lucide_arrow_up(16.0, 2.4, ink("#ffffff"))),
            );
    } else {
        send_btn = send_btn
            .key("composer-send")
            .fill(ink("#ffffff"))
            .opacity(0.72)
            .on_press({
                let on_send = buttons.send.clone();
                move |s: &mut S| on_send(s)
            })
            .child(
                rune::el()
                    .size(16.0, 16.0)
                    .inert()
                    .shape(crate::lucide_arrow_up(16.0, 2.4, ink("#111113"))),
            );
    }
    let card = col()
        .key("composer-card")
        .w_full()
        .radius(18.0)
        .fill(ink("#101013"))
        .linear_gradient(
            145.0 * std::f32::consts::PI / 180.0,
            [(0.0, ink("#161618fa")), (1.0, ink("#0e0e10fc"))],
        )
        .border(
            1.0,
            rune::Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.08,
            },
        )
        .border_layout()
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
        // React's `.prompt-dock` has no padding of its own: the input wrapper
        // is 54pt (10/4 + 40pt textarea) and the toolbar 54pt (border + 9/10
        // + 34pt controls), so the bordered dock is exactly 110pt.
        .child(
            col()
                .key("composer-input-wrap")
                .w_full()
                .px(12.0)
                .pt(10.0)
                .pb(4.0)
                .child(
                    text_input(edit, {
                        // `.prompt-input`: 15px at line-height 1.55, padding 3px top.
                        let mut style = rune::tok::TextStyle::new(15.0, 400.0);
                        style.line_height = 15.0 * 1.55;
                        style
                    })
                    .key("composer-input")
                    .w_full()
                    .h(40.0)
                    .pt(3.0)
                    .placeholder(
                        if voice_live {
                            "Listening…"
                        } else {
                            "Ask anything"
                        },
                        ink("#74747deb"),
                    )
                    .caret(ink("#f2f2f4"), ink("#3b82f6"))
                    .on_edit(edit_of)
                    .on_submit(submit),
                ),
        )
        .child(
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
                                .child(attach_button())
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
                        .child(usage_orb::<S>(meta.usage, {
                            let usage = buttons.usage.clone();
                            move |s: &mut S| usage(s)
                        }))
                        .child(provider_pill(
                            meta.provider_label,
                            meta.provider_warn,
                            buttons.config.clone(),
                        ))
                        .child(if meta.models.is_empty() {
                            // Map dock: no model list (HouseChat never passes
                            // `onModelChange`). The negative margin cancels
                            // the flex gap so the right cluster still packs
                            // provider → voice at 8pt like React.
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
                            )
                        })
                        .child(voice_button(voice_live, {
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
