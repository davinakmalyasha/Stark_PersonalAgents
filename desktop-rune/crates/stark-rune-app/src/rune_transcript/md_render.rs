//! Markdown rendering for transcript rows — a direct port of
//! `frontend/src/components/Workspace/Markdown.tsx` and `DiffView.tsx`.
//!
//! `stark-kit::markdown` parses GFM; this module owns every visual value:
//! Tailwind margins, line boxes, one-dark code colours, the code header and
//! copy button, the 800-line diff collapse, tables and task lists. It reuses
//! rune-kit's syntax scanners (the OpenCode component crate) without its
//! theme or block layout.

use rune::{col, row, text, El, ScrollSpec, TextStyle};

use super::{home_icon_copy, Thread, TranscriptHost};

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

/// `rgba()` in sRGB, stored as Rune's linear RGB.
fn rgba(r: f32, g: f32, b: f32, a: f32) -> rune::Color {
    fn linear(c: f32) -> f32 {
        let c = c / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    rune::Color {
        r: linear(r),
        g: linear(g),
        b: linear(b),
        a,
    }
}

/// Prism `one-dark` token colours, mapped onto rune-kit's scanner kinds.
fn syntax_theme() -> rune_kit::syntax::SyntaxTheme {
    use rune_kit::syntax::{Syntax, SyntaxTheme};
    SyntaxTheme::neutral(true)
        .with(Syntax::Keyword, ink("#c678dd"))
        .with(Syntax::String, ink("#98c379"))
        .with(Syntax::Number, ink("#d19a66"))
        .with(Syntax::Comment, ink("#5c6370"))
        .with(Syntax::Type, ink("#d19a66"))
        .with(Syntax::Function, ink("#61afef"))
        .with(Syntax::Property, ink("#e06c75"))
        .with(Syntax::Punctuation, ink("#abb2bf"))
        .with(Syntax::Inserted, ink("#98c379"))
        .with(Syntax::Deleted, ink("#e06c75"))
        .with(Syntax::Parameter, ink("#abb2bf"))
}

/// Inline runs flattened to `rich` text + spans + link/strike maps.
#[derive(Default)]
struct Runs {
    text: String,
    spans: Vec<rune::Span>,
    links: Vec<rune::Link>,
    strikes: Vec<(usize, usize, Option<rune::Color>)>,
    /// Byte ranges of inline-code runs: each carries a 6px pad and 1px border
    /// in React, which `content_widths` alone cannot see.
    codes: Vec<(usize, usize)>,
    /// Paint-only boxes for the inline-code runs (fill + 1px inset ring),
    /// emitted into `rich` via `text_highlights`.
    highlights: Vec<rune::TextHighlight>,
}

const INLINE_CODE_SIZE: f32 = 13.5;

/// Inline code: 13.5pt mono indigo-300 on a zinc-800/80 well with the
/// `border-zinc-700/50` 1px ring (React `code` component). The well comes
/// from a paint-only highlight so the border rides the same rounded rect.
fn inline_code_span(start: usize, end: usize) -> (rune::Span, rune::TextHighlight) {
    (
        rune::Span::new(start, end)
            .mono()
            .size(INLINE_CODE_SIZE)
            .color(ink("#a5b4fc")),
        rune::TextHighlight::new(start..end, rgba(39.0, 39.0, 42.0, 0.8))
            .ring(rgba(63.0, 63.0, 70.0, 0.5), 1.0)
            .pad(6.0),
    )
}

fn inlines(runs: &mut Runs, list: &[stark_kit::markdown::Inline], italic: bool) {
    use stark_kit::markdown::Inline;
    for inline in list {
        match inline {
            Inline::Text(t) => {
                let start = runs.text.len();
                runs.text.push_str(t);
                if italic {
                    runs.spans
                        .push(rune::Span::new(start, runs.text.len()).italic());
                }
            }
            Inline::Break => runs.text.push('\n'),
            Inline::Code(t) => {
                let start = runs.text.len();
                runs.text.push_str(t);
                let (span, highlight) = inline_code_span(start, runs.text.len());
                runs.spans.push(span);
                runs.highlights.push(highlight);
                runs.codes.push((start, runs.text.len()));
            }
            Inline::Bold(inner) => {
                let start = runs.text.len();
                let at = runs.spans.len();
                runs.spans.push(
                    rune::Span::new(start, start)
                        .weight(600.0)
                        .color(ink("#fafafa")),
                );
                inlines(runs, inner, italic);
                if runs.text.len() > start {
                    runs.spans[at].start = start as u32;
                    runs.spans[at].end = runs.text.len() as u32;
                } else {
                    runs.spans.remove(at);
                }
            }
            Inline::Italic(inner) => inlines(runs, inner, true),
            Inline::Strike(inner) => {
                let start = runs.text.len();
                inlines(runs, inner, italic);
                if runs.text.len() > start {
                    runs.strikes
                        .push((start, runs.text.len(), Some(ink("#71717a"))));
                }
            }
            Inline::Link { href, text } => {
                let start = runs.text.len();
                let at = runs.spans.len();
                runs.spans.push(
                    rune::Span::new(start, start)
                        .color(ink("#60a5fa"))
                        .underline(rgba(59.0, 130.0, 246.0, 0.4)),
                );
                inlines(runs, text, italic);
                if runs.text.len() > start {
                    runs.spans[at].start = start as u32;
                    runs.spans[at].end = runs.text.len() as u32;
                    runs.links
                        .push(rune::Link::new(start..runs.text.len(), href.clone()));
                } else {
                    runs.spans.remove(at);
                }
            }
            Inline::Image { alt, .. } => {
                let start = runs.text.len();
                runs.text
                    .push_str(if alt.is_empty() { "image" } else { alt });
                if runs.text.len() > start {
                    runs.spans.push(
                        rune::Span::new(start, runs.text.len())
                            .color(ink("#a1a1aa"))
                            .italic(),
                    );
                }
            }
        }
    }
}

fn rich<S: TranscriptHost + 'static>(
    key: String,
    runs: Runs,
    style: TextStyle,
    color: rune::Color,
) -> El {
    let mut e = rune::rich(runs.text, runs.spans)
        .key(key)
        .font(style)
        .color(color)
        .exact_lines()
        .selectable(true);
    for (start, end, c) in runs.strikes {
        e = e.strike(start, end, c);
    }
    if !runs.highlights.is_empty() {
        e = e.text_highlights(runs.highlights);
    }
    let has_links = !runs.links.is_empty();
    for link in runs.links {
        e = e.link(link);
    }
    if has_links {
        e = e.on_link(|_: &mut S, id: &str, tx: &mut rune::Tx| {
            let _ = tx.open_url(id);
        });
    }
    e
}

/// React `Markdown.tsx` block margins (Tailwind `my-*`, collapsed like CSS).
fn margins(block: &stark_kit::markdown::Block) -> (f32, f32) {
    use stark_kit::markdown::Block;
    match block {
        Block::Para(_) => (12.0, 12.0),
        Block::Heading { level: 1, .. } => (24.0, 12.0),
        Block::Heading { level: 2, .. } => (20.0, 10.0),
        Block::Heading { level: 3, .. } => (16.0, 8.0),
        Block::Heading { .. } => (12.0, 6.0),
        Block::Code { .. } | Block::Table { .. } => (20.0, 20.0),
        Block::List { .. } => (10.0, 10.0),
        Block::Quote(_) => (16.0, 16.0),
        Block::Rule => (24.0, 24.0),
        Block::Image { .. } => (8.0, 8.0),
    }
}

/// `.markdown-compact` overrides (activity notes): 4pt paragraphs/lists and
/// tighter headings; code/table keep the base block spacing.
fn margins_compact(block: &stark_kit::markdown::Block, compact: bool) -> (f32, f32) {
    use stark_kit::markdown::Block;
    if !compact {
        return margins(block);
    }
    match block {
        Block::Para(_) => (4.0, 4.0),
        Block::Heading { .. } => (8.0, 4.0),
        Block::List { .. } => (4.0, 4.0),
        other => margins(other),
    }
}

/// Bottom margin React keeps inside an agent response after its final block
/// (`Markdown.tsx` block `mb-*`); the response box ends there, not at the
/// last glyph.
pub(super) fn last_margin(body: &str) -> f32 {
    parse_cached(body)
        .last()
        .map(|block| margins(block).1)
        .unwrap_or(0.0)
}

fn heading_style(level: u8) -> TextStyle {
    let (size, weight) = match level {
        1 => (21.0, 600.0),
        2 => (17.5, 600.0),
        3 => (15.0, 500.0),
        4 => (14.5, 500.0),
        5 => (12.9, 600.0),
        _ => (10.4, 600.0),
    };
    let mut style = TextStyle::new(size, weight);
    style.line_height = size * 1.85;
    style
}

fn heading_color(level: u8) -> rune::Color {
    match level {
        1 => ink("#f4f4f5"),
        2 => ink("#e4e4e7"),
        _ => ink("#d4d4d8"),
    }
}

#[allow(clippy::too_many_arguments)]
fn block<S: TranscriptHost + 'static>(
    s: &S,
    which: Thread,
    key: &str,
    block: &stark_kit::markdown::Block,
    body: TextStyle,
    depth: usize,
    compact: bool,
    body_color: rune::Color,
) -> El {
    use stark_kit::markdown::Block;
    match block {
        Block::Para(list) => {
            let mut runs = Runs::default();
            inlines(&mut runs, list, false);
            rich::<S>(format!("{key}-p"), runs, body, body_color)
        }
        Block::Heading { level, text: list } => {
            let mut runs = Runs::default();
            inlines(&mut runs, list, false);
            let style = if compact {
                let mut style = TextStyle::new(13.0, 600.0);
                style.line_height = 13.0 * 1.7;
                style
            } else {
                heading_style(*level)
            };
            let mut e = rich::<S>(format!("{key}-h"), runs, style, heading_color(*level));
            if *level == 2 {
                e = e
                    .pb(6.0)
                    .border_sides([0.0, 0.0, 1.0, 0.0], rgba(39.0, 39.0, 42.0, 0.4))
                    .border_layout();
            }
            e
        }
        Block::List {
            ordered,
            start,
            items,
        } => {
            let mut list =
                col()
                    .key(format!("{key}-list"))
                    .w_full()
                    .gap(if *ordered { 6.0 } else { 4.0 });
            for (i, item) in items.iter().enumerate() {
                let content = blocks_inner::<S>(
                    s,
                    which,
                    &format!("{key}-li-{i}"),
                    &item.blocks,
                    body,
                    depth + 1,
                    true,
                    compact,
                    body_color,
                    false,
                    item.tight,
                )
                .grow(1.0)
                .min_w(0.0);
                let marker = list_marker(*ordered, i, *start, body, body_color);
                let mut row = row()
                    .key(format!("{key}-li-{i}"))
                    .w_full()
                    .items_start()
                    .pl(8.0);
                if let Some(checked) = item.checked {
                    let boxed = rune::row()
                        .items_start()
                        .gap(6.0)
                        .child(checkbox(checked, body.line_height))
                        .child(content);
                    row = row.gap(10.0).child(marker).child(boxed);
                } else {
                    row = row
                        .gap(if *ordered { 8.0 } else { 10.0 })
                        .child(marker)
                        .child(content);
                }
                list = list.child(row);
            }
            list
        }
        Block::Code { lang, text: code } => code_block::<S>(s, which, key, lang, code),
        Block::Table {
            headers,
            rows,
            align: _,
        } => table::<S>(key, headers, rows, body),
        Block::Quote(list) => {
            // React's blockquote is fully italic (`italic` class on the
            // container); the base style carries it so nested spans inherit.
            // Padded well (`py-8`): child margins cannot collapse out, so
            // the first block keeps its full top and the last block's bottom
            // margin lands as an explicit spacer (like `last_margin`).
            let inner = blocks_inner::<S>(
                s,
                which,
                &format!("{key}-quote"),
                list,
                body.italic(),
                depth + 1,
                false,
                compact,
                body_color,
                true,
                false,
            );
            let bottom = list
                .last()
                .map(|b| margins_compact(b, compact).1)
                .unwrap_or(0.0);
            // The trailing margin hangs below the border (it collapses out
            // of the unpadded blockquote), so it sits outside the fill.
            col()
                .key(format!("{key}-quote-wrap"))
                .w_full()
                .child(
                    row()
                        .key(format!("{key}-quote"))
                        .w_full()
                        .items_start()
                        .pl(16.0)
                        .py(8.0)
                        .radius(8.0)
                        .fill(rgba(99.0, 102.0, 241.0, 0.05))
                        .border_sides([2.0, 0.0, 0.0, 0.0], rgba(99.0, 102.0, 241.0, 0.5))
                        .border_layout()
                        .child(inner.grow(1.0).min_w(0.0)),
                )
                .child(
                    rune::el()
                        .key(format!("{key}-quote-pad"))
                        .h(bottom)
                        .w_full(),
                )
        }
        Block::Rule => rune::el()
            .key(format!("{key}-rule"))
            .w_full()
            .h(1.0)
            .fill(ink("#27272a")),
        Block::Image { src, alt } => col()
            .key(format!("{key}-img"))
            .w_full()
            .max_h(320.0)
            .radius(8.0)
            .border(1.0, ink("#3f3f46"))
            .fill(ink("#18181b"))
            .center()
            .py(24.0)
            .child(
                text(if alt.is_empty() {
                    src.clone()
                } else {
                    alt.clone()
                })
                .font(TextStyle::new(12.5, 400.0))
                .color(ink("#a1a1aa")),
            ),
    }
}

/// A block sequence with CSS-style collapsed margins.
///
/// `pad_parent` is set when the parent carries its own vertical padding
/// (the quote well's `py-8`): child margins cannot collapse out through
/// padding, so the first block keeps its full top margin and the caller
/// appends the last block's bottom margin (see the Quote arm). The
/// unpadded response instead relies on its own 12pt top pad, keeping only
/// the remainder above the first block.
/// `tight_item` marks a tight list item: React renders no `<p>` wrapper,
/// so phantom paragraphs carry no margins and a nested list keeps its own
/// `my-2.5` instead of collapsing with paragraph margins.
#[allow(clippy::too_many_arguments)]
fn blocks_inner<S: TranscriptHost + 'static>(
    s: &S,
    which: Thread,
    key: &str,
    list: &[stark_kit::markdown::Block],
    body: TextStyle,
    depth: usize,
    in_item: bool,
    compact: bool,
    body_color: rune::Color,
    pad_parent: bool,
    tight_item: bool,
) -> El {
    let _ = depth;
    let mut stack = col().key(format!("{key}-blocks")).w_full();
    let mut prev_mb = 0.0f32;
    let mut after_heading = false;
    for (i, b) in list.iter().enumerate() {
        let (mt, mb) = if in_item {
            match b {
                stark_kit::markdown::Block::Para(_) => {
                    if tight_item && !compact {
                        (0.0, 0.0)
                    } else if compact {
                        (if i == 0 { 0.0 } else { 4.0 }, 4.0)
                    } else {
                        (if i == 0 { 0.0 } else { 12.0 }, 12.0)
                    }
                }
                stark_kit::markdown::Block::List { .. } => {
                    if compact {
                        (4.0, 4.0)
                    } else {
                        // Nested `ul` keeps `my-2.5`; with a tight item there
                        // is no paragraph margin to collapse with.
                        margins_compact(b, compact)
                    }
                }
                other => margins_compact(other, compact),
            }
        } else {
            margins_compact(b, compact)
        };
        let top = if i == 0 {
            if pad_parent {
                mt
            } else {
                // First block under the unpadded response: its margin
                // collapses out, and the response's own 12pt top pad covers
                // the remainder (para/h4/lists stay 0, h1 keeps 12).
                (mt - 12.0).max(0.0)
            }
        } else if matches!(b, stark_kit::markdown::Block::Heading { .. }) {
            mt.max(prev_mb)
        } else if after_heading {
            prev_mb
        } else {
            mt.max(prev_mb)
        };
        let e = block::<S>(
            s,
            which,
            &format!("{key}-b{i}"),
            b,
            body,
            depth + 1,
            compact,
            body_color,
        );
        stack = stack.child(e.mt(top));
        prev_mb = mb;
        after_heading = matches!(b, stark_kit::markdown::Block::Heading { .. });
    }
    stack
}

/// Render a message body as markdown blocks.
pub(super) fn render_body<S: TranscriptHost + 'static>(
    s: &S,
    which: Thread,
    key: &str,
    body: &str,
    body_ink: &'static str,
) -> El {
    render_body_styled(s, which, key, body, body_ink, TextStyle::new(12.5, 400.0))
}

/// Markdown renderer at a caller-chosen type scale (home rows are 15.5pt,
/// the sheet keeps 12.5pt).
pub(super) fn render_body_styled<S: TranscriptHost + 'static>(
    s: &S,
    which: Thread,
    key: &str,
    body: &str,
    _body_ink: &'static str,
    body_style: TextStyle,
) -> El {
    let blocks = parse_cached(body);
    if blocks.is_empty() {
        return col().key(format!("{key}-md"));
    }
    let body_color = ink(_body_ink);
    blocks_inner::<S>(
        s, which, key, &blocks, body_style, 0, false, false, body_color, false, false,
    )
}

/// `.markdown-compact`: activity notes (12.5pt / 1.7, 4pt paragraphs).
pub(super) fn render_body_compact<S: TranscriptHost + 'static>(
    s: &S,
    which: Thread,
    key: &str,
    body: &str,
    _body_ink: &'static str,
    size: f32,
) -> El {
    let blocks = parse_cached(body);
    if blocks.is_empty() {
        return col().key(format!("{key}-md"));
    }
    let mut style = TextStyle::new(size, 400.0);
    style.line_height = size * 1.7;
    let body_color = ink(_body_ink);
    blocks_inner::<S>(
        s, which, key, &blocks, style, 0, false, true, body_color, false, false,
    )
}

/// `StreamingMarkdown` from `MessageBubble.tsx`: parse the stable prefix,
/// keep an unfinished fence as a real code card, and drop a dangling inline
/// opener so it cannot swallow the rest of the message.
pub(super) fn render_body_streaming<S: TranscriptHost + 'static>(
    s: &S,
    which: Thread,
    key: &str,
    body: &str,
    body_ink: &'static str,
    body_style: TextStyle,
) -> El {
    if body.is_empty() {
        return col().key(format!("{key}-stream"));
    }
    let split = stream_split(body);
    let stable = &body[..split];
    let trailing = &body[split..];
    if trailing.is_empty() {
        return render_body_styled(s, which, key, stable, body_ink, body_style);
    }
    let mut out = col().key(format!("{key}-stream")).w_full();
    if !stable.is_empty() {
        out = out.child(render_body_styled::<S>(
            s, which, key, stable, body_ink, body_style,
        ));
    }
    if let Some((lang, code)) = parse_open_fence(trailing) {
        out = out.child(open_fence_card(key, &lang, &code));
        return out;
    }
    // Unclosed inline code / bold: drop the dangling opener when the stable
    // prefix already opened one.
    let safe_trailing = strip_dangling_marker(trailing, stable);
    out.child(
        text(safe_trailing)
            .key(format!("{key}-stream-tail"))
            .font(body_style)
            .color(ink(body_ink)),
    )
}

/// Index where the stable prefix ends (React's `StreamingMarkdown` split).
fn stream_split(body: &str) -> usize {
    let fences: Vec<usize> = body.match_indices("```").map(|(at, _)| at).collect();
    if fences.len() % 2 == 1 {
        let open = *fences.last().unwrap_or(&0);
        body[..open].rfind('\n').map_or(open, |at| at + 1)
    } else {
        body.len()
    }
}

/// Drop a trailing `**`/`~~`/`` ` `` opener only when the stable prefix has
/// already opened that construct.
fn strip_dangling_marker(trailing: &str, stable: &str) -> String {
    let mut safe = trailing.to_string();
    for marker in ["**", "~~", "`"] {
        let trimmed = safe.trim_end();
        if trimmed.ends_with(marker) && stable.contains(marker) {
            let end = trimmed.len() - marker.len();
            safe = safe[..end].to_string();
            break;
        }
    }
    safe
}

fn parse_open_fence(trailing: &str) -> Option<(String, String)> {
    let rest = trailing.strip_prefix("```")?;
    let newline = rest.find('\n')?;
    let lang = &rest[..newline];
    if !lang
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '+' || c == '-')
    {
        return None;
    }
    Some((lang.to_string(), rest[newline + 1..].to_string()))
}

fn open_fence_card(key: &str, lang: &str, code: &str) -> El {
    let mut header = row()
        .key(format!("{key}-stream-head"))
        .items_center()
        .px(12.0)
        .py(4.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], rgba(39.0, 39.0, 42.0, 0.6))
        .border_layout()
        .fill(rgba(24.0, 24.0, 27.0, 0.7));
    header = header.child(
        text(if lang.is_empty() { "text" } else { lang })
            .font({
                let mut style = TextStyle::new(10.0, 400.0).mono();
                style.line_height = 15.0;
                style.tracking = 0.5;
                style
            })
            .color(ink("#71717a")),
    );
    col()
        .key(format!("{key}-stream-fence"))
        .w_full()
        .my(16.0)
        .radius(12.0)
        .border(1.0, rgba(39.0, 39.0, 42.0, 0.6))
        .border_layout()
        .fill(ink("#09090b"))
        .child(header)
        .child(
            text(code.to_string())
                .key(format!("{key}-stream-code"))
                .font({
                    let mut style = TextStyle::new(13.0, 400.0).mono();
                    style.line_height = 19.5;
                    style
                })
                .color(ink("#d4d4d8"))
                .px(16.0)
                .py(16.0),
        )
}

/// Bounded parse cache for unchanged markdown (completed rows and repeated
/// frames). Live rows change every delta and fall out naturally.
fn parse_cached(body: &str) -> std::rc::Rc<Vec<stark_kit::markdown::Block>> {
    use std::cell::RefCell;
    use std::rc::Rc;
    thread_local! {
        static CACHE: RefCell<Vec<(String, Rc<Vec<stark_kit::markdown::Block>>)>> =
            const { RefCell::new(Vec::new()) };
    }
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((_, blocks)) = cache.iter().find(|(text, _)| text == body) {
            return blocks.clone();
        }
        let blocks = Rc::new(stark_kit::markdown::parse(body));
        if body.len() <= 32_768 {
            cache.push((body.to_string(), blocks.clone()));
            if cache.len() > 8 {
                cache.remove(0);
            }
        }
        blocks
    })
}

/// A list marker: `•` in a 6pt box or `1.` right-aligned in 12pt.
fn list_marker(
    ordered: bool,
    index: usize,
    start: u64,
    body: TextStyle,
    body_color: rune::Color,
) -> El {
    if ordered {
        row()
            .w(12.0)
            .h(body.line_height)
            .justify_end()
            .items_center()
            .child(
                text(format!("{}.", start + index as u64))
                    .font(body)
                    .color(body_color),
            )
    } else {
        row()
            .w(6.0)
            .h(body.line_height)
            .center()
            .child(rune::el().size(4.0, 4.0).radius(2.0).fill(body_color))
    }
}

/// GFM task checkbox (`input[type=checkbox]`, 13pt, indigo accent).
/// GFM task checkbox (`input[type=checkbox]`, 13pt). The Markdown renderer
/// hardcodes `disabled`, and disabled controls render grayed out: the
/// checked fill reads neutral gray (the `accent-indigo-500` class is
/// overridden by the disabled graying) with a white check. Vertical
/// centering follows `align-middle`.
fn checkbox(checked: bool, line_height: f32) -> El {
    let mut e = rune::el()
        .size(13.0, 13.0)
        .mt(((line_height - 13.0) / 2.0).max(0.0))
        .radius(2.0)
        .border(
            1.0,
            if checked {
                ink("#9a9a9b")
            } else {
                ink("#a9a9aa")
            },
        )
        .fill(if checked {
            ink("#9a9a9b")
        } else {
            rune::Color::TRANSPARENT
        });
    if checked {
        let p = rune::Path::new()
            .move_to([3.2, 7.0])
            .line_to([5.6, 9.4])
            .line_to([9.9, 4.2]);
        e = e.child(
            rune::el().size(13.0, 13.0).inert().shape(
                rune::Shape::new()
                    .stroke_path(&p, 2.0, rune::LineCap::Round, ink("#ffffff"))
                    .view_box(13.0, 13.0),
            ),
        );
    }
    e
}

fn icon_check(size: f32, color: rune::Color) -> rune::Shape {
    let p = rune::Path::new()
        .move_to([20.0, 6.0])
        .line_to([9.0, 17.0])
        .line_to([4.0, 12.0]);
    rune::Shape::new()
        .stroke_path(&p, crate::lucide_stroke(size), rune::LineCap::Round, color)
        .view_box(24.0, 24.0)
}

fn icon_chevron(down: bool, size: f32, color: rune::Color) -> rune::Shape {
    let p = if down {
        rune::Path::new()
            .move_to([6.0, 9.0])
            .line_to([12.0, 15.0])
            .line_to([18.0, 9.0])
    } else {
        rune::Path::new()
            .move_to([9.0, 6.0])
            .line_to([15.0, 12.0])
            .line_to([9.0, 18.0])
    };
    rune::Shape::new()
        .stroke_path(&p, crate::lucide_stroke(size), rune::LineCap::Round, color)
        .view_box(24.0, 24.0)
}

fn code_body_style() -> TextStyle {
    let mut style = TextStyle::new(13.5, 400.0).mono();
    style.line_height = 21.6;
    style
}

fn looks_like_diff(lang: &str, text: &str) -> bool {
    lang == "diff"
        || lang == "patch"
        || text.starts_with("diff --git")
        || text.starts_with("--- ")
        || text.contains("\n@@ ")
}

/// The primitive types Prism's Rust grammar colours as keywords.
fn is_rust_primitive(text: &str) -> bool {
    matches!(
        text,
        "bool"
            | "char"
            | "str"
            | "f32"
            | "f64"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
    )
}

/// Prism's Rust `operator` pattern, minus `->`, which stays punctuation.
fn rust_operator(code: &str, start: usize, end: usize) -> bool {
    let text = &code[start..end];
    if text.len() != 1 {
        return false;
    }
    let ch = text.as_bytes()[0];
    if ch == b'-' && code[end..].starts_with('>') {
        return false;
    }
    if ch == b'>' && code[..start].ends_with('-') {
        return false;
    }
    matches!(
        ch,
        b'-' | b'+'
            | b'*'
            | b'/'
            | b'%'
            | b'!'
            | b'^'
            | b'='
            | b'&'
            | b'|'
            | b'<'
            | b'>'
            | b'@'
            | b'?'
    )
}

/// React's code block: header (language + copy), then a horizontal code well.
fn code_block<S: TranscriptHost + 'static>(
    s: &S,
    which: Thread,
    key: &str,
    lang: &str,
    code: &str,
) -> El {
    if looks_like_diff(lang, code) {
        return diff::<S>(s, which, key, code);
    }
    let highlighted = !lang.is_empty() && code.chars().count() <= 40_000;
    // React's `code` renderer does `String(children).replace(/\n$/, '')`; the
    // markdown parser keeps that newline, and Rune would lay out an empty
    // sixteenth line for it.
    let code = code.strip_suffix('\n').unwrap_or(code);
    let base = if highlighted {
        ink("#abb2bf")
    } else {
        ink("#d4d4d8")
    };
    let label = if lang.is_empty() {
        "text".to_string()
    } else {
        lang.to_uppercase()
    };
    let label_style = {
        let mut style = TextStyle::new(10.0, 400.0).mono();
        style.line_height = 18.5;
        style.tracking = 0.5;
        style
    };
    let header = row()
        .key(format!("{key}-code-head"))
        .w_full()
        .items_center()
        .px(12.0)
        .py(4.0)
        .corners([12.0, 12.0, 0.0, 0.0])
        .fill(rgba(24.0, 24.0, 27.0, 0.7))
        // `.border-b-0`: the body's own top border is the separator.
        .border_sides([1.0, 1.0, 0.0, 1.0], rgba(39.0, 39.0, 42.0, 0.6))
        .border_layout()
        .child(text(label).font(label_style).color(ink("#71717a")));
    let mut spans = Vec::new();
    if highlighted {
        use rune_kit::syntax::{Paint, Syntax};
        let theme = syntax_theme();
        let rust = matches!(lang.to_ascii_lowercase().as_str(), "rust" | "rs");
        for (range, paint) in rune_kit::syntax::highlight(Some(lang), code).iter() {
            let mut paint = *paint;
            if rust {
                // Prism's Rust grammar folds primitive types into `keyword`, paints
                // operators with the function blue and macros with the property rose.
                let text = &code[range.clone()];
                if paint == Paint::Kind(Syntax::Punctuation) {
                    // The scanner groups adjacent separators (`&[`, `])`); Prism
                    // splits operators from punctuation, so emit one span per char.
                    let mut at = range.start;
                    for ch in text.chars() {
                        let end = at + ch.len_utf8();
                        let kind = if rust_operator(code, at, end) {
                            Syntax::Function
                        } else {
                            Syntax::Punctuation
                        };
                        spans.push(rune::Span::new(at, end).color(theme.color(kind)));
                        at = end;
                    }
                    continue;
                }
                paint = match paint {
                    Paint::Kind(Syntax::Type) if is_rust_primitive(text) => {
                        Paint::Kind(Syntax::Keyword)
                    }
                    Paint::Kind(Syntax::Function) if text.ends_with('!') => {
                        Paint::Kind(Syntax::Property)
                    }
                    other => other,
                };
            }
            let color = match paint {
                Paint::Kind(kind) => theme.color(kind),
                Paint::Color(c) => c,
            };
            let mut span = rune::Span::new(range.start, range.end).color(color);
            if paint == Paint::Kind(Syntax::Comment) {
                span = span.italic();
            }
            spans.push(span);
        }
    }
    let code_text = rune::rich(code.to_string(), spans)
        .key(format!("{key}-code-body"))
        .font(code_body_style())
        .color(base)
        .exact_lines()
        .selectable(true)
        .w_full();
    let body = rune::el()
        .key(format!("{key}-code-well"))
        .w_full()
        .scroll_x()
        .fill(ink("#09090b"))
        .border(1.0, rgba(39.0, 39.0, 42.0, 0.6))
        .border_layout()
        .corners([0.0, 0.0, 12.0, 12.0])
        .child(col().px(16.0).pt(17.0).pb(15.0).child(code_text));
    let code_owned = code.to_string();
    let copy_key = format!("{key}-copy");
    let copied = s.thread(which).copied_code.as_deref() == Some(copy_key.as_str());
    let icon = if copied {
        icon_check(13.0, ink("#34d399"))
    } else {
        home_icon_copy(13.0, ink("#a1a1aa"))
    };
    let copy = rune::el()
        .key(copy_key.clone())
        .role("button")
        .label("Copy code")
        .size(25.0, 25.0)
        .radius(6.0)
        .center()
        .fill(rgba(39.0, 39.0, 42.0, 0.8))
        .hover(rgba(63.0, 63.0, 70.0, 0.9))
        .pin(None, Some(8.0), Some(8.0), None)
        .child(rune::el().size(13.0, 13.0).inert().shape(icon))
        .on_press_tx(move |s: &mut S, tx: &mut rune::Tx| {
            let t = s.thread_mut(which);
            t.copied_code = Some(copy_key.clone());
            t.copied_at = Some(std::time::Instant::now());
            tx.copy(code_owned.clone());
            tx.wake_after(std::time::Duration::from_millis(1800));
        });
    col()
        .key(format!("{key}-code"))
        .w_full()
        .child(header)
        .child(body)
        .child(copy)
}

/// Port of `DiffView.tsx`'s `parseDiff`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DiffKind {
    Add,
    Del,
    Ctx,
    Hunk,
    Meta,
}

struct DiffLine {
    kind: DiffKind,
    old: Option<u32>,
    new: Option<u32>,
    text: String,
}

fn parse_diff(raw: &str) -> (Vec<DiffLine>, String, usize, usize) {
    let mut lines = Vec::new();
    let mut header = String::new();
    let mut add = 0usize;
    let mut del = 0usize;
    let mut old_no = 0u32;
    let mut new_no = 0u32;
    for raw_line in raw.split('\n') {
        if raw_line.starts_with("diff --git")
            || raw_line.starts_with("index ")
            || raw_line.starts_with("--- ")
            || raw_line.starts_with("+++ ")
        {
            if raw_line.starts_with("--- a/") || raw_line.starts_with("+++ b/") {
                let name = &raw_line[6..];
                if !name.is_empty() && name != "/dev/null" {
                    header = name.to_string();
                }
            }
            lines.push(DiffLine {
                kind: DiffKind::Meta,
                old: None,
                new: None,
                text: raw_line.to_string(),
            });
            continue;
        }
        if let Some((old, new)) = parse_hunk(raw_line) {
            old_no = old;
            new_no = new;
            lines.push(DiffLine {
                kind: DiffKind::Hunk,
                old: None,
                new: None,
                text: raw_line.to_string(),
            });
            continue;
        }
        if let Some(text) = raw_line.strip_prefix('+') {
            lines.push(DiffLine {
                kind: DiffKind::Add,
                old: None,
                new: Some(new_no),
                text: format!("+{text}"),
            });
            new_no += 1;
            add += 1;
        } else if let Some(text) = raw_line.strip_prefix('-') {
            lines.push(DiffLine {
                kind: DiffKind::Del,
                old: Some(old_no),
                new: None,
                text: format!("-{text}"),
            });
            old_no += 1;
            del += 1;
        } else if raw_line.starts_with('\\') {
            lines.push(DiffLine {
                kind: DiffKind::Ctx,
                old: None,
                new: None,
                text: raw_line.to_string(),
            });
        } else {
            lines.push(DiffLine {
                kind: DiffKind::Ctx,
                old: Some(old_no),
                new: Some(new_no),
                text: raw_line.to_string(),
            });
            old_no += 1;
            new_no += 1;
        }
    }
    (lines, header, add, del)
}

fn parse_hunk(line: &str) -> Option<(u32, u32)> {
    let rest = line.strip_prefix("@@ -")?;
    let old_end = rest.find([',', ' '])?;
    let old: u32 = rest[..old_end].parse().ok()?;
    let after_old = &rest[old_end..];
    let plus = after_old.find(" +")?;
    let new_rest = &after_old[plus + 2..];
    let new_end = new_rest.find([',', ' '])?;
    let new: u32 = new_rest[..new_end].parse().ok()?;
    Some((old, new))
}

fn line_style(kind: DiffKind) -> (rune::Color, Option<rune::Color>) {
    match kind {
        DiffKind::Add => (
            rgba(16.0, 185.0, 129.0, 0.08),
            Some(rgba(167.0, 243.0, 208.0, 0.9)),
        ),
        DiffKind::Del => (
            rgba(239.0, 68.0, 68.0, 0.08),
            Some(rgba(252.0, 165.0, 165.0, 0.9)),
        ),
        DiffKind::Ctx => (rune::Color::TRANSPARENT, Some(ink("#a1a1aa"))),
        DiffKind::Hunk => (
            rgba(99.0, 102.0, 241.0, 0.06),
            Some(rgba(165.0, 180.0, 252.0, 0.9)),
        ),
        DiffKind::Meta => (rune::Color::TRANSPARENT, Some(ink("#52525b"))),
    }
}

/// Render a raw unified diff with the same `DiffView` parity as a fenced
/// `diff` code block (used by the activity tool panel).
pub(super) fn render_diff<S: TranscriptHost + 'static>(
    s: &S,
    which: Thread,
    key: &str,
    raw: &str,
) -> El {
    diff(s, which, key, raw)
}

/// Port of `DiffView.tsx`: header, numbered lines, 800-line collapse.
fn diff<S: TranscriptHost + 'static>(s: &S, which: Thread, key: &str, raw: &str) -> El {
    const MAX_VISIBLE: usize = 800;
    let (lines, header, add, del) = parse_diff(raw);
    if lines.is_empty() {
        return text("No text diff available for this file.")
            .font(TextStyle::new(12.5, 400.0))
            .color(ink("#52525b"));
    }
    let toggle_key = format!("{key}-diff-all");
    let expanded = s.thread(which).expanded_diffs.contains(&toggle_key);
    let truncated = lines.len() > MAX_VISIBLE && !expanded;
    let visible = if truncated {
        &lines[..MAX_VISIBLE]
    } else {
        &lines[..]
    };
    let mono = {
        let mut style = TextStyle::new(12.0, 400.0).mono();
        style.line_height = 18.6;
        style
    };
    // The header inherits `.markdown-body`'s 1.85 line-height rather than the
    // table's 1.55, so its filename and stat rows are taller than the rows.
    let head_name = {
        let mut style = TextStyle::new(12.0, 400.0).mono();
        style.line_height = 12.0 * 1.85;
        style
    };
    let head_stats = {
        let mut style = TextStyle::new(11.0, 600.0).mono();
        style.line_height = 11.0 * 1.85;
        style
    };
    let mut body = col().key(format!("{key}-diff-lines")).w_full();
    for (i, line) in visible.iter().enumerate() {
        let (bg, fg) = line_style(line.kind);
        let fg = fg.unwrap_or(ink("#a1a1aa"));
        let line_no = |n: Option<u32>| {
            row()
                .w(42.0)
                .justify_end()
                .px(10.0)
                .border_sides([0.0, 1.0, 0.0, 0.0], rgba(39.0, 39.0, 42.0, 0.5))
                .border_layout()
                .child(
                    text(n.map(|n| n.to_string()).unwrap_or_default())
                        .font(mono)
                        .color(ink("#52525b"))
                        // Chrome sits the glyphs one device pixel lower in this line
                        // box than Rune; shift the paint, not the row.
                        .offset(0.0, 1.0),
                )
        };
        let content = text(if line.text.is_empty() {
            " ".to_string()
        } else {
            line.text.clone()
        })
        .font(mono)
        .color(fg)
        .pl(10.0)
        .pr(10.0)
        .grow(1.0)
        .min_w(0.0)
        .wrap_anywhere()
        .offset(0.0, 1.0);
        body = body.child(
            row()
                .key(format!("{key}-diff-{i}"))
                .w_full()
                .items_start()
                .fill(bg)
                .child(line_no(line.old))
                .child(line_no(line.new))
                .child(content),
        );
    }
    let header_name = if header.is_empty() {
        "diff".to_string()
    } else {
        header
    };
    let head = row()
        .key(format!("{key}-diff-head"))
        .w_full()
        .items_center()
        .justify_between()
        .px(12.0)
        .py(8.0)
        .fill(rgba(24.0, 24.0, 27.0, 0.5))
        .border_sides([0.0, 0.0, 1.0, 0.0], ink("#27272a"))
        .border_layout()
        .child(
            text(header_name)
                .font(head_name)
                .color(ink("#d4d4d8"))
                .truncate(),
        )
        .child(
            row()
                .items_center()
                .gap(8.0)
                .child(
                    text(format!("+{add}"))
                        .font(head_stats)
                        .color(ink("#34d399")),
                )
                .child(
                    text(format!("-{del}"))
                        .font(head_stats)
                        .color(ink("#f87171")),
                ),
        );
    let mut scroll = col()
        .key(format!("{key}-diff-scroll"))
        .w_full()
        .max_h(420.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(body);
    if lines.len() > MAX_VISIBLE {
        let label = if expanded {
            "Collapse".to_string()
        } else {
            format!("Show all {} lines", lines.len())
        };
        let label_style = {
            let mut style = TextStyle::new(11.5, 600.0);
            style.line_height = 11.5 * 1.85;
            style
        };
        // React keeps the expander inside the 420px scroller, below the table.
        scroll = scroll.child(
            row()
                .key(toggle_key.clone())
                .role("button")
                .w_full()
                .items_center()
                .justify_center()
                .gap(6.0)
                .py(8.0)
                .fill(rgba(24.0, 24.0, 27.0, 0.4))
                .border_sides([1.0, 0.0, 0.0, 0.0], ink("#27272a"))
                .border_layout()
                .child(rune::el().size(13.0, 13.0).inert().shape(icon_chevron(
                    !expanded,
                    13.0,
                    ink("#818cf8"),
                )))
                .child(text(label).font(label_style).color(ink("#818cf8")))
                .on_press(move |s: &mut S| {
                    let set = &mut s.thread_mut(which).expanded_diffs;
                    if !set.remove(&toggle_key) {
                        set.insert(toggle_key.clone());
                    }
                }),
        );
    }
    let card = col()
        .key(format!("{key}-diff-card"))
        .w_full()
        .radius(8.0)
        .border(1.0, ink("#27272a"))
        .fill(rgba(9.0, 9.0, 11.0, 0.6))
        .clip_rounded(8.0)
        .border_layout()
        .child(head)
        .child(scroll);
    // `CodeBlock` wraps `DiffView` in a second, softer rounded border.
    col()
        .key(format!("{key}-diff"))
        .w_full()
        .radius(12.0)
        .border(1.0, rgba(39.0, 39.0, 42.0, 0.6))
        .clip_rounded(12.0)
        .border_layout()
        .child(card)
}

/// React's table: rounded well, uppercase 600 header, hairline row rules.
fn table<S: TranscriptHost + 'static>(
    key: &str,
    headers: &[Vec<stark_kit::markdown::Inline>],
    rows: &[Vec<Vec<stark_kit::markdown::Inline>>],
    body: TextStyle,
) -> El {
    let _ = body;
    let head_style = {
        let mut style = TextStyle::new(13.0, 600.0);
        // The table's `leading-relaxed` (1.625), inherited by the header.
        style.line_height = 13.0 * 1.625;
        // `tracking-wider`.
        style.tracking = 0.65;
        style
    };
    let cell_style = {
        let mut style = TextStyle::new(14.5, 400.0);
        style.line_height = 14.5 * 1.625;
        style
    };
    let cols = headers
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    if cols == 0 {
        return col().key(format!("{key}-table"));
    }
    // Inline code is a padded, bordered inline box in React; `content_widths`
    // measures the glyphs alone, so add the 6px pad + 1px border on each side.
    let measure = |list: &[stark_kit::markdown::Inline], style: TextStyle| {
        let mut runs = Runs::default();
        inlines(&mut runs, list, false);
        let (_min, max) = rune::text::content_widths(&runs.text, &runs.spans, style);
        max + 14.0 * runs.codes.len() as f32
    };
    let mut max = vec![0.0f32; cols];
    for (j, cell) in headers.iter().enumerate() {
        let mut runs = Runs::default();
        inlines(&mut runs, cell, false);
        let upper = runs.text.to_uppercase();
        let (_min, ma) = rune::text::content_widths(&upper, &runs.spans, head_style);
        max[j] = max[j].max(ma + 14.0 * runs.codes.len() as f32);
    }
    for row_cells in rows {
        for (j, cell) in row_cells.iter().enumerate().take(cols) {
            max[j] = max[j].max(measure(cell, cell_style));
        }
    }
    let wanted: Vec<f32> = (0..cols).map(|j| (max[j] + 40.0).max(40.0)).collect();
    let total: f32 = wanted.iter().sum();
    let frac: Vec<f32> = wanted.iter().map(|w| w / total).collect();
    let mut head = row()
        .key(format!("{key}-thead"))
        .w_full()
        .fill(rgba(24.0, 24.0, 27.0, 0.4));
    for (j, frac) in frac.iter().enumerate() {
        let cell = headers.get(j).map(Vec::as_slice).unwrap_or(&[]);
        let mut runs = Runs::default();
        inlines(&mut runs, cell, false);
        runs.text = runs.text.to_uppercase();
        let e = rich::<S>(format!("{key}-th-{j}"), runs, head_style, ink("#d4d4d8"))
            .w_frac(*frac)
            .px(20.0)
            .pt(14.0)
            // Rune snaps a 0.5pt border to one device pixel; take the half
            // point out of padding so the header lands on React's 49.625.
            .pb(13.5)
            .border_sides([0.0, 0.0, 1.0, 0.0], rgba(255.0, 255.0, 255.0, 0.15))
            .border_layout();
        head = head.child(e);
    }
    let mut table = col().key(format!("{key}-table")).w_full().child(head);
    for (r, cells) in rows.iter().enumerate() {
        let mut tr = row()
            .key(format!("{key}-tr-{r}"))
            .w_full()
            .hover(rgba(255.0, 255.0, 255.0, 0.02));
        for (j, frac) in frac.iter().enumerate() {
            let cell = cells.get(j).map(Vec::as_slice).unwrap_or(&[]);
            let mut runs = Runs::default();
            inlines(&mut runs, cell, false);
            let code_cell = !runs.codes.is_empty();
            let e = rich::<S>(
                format!("{key}-td-{r}-{j}"),
                runs,
                cell_style,
                ink("#d4d4d8"),
            )
            .w_frac(*frac)
            .px(20.0)
            .pt(14.0)
            // The mono line box runs 1.375pt taller than the 23.5625 line;
            // React's inline box adds only 0.375, so take 1pt off the bottom.
            .pb(if code_cell { 13.0 } else { 14.0 })
            .border_sides([0.0, 0.0, 1.0, 0.0], rgba(255.0, 255.0, 255.0, 0.08))
            .border_layout();
            tr = tr.child(e);
        }
        table = table.child(tr);
    }
    col()
        .key(format!("{key}-table-wrap"))
        .w_full()
        .radius(12.0)
        .border(1.0, rgba(255.0, 255.0, 255.0, 0.12))
        .border_layout()
        .fill(rgba(9.0, 9.0, 11.0, 0.2))
        .clip_rounded(12.0)
        // React's collapsed last-row border leaves half a point below the rows.
        .pb(0.5)
        .child(table)
}

#[cfg(test)]
mod streaming_tests {
    use super::*;

    #[test]
    fn inline_code_carries_bordered_well() {
        use stark_kit::markdown::Inline;
        let mut runs = Runs::default();
        inlines(
            &mut runs,
            &[Inline::Code("code".into()), Inline::Text(" and ".into())],
            false,
        );
        assert_eq!(runs.codes, vec![(0, 4)]);
        // Exactly one paint-only well at the code bytes (fill + 1px ring are
        // set at construction; the border itself is verified on pixels).
        assert_eq!(runs.highlights.len(), 1);
        assert_eq!(runs.highlights[0].range(), 0..4);
    }

    #[test]
    fn short_hunk_without_counts_starts_at_one() {
        // React's hunk regex makes the `,count` optional; the native split
        // on `,`/space parses the same short form.
        let (lines, _, add, del) = parse_diff("@@ -1 +1 @@\n-old\n+new\n");
        assert_eq!(add, 1);
        assert_eq!(del, 1);
        assert_eq!(lines.len(), 4, "hunk + del + add + trailing ctx");
        assert_eq!(lines[0].kind, DiffKind::Hunk);
        assert_eq!(lines[1].old, Some(1));
        assert_eq!(lines[1].new, None);
        assert_eq!(lines[2].old, None);
        assert_eq!(lines[2].new, Some(1));
    }

    #[test]
    fn unclosed_fence_splits_at_its_line_start() {
        let body = "Intro\n\n```rust\nfn main() {\n";
        let split = stream_split(body);
        assert_eq!(&body[..split], "Intro\n\n");
        assert_eq!(&body[split..], "```rust\nfn main() {\n");
        assert_eq!(
            parse_open_fence(&body[split..]),
            Some(("rust".to_string(), "fn main() {\n".to_string()))
        );
    }

    #[test]
    fn closed_fences_stay_whole() {
        let body = "```rust\nfn main() {}\n```\n\nafter";
        assert_eq!(stream_split(body), body.len());
    }

    #[test]
    fn fence_without_a_language_or_newline_is_not_a_card() {
        assert_eq!(parse_open_fence("```"), None);
        assert_eq!(parse_open_fence("```   \ncode"), None);
        assert_eq!(
            parse_open_fence("```text\ncode"),
            Some(("text".to_string(), "code".to_string()))
        );
    }

    #[test]
    fn dangling_marker_drops_only_when_the_prefix_opened_it() {
        assert_eq!(strip_dangling_marker("and **", "an **open** bold"), "and ");
        assert_eq!(strip_dangling_marker("and **", "plain text"), "and **");
        assert_eq!(strip_dangling_marker("code `", "an `open` span"), "code ");
        assert_eq!(strip_dangling_marker("tilde ~~", "a ~~strike"), "tilde ");
    }
}
