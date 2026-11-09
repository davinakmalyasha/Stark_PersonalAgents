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
fn voice_meter(live: bool, level: f32, note: Option<&str>) -> El {
    let lit = if live {
        crate::voice::lit_bars(level)
    } else {
        0
    };
    let mut bars = row().key("voice-meter").items_center().gap(3.0);
    for i in 0..5 {
        let h = 4.0 + i as f32 * 3.0;
        bars = bars.child(
            rune::el()
                .key(format!("voice-bar-{i}"))
                .size(3.0, h)
                .radius(1.5)
                .fill(ink(if live && i < lit {
                    "#ededf0"
                } else {
                    "#3a3a40"
                })),
        );
    }
    let mut meter = col().key("voice-meter-wrap").gap(2.0).child(bars);
    if live {
        if let Some(note) = note {
            meter = meter.child(
                text(note.to_string())
                    .key("voice-note")
                    .font(rune::tok::TextStyle::new(10.0, 400.0))
                    .color(ink("#8a8a93")),
            );
        }
    }
    meter
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
                .size(8.0, 8.0)
                .radius(4.0)
                .fill(ink("#b6b6be")),
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
fn target_toggle<S: 'static>(
    target: &str,
    on_local: impl Fn(&mut S) + 'static,
    on_cloud: impl Fn(&mut S) + 'static,
) -> El {
    let cloud_active = target == "cloud";
    let mut cloud_icon = text("☁".to_string())
        .key("composer-target-cloud-icon")
        .font(rune::tok::TextStyle::new(11.0, 400.0))
        .color(ink(if cloud_active { "#7dd3fc" } else { "#71717a" }));
    if cloud_active {
        // React pulses the cloud icon (`animate-pulse`); same rhythm here.
        cloud_icon = cloud_icon.looping(rune::LoopSpec::opacity(1.4, 0.0, 0.4, 1.0));
    }
    row()
        .key("composer-target")
        .items_center()
        .radius(8.0)
        .fill(ink("#00000000"))
        .border(1.0, ink("#27272a"))
        .child(
            text("Local".to_string())
                .key("composer-target-local")
                .role("button")
                .label("Run on Local — click to target Cloud (VPS)")
                .font(rune::tok::TextStyle::new(11.0, 500.0))
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
                .child(cloud_icon)
                .child(
                    text("Cloud".to_string())
                        .font(rune::tok::TextStyle::new(11.0, 500.0))
                        .color(ink(if cloud_active { "#7dd3fc" } else { "#71717a" })),
                ),
        )
}

/// While `running` the send circle becomes the Stop square
/// (`index.css .send-button.stop`: white circle, dark square, "Stop
/// execution") and presses `on_cancel` — React's `isRunning && empty → Stop`
/// simplified: a live run always stops first, a second prompt waits its turn.
/// Usage orb (`usage-orb`): 31px circle, tabular number = context percent,
/// ring track underneath. The progress arc is tessellated segments stroked
/// round-capped (Rune has no arc primitive) — same 3px width, same color
/// steps (95+ red, 85+ amber, else light).
fn usage_orb<S: 'static>(
    usage: &super::rune_transcript::UsageTotals,
    on_usage: impl Fn(&mut S) + 'static,
) -> El {
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
    // Tessellated arc from -90°, clockwise, over `pct` of the circle.
    let mut pts = Vec::new();
    let steps = 28usize;
    let frac = (pct / 100.0).clamp(0.0, 1.0);
    let n = (frac * steps as f64).round() as usize;
    for i in 0..=n.max(1).min(steps) {
        let a =
            -std::f32::consts::FRAC_PI_2 + (i as f32 / steps as f32) * 2.0 * std::f32::consts::PI;
        pts.push([15.5 + 13.0 * a.cos(), 15.5 + 13.0 * a.sin()]);
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
                .font(rune::tok::TextStyle::new(9.0, 400.0))
                .color(ink(number_ink)),
        );
    if n > 0 {
        orb = orb.child(
            rune::el()
                .key("composer-usage-ring")
                .absolute(0.0, 0.0)
                .size(31.0, 31.0)
                .inert()
                .shape(rune::Shape::new().stroke_with(
                    pts,
                    false,
                    3.0,
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
    .font(rune::tok::TextStyle::new(11.0, 400.0))
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
    let mut wrap = col().key("composer-model").w_full().gap(4.0);
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
    let (voice_live, voice_level, voice_note) = voice;
    let submit = {
        let on_send = buttons.send.clone();
        move |s: &mut S, _tx: &mut Tx| on_send(s)
    };
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
        .px(12.0)
        .pt(10.0)
        .pb(10.0)
        .gap(8.0)
        .child(
            text_input(edit, rune::tok::TextStyle::new(15.0, 400.0))
                .key("composer-input")
                .w_full()
                .placeholder(
                    if voice_live {
                        "Listening…"
                    } else {
                        "Ask anything"
                    },
                    ink("#74747d"),
                )
                .caret(ink("#f2f2f4"), ink("#3b82f6"))
                .on_edit(edit_of)
                .on_submit(submit),
        )
        // Toolbar top hairline (`.prompt-toolbar border-top` — Rune draws
        // all-side borders only, so a 1pt rule stands in).
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
                .gap(8.0)
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
                ))
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
                    // Map dock: no model list (like `HouseChat`) — the
                    // provider pill above is the only pill, per screenshot.
                    rune::el()
                        .key("composer-model-empty")
                        .size(0.0, 0.0)
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
                .child(
                    // Voice toggle (`.prompt-voice-button`) with a 5-bar level
                    // meter beside it while live. Levels come from the capture
                    // thread via the voice bus; without a mic the note says so.
                    voice_meter(voice_live, voice_level, voice_note),
                )
                .child(voice_button(voice_live, {
                    let voice = buttons.voice.clone();
                    move |s: &mut S| voice(s)
                }))
                .child(if running {
                    rune::el()
                        .key("composer-stop")
                        .role("button")
                        .label("Stop execution")
                        .size(34.0, 34.0)
                        .radius(17.0)
                        .fill(ink("#ffffff"))
                        .center()
                        .on_press({
                            let cancel = buttons.cancel.clone();
                            move |s: &mut S| cancel(s)
                        })
                        .child(
                            text("■".to_string())
                                .font(rune::tok::TextStyle::new(12.0, 650.0))
                                .color(ink("#111113")),
                        )
                } else {
                    rune::el()
                        .key("composer-send")
                        .role("button")
                        .label("Send")
                        .size(34.0, 34.0)
                        .radius(17.0)
                        .fill(ink("#ffffff"))
                        .center()
                        .on_press({
                            let on_send = buttons.send.clone();
                            move |s: &mut S| on_send(s)
                        })
                        .child(
                            text("↑".to_string())
                                .font(rune::tok::TextStyle::new(16.0, 650.0))
                                .color(ink("#111113")),
                        )
                }),
        );
    let mut card = card;
    if meta.usage_open {
        card = card.child(usage_popover::<S>(meta.usage, buttons.stats.clone()));
    }
    card
}
