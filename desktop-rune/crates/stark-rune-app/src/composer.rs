//! Shared composer card — the `.premium-surface` prompt dock.
//!
//! Used docked in the transcript view and floating over the canvas map.
//! Voice button and live `text_input` binding arrive next; the input is static
//! text for now.

use rune::{col, row, spacer, text, text_input, El, TextEdit, Tx};

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

/// Premium-surface composer: gradient card, live 15px input, toolbar with
/// model pill, spacer, and white send circle. `w_full` — callers set the
/// width. `on_send` runs on send-button press and field submit; each view
/// wires its own state type through `edit_of`.
pub fn composer_card<S: 'static>(
    model: &str,
    edit: &TextEdit,
    edit_of: impl for<'a> Fn(&'a mut S) -> &'a mut TextEdit + 'static,
    on_send: impl Fn(&mut S) + 'static,
) -> El {
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
                .placeholder("Ask anything", ink("#74747d"))
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
                        ),
                ),
        )
}
