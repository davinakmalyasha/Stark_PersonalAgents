//! Shared composer card — the `.premium-surface` prompt dock.
//!
//! Used docked in the transcript view and floating over the canvas map.
//! Voice button and live `text_input` binding arrive next; the input is static
//! text for now.

use rune::{col, row, spacer, text, El};

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

/// Premium-surface composer: gradient card, 15px input, toolbar with model
/// pill, spacer, and white send circle. `w_full` — callers set the width.
/// `on_send` runs on send-button press (each view wires its own state type).
pub fn composer_card<S: 'static>(
    model: &str,
    draft: &str,
    on_send: impl Fn(&mut S) + 'static,
) -> El {
    let input = if draft.is_empty() {
        "Ask anything".to_string()
    } else {
        draft.to_string()
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
            text(input)
                .key("composer-input")
                .font(rune::tok::TextStyle::new(15.0, 400.0))
                .wrap()
                .color(ink("#74747d")),
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
                    rune::el()
                        .key("composer-send")
                        .role("button")
                        .label("Send")
                        .size(34.0, 34.0)
                        .radius(17.0)
                        .fill(ink("#ffffff"))
                        .center()
                        .on_press(on_send)
                        .child(
                            text("↑".to_string())
                                .font(rune::tok::TextStyle::new(16.0, 650.0))
                                .color(ink("#111113")),
                        ),
                ),
        )
}
