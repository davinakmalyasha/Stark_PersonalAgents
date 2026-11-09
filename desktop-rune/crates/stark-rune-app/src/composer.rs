//! Shared composer card — the `.premium-surface` prompt dock.
//!
//! Used docked in the transcript view and floating over the canvas map.
//! Voice button and live `text_input` binding arrive next; the input is static
//! text for now.

use rune::{col, row, spacer, text, text_input, El, TextEdit, Tx};

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

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

/// Premium-surface composer: gradient card, live 15px input, toolbar with
/// model pill, spacer, voice toggle + meter, and white send circle. `w_full`
/// — callers set the width. `on_send` runs on send-button press and field
/// submit; `on_voice` toggles capture; each view wires its own state type
/// through `edit_of`. While `running` the send circle becomes the Stop square
/// (`index.css .send-button.stop`: white circle, dark square, "Stop
/// execution") and presses `on_cancel` — React's `isRunning && empty → Stop`
/// simplified: a live run always stops first, a second prompt waits its turn.
pub fn composer_card<S: 'static>(
    model: &str,
    edit: &TextEdit,
    edit_of: impl for<'a> Fn(&'a mut S) -> &'a mut TextEdit + 'static,
    voice: (bool, f32, Option<&str>),
    on_voice: impl Fn(&mut S) + 'static,
    stop: (bool, impl Fn(&mut S) + 'static),
    on_send: impl Fn(&mut S) + 'static,
) -> El {
    let (voice_live, voice_level, voice_note) = voice;
    let (running, on_cancel) = stop;
    let on_send = std::rc::Rc::new(on_send);
    let submit = {
        let on_send = on_send.clone();
        move |s: &mut S, _tx: &mut Tx| on_send(s)
    };
    col()
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
        .child(
            row()
                .key("composer-toolbar")
                .w_full()
                .items_center()
                .gap(8.0)
                .child(
                    row()
                        .key("model-pill")
                        .items_center()
                        .gap(6.0)
                        .radius(9.0)
                        .fill(rune::Color {
                            r: 1.0,
                            g: 1.0,
                            b: 1.0,
                            a: 0.04,
                        })
                        .px(8.0)
                        .py(6.0)
                        .child(
                            rune::el()
                                .key("model-dot")
                                .size(6.0, 6.0)
                                .radius(3.0)
                                .fill(ink("#aa8dff")),
                        )
                        .child(
                            text(model.to_string())
                                .key("model-name")
                                .font(rune::tok::TextStyle::new(11.0, 500.0))
                                .color(ink("#c9c9d1")),
                        ),
                )
                .child(spacer())
                .child(
                    // Voice toggle (`.prompt-voice-button`) with a 5-bar level
                    // meter beside it while live. Levels come from the capture
                    // thread via the voice bus; without a mic the note says so.
                    voice_meter(voice_live, voice_level, voice_note),
                )
                .child(voice_button(voice_live, on_voice))
                .child(if running {
                    rune::el()
                        .key("composer-stop")
                        .role("button")
                        .label("Stop execution")
                        .size(34.0, 34.0)
                        .radius(17.0)
                        .fill(ink("#ffffff"))
                        .center()
                        .on_press(on_cancel)
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
                            let on_send = on_send.clone();
                            move |s: &mut S| on_send(s)
                        })
                        .child(
                            text("↑".to_string())
                                .font(rune::tok::TextStyle::new(16.0, 650.0))
                                .color(ink("#111113")),
                        )
                }),
        )
}
