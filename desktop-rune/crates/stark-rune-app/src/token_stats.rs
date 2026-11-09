//! Usage stats — port of `TokenStatsModal.tsx` + `Dashboard/TokenStats.tsx`
//! (`TokenSummary.tsx`, `CallList.tsx`).
//!
//! The modal reads the thread's accumulated usage (`combinedTokenUsage`
//! twin): totals + per-call rows. Tokens verbatim (`toLocaleString` ≈ plain
//! digits — Rune has no locale grouping; documented, not faked).

use rune::{col, row, text, El};

use crate::rune_transcript::{UsageCall, UsageTotals};

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

/// Model context limit (`getModelContextLimit`).
pub fn context_limit(model: &str) -> u64 {
    let name = model.to_lowercase();
    if name.contains("gemini-1.5-pro") || name.contains("gemini-3.5-pro") {
        2_000_000
    } else if name.contains("gemini")
        || name.contains("deepseek-v4")
        || name.contains("deepseek-flash")
        || name.contains("deepseek-chat")
        || name.contains("deepseek-v3")
    {
        1_000_000
    } else if name.contains("claude-3") || name.contains("claude-3-5") {
        200_000
    } else if name.contains("reasoner")
        || name.contains("r1")
        || name.contains("gpt-4")
        || name.contains("llama")
    {
        128_000
    } else {
        1_000_000
    }
}

/// `1.2M` / `34K` (`formatCompact`).
pub fn compact(n: u64) -> String {
    if n >= 1_000_000 {
        format!(
            "{:.1$}M",
            n as f64 / 1_000_000.0,
            if n >= 10_000_000 { 0 } else { 1 }
        )
    } else if n >= 1_000 {
        format!(
            "{:.1$}K",
            n as f64 / 1_000.0,
            if n >= 100_000 { 0 } else { 1 }
        )
    } else {
        format!("{n}")
    }
}

/// Totals card (`TokenSummary`): cost header, 4-stat grid, cache bar,
/// context footer.
pub fn token_summary(usage: &UsageTotals, model: &str) -> El {
    let input = usage.input;
    let output = usage.output;
    let hits = usage.cache_hit;
    let misses = input.saturating_sub(hits);
    let hit_ratio: f32 = if input > 0 {
        (hits as f32 / input as f32) * 100.0
    } else {
        0.0
    };
    let miss_ratio = if input > 0 { 100.0 - hit_ratio } else { 0.0 };
    let max_ctx = usage.calls.iter().map(|c| c.input).max().unwrap_or(0);
    let limit = context_limit(model);
    let ctx_pct: f32 = if limit > 0 {
        (max_ctx as f32 / limit as f32) * 100.0
    } else {
        0.0
    };
    let stat = |key: &str, label: &str, value: String, sub: String| {
        col()
            .key(format!("stats-{key}"))
            .grow(1.0)
            .min_h(0.0)
            .gap(2.0)
            .child(
                text(label.to_string())
                    .font(rune::tok::TextStyle::new(11.0, 400.0))
                    .color(ink("#71717a")),
            )
            .child(
                text(value)
                    .font(rune::tok::TextStyle::new(15.0, 600.0))
                    .color(ink("#f4f4f5")),
            )
            .child(
                text(sub)
                    .font(rune::tok::TextStyle::new(11.0, 400.0))
                    .color(ink("#71717a")),
            )
    };
    col()
        .key("stats-summary")
        .w_full()
        .radius(14.0)
        .fill(rune::Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.02,
        })
        .border(1.0, ink("#ffffff14"))
        .px(20.0)
        .pt(18.0)
        .pb(16.0)
        .gap(12.0)
        .child(
            row()
                .key("stats-head")
                .w_full()
                .items_center()
                .gap(10.0)
                .child(
                    col()
                        .grow(1.0)
                        .min_h(0.0)
                        .gap(6.0)
                        .child(
                            text("Total cost".to_string())
                                .font(rune::tok::TextStyle::new(11.0, 500.0))
                                .color(ink("#71717a")),
                        )
                        .child(
                            row()
                                .items_center()
                                .gap(8.0)
                                .child(
                                    text(format!("${:.5}", usage.cost))
                                        .key("stats-cost")
                                        .font(rune::tok::TextStyle::new(28.0, 600.0))
                                        .color(ink("#fafafa")),
                                )
                                .child(
                                    text("USD".to_string())
                                        .font(rune::tok::TextStyle::new(11.0, 500.0))
                                        .color(ink("#71717a")),
                                ),
                        ),
                )
                .child(
                    col()
                        .gap(2.0)
                        .child(
                            text(format!(
                                "{} {}",
                                usage.calls.len(),
                                if usage.calls.len() == 1 {
                                    "call"
                                } else {
                                    "calls"
                                }
                            ))
                            .font(rune::tok::TextStyle::new(11.0, 400.0))
                            .color(ink("#71717a")),
                        )
                        .child(
                            text(format!("{} tokens total", input + output))
                                .font(rune::tok::TextStyle::new(11.0, 400.0))
                                .color(ink("#71717a")),
                        ),
                ),
        )
        .child(
            row()
                .key("stats-grid")
                .w_full()
                .gap(10.0)
                .child(stat(
                    "in",
                    "Input tokens",
                    input.to_string(),
                    format!("{} total", compact(input)),
                ))
                .child(stat(
                    "out",
                    "Output tokens",
                    output.to_string(),
                    format!("{} total", compact(output)),
                ))
                .child(stat(
                    "hit",
                    "Cache hit",
                    format!("{hit_ratio:.1}%"),
                    format!("{hits} tokens"),
                ))
                .child(stat(
                    "miss",
                    "Cache miss",
                    format!("{miss_ratio:.1}%"),
                    format!("{misses} tokens"),
                )),
        )
        .child(
            col()
                .key("stats-cachebar")
                .w_full()
                .gap(8.0)
                .child(
                    row()
                        .w_full()
                        .h(5.0)
                        .radius(2.5)
                        .fill(ink("#ffffff12"))
                        .child(
                            rune::el()
                                .h_full()
                                .radius(2.5)
                                .fill(ink("#f4f4f5"))
                                .grow(hit_ratio.max(1.0))
                                .min_h(0.0),
                        )
                        .child(rune::el().grow((100f32 - hit_ratio).max(1.0))),
                )
                .child(
                    row()
                        .w_full()
                        .items_center()
                        .child(
                            text(format!("{hit_ratio:.1}% hits"))
                                .grow(1.0)
                                .font(rune::tok::TextStyle::new(11.0, 400.0))
                                .color(ink("#71717a")),
                        )
                        .child(
                            text(format!("{miss_ratio:.1}% misses"))
                                .font(rune::tok::TextStyle::new(11.0, 400.0))
                                .color(ink("#71717a")),
                        ),
                ),
        )
        .child(
            row()
                .key("stats-context")
                .w_full()
                .items_center()
                .gap(10.0)
                .child(
                    text(format!(
                        "Peak context {max_ctx} / {} ({ctx_pct:.1}%)",
                        compact(limit)
                    ))
                    .grow(1.0)
                    .font(rune::tok::TextStyle::new(11.0, 400.0))
                    .color(ink("#71717a")),
                )
                .child(
                    row()
                        .w(112.0)
                        .h(3.0)
                        .radius(1.5)
                        .fill(ink("#ffffff14"))
                        .child(
                            rune::el()
                                .h_full()
                                .radius(1.5)
                                .fill(ink("#d4d4d8"))
                                .grow(ctx_pct.clamp(1.0, 100.0))
                                .min_h(0.0),
                        )
                        .child(rune::el().grow((100.0 - ctx_pct.clamp(0.0, 100.0)).max(1.0))),
                ),
        )
}

/// Per-request list, newest first (`CallList`).
pub fn call_list(calls: &[UsageCall]) -> El {
    let mut section = col()
        .key("stats-calls")
        .w_full()
        .radius(14.0)
        .fill(rune::Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.02,
        })
        .border(1.0, ink("#ffffff14"))
        .px(18.0)
        .pt(14.0)
        .pb(12.0)
        .gap(10.0)
        .child(
            row()
                .key("stats-calls-head")
                .w_full()
                .items_center()
                .child(
                    text("Requests".to_string())
                        .grow(1.0)
                        .font(rune::tok::TextStyle::new(11.0, 500.0))
                        .color(ink("#71717a")),
                )
                .child(
                    text(format!(
                        "{} {}",
                        calls.len(),
                        if calls.len() == 1 { "call" } else { "calls" }
                    ))
                    .font(rune::tok::TextStyle::new(11.0, 400.0))
                    .color(ink("#71717a")),
                ),
        );
    for (i, c) in calls.iter().rev().enumerate() {
        let hit_pct: f32 = if c.input > 0 {
            (c.hits as f32 / c.input as f32) * 100.0
        } else {
            0.0
        };
        section = section.child(
            col()
                .key(format!("stats-call-{i}"))
                .w_full()
                .gap(7.0)
                .child(
                    row()
                        .w_full()
                        .items_center()
                        .gap(8.0)
                        .child(
                            text(c.agent.clone())
                                .font(rune::tok::TextStyle::new(10.0, 600.0))
                                .px(8.0)
                                .py(2.0)
                                .radius(6.0)
                                .fill(rune::Color {
                                    r: 1.0,
                                    g: 1.0,
                                    b: 1.0,
                                    a: 0.04,
                                })
                                .border(1.0, ink("#ffffff1a"))
                                .color(ink("#d4d4d8")),
                        )
                        .child(
                            text(c.model.clone())
                                .grow(1.0)
                                .font(rune::tok::TextStyle::new(11.0, 400.0).mono())
                                .color(ink("#71717a")),
                        )
                        .child(
                            text(format!("${:.6}", c.cost))
                                .font(rune::tok::TextStyle::new(12.0, 600.0))
                                .color(ink("#f4f4f5")),
                        )
                        .child(
                            text(crate::rune_transcript::clock(&c.at))
                                .font(rune::tok::TextStyle::new(10.0, 400.0))
                                .color(ink("#52525b")),
                        ),
                )
                .child(
                    row()
                        .w_full()
                        .items_center()
                        .gap(6.0)
                        .child(
                            text(format!(
                                "{} cached ({hit_pct:.0}%) · {} uncached",
                                c.hits, c.misses
                            ))
                            .grow(1.0)
                            .font(rune::tok::TextStyle::new(11.0, 400.0))
                            .color(ink("#a1a1aa")),
                        )
                        .child(
                            text(format!("{} out", c.output))
                                .font(rune::tok::TextStyle::new(11.0, 400.0))
                                .color(ink("#a1a1aa")),
                        ),
                )
                .child(
                    row()
                        .w_full()
                        .h(4.0)
                        .radius(2.0)
                        .fill(ink("#ffffff12"))
                        .child(
                            rune::el()
                                .h_full()
                                .radius(2.0)
                                .fill(ink("#e4e4e7"))
                                .grow(hit_pct.max(1.0))
                                .min_h(0.0),
                        )
                        .child(rune::el().grow((100.0 - hit_pct).max(1.0))),
                ),
        );
    }
    section
}

/// Stats body (`TokenStats`): empty state or summary + calls.
pub fn token_stats(usage: &UsageTotals, model: &str) -> El {
    if usage.calls.is_empty() {
        return col()
            .key("stats-empty")
            .w_full()
            .grow(1.0)
            .min_h(200.0)
            .center()
            .gap(10.0)
            .child(
                text("No usage yet".to_string())
                    .font(rune::tok::TextStyle::new(14.0, 600.0))
                    .color(ink("#e4e4e7")),
            )
            .child(
                text(
                    "Run a message to see cost, token usage, cache activity, and context here."
                        .to_string(),
                )
                .font(rune::tok::TextStyle::new(12.0, 400.0))
                .color(ink("#71717a")),
            );
    }
    col()
        .key("stats-body")
        .w_full()
        .gap(16.0)
        .child(token_summary(usage, model))
        .child(call_list(&usage.calls))
}

/// Stats modal (`TokenStatsModal`): Usage head + body, per thread.
pub fn stats_sheet<S: 'static>(
    usage: &UsageTotals,
    model: &str,
    on_close: std::rc::Rc<dyn Fn(&mut S)>,
) -> El {
    rune::el()
        .key("stats-overlay")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(rune::Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.7,
        })
        .center()
        .modal(true, {
            let close = on_close.clone();
            move |s: &mut S| close(s)
        })
        .child(
            col()
                .key("stats-card")
                .w(860.0)
                .radius(16.0)
                .fill(ink("#09090b"))
                .border(1.0, ink("#ffffff14"))
                .px(24.0)
                .pt(20.0)
                .pb(20.0)
                .gap(14.0)
                .child(
                    row()
                        .key("stats-head")
                        .w_full()
                        .items_center()
                        .gap(10.0)
                        .child(
                            col()
                                .grow(1.0)
                                .min_h(0.0)
                                .child(
                                    text("Usage".to_string())
                                        .key("stats-title")
                                        .font(rune::tok::TextStyle::new(14.0, 600.0))
                                        .color(ink("#f4f4f5")),
                                )
                                .child(
                                    text(
                                        "Cost, context, and cache activity for this chat"
                                            .to_string(),
                                    )
                                    .font(rune::tok::TextStyle::new(11.0, 400.0))
                                    .color(ink("#71717a")),
                                ),
                        )
                        .child(
                            text("×".to_string())
                                .key("stats-close")
                                .role("button")
                                .label("Close usage")
                                .font(rune::tok::TextStyle::new(14.0, 400.0))
                                .color(ink("#8a8a93"))
                                .on_press({
                                    let close = on_close.clone();
                                    move |s: &mut S| close(s)
                                }),
                        ),
                )
                .child(token_stats(usage, model)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_limits_match_reference() {
        assert_eq!(context_limit("deepseek-v4.1-flash"), 1_000_000);
        assert_eq!(context_limit("claude-3-5-sonnet"), 200_000);
        assert_eq!(context_limit("gpt-4o"), 128_000);
        assert_eq!(context_limit("mystery"), 1_000_000);
    }

    #[test]
    fn compact_shapes() {
        assert_eq!(compact(999), "999");
        assert_eq!(compact(1500), "1.5K");
        assert_eq!(compact(2_500_000), "2.5M");
    }

    #[test]
    fn modal_mounts_summary_and_calls() {
        use rune::{Env, Runtime};
        struct T {
            usage: UsageTotals,
            model: String,
            open: bool,
        }
        fn view(s: &T, _: &mut rune::Cx<T>) -> rune::El {
            rune::col()
                .key("stats-test-root")
                .w_full()
                .h_full()
                .child(if s.open {
                    stats_sheet::<T>(
                        &s.usage,
                        &s.model,
                        std::rc::Rc::new(|s: &mut T| {
                            s.open = false;
                        }),
                    )
                } else {
                    rune::col().key("stats-test-empty").w_full()
                })
        }
        let mut usage = UsageTotals {
            input: 2000,
            output: 100,
            cache_hit: 500,
            cost: 0.00123,
            active: 2000,
            ..UsageTotals::default()
        };
        usage.calls.push(UsageCall {
            agent: "chat".to_string(),
            model: "m".to_string(),
            input: 2000,
            output: 100,
            hits: 500,
            misses: 1500,
            cost: 0.00123,
            at: "2026-11-09T14:44:00Z".to_string(),
        });
        let mut rt = Runtime::new(
            T {
                usage,
                model: "deepseek-v4.1-flash".to_string(),
                open: true,
            },
            view,
            (900.0, 700.0),
            Env::new(true, 1.0),
        );
        rt.frame(0.0);
        for key in [
            "stats-overlay",
            "stats-card",
            "stats-title",
            "stats-cost",
            "stats-grid",
            "stats-call-0",
        ] {
            assert!(rt.find(key).is_some(), "{key} mounted");
        }
    }
}
