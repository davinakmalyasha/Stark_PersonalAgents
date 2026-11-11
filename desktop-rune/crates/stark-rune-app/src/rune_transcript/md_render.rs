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
}

const INLINE_CODE_SIZE: f32 = 13.5;

fn inline_code_span(start: usize, end: usize) -> rune::Span {
    rune::Span::new(start, end)
        .mono()
        .size(INLINE_CODE_SIZE)
        .color(ink("#a5b4fc"))
        // `bg-zinc-800/80`: rgba(39,39,42,.8).
        .bg(rgba(39.0, 39.0, 42.0, 0.8), 4.0)
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
                runs.spans.push(inline_code_span(start, runs.text.len()));
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

fn block<S: TranscriptHost + 'static>(
    s: &S,
    which: Thread,
    key: &str,
    block: &stark_kit::markdown::Block,
    body: TextStyle,
    depth: usize,
) -> El {
    use stark_kit::markdown::Block;
    match block {
        Block::Para(list) => {
            let mut runs = Runs::default();
            inlines(&mut runs, list, false);
            rich::<S>(format!("{key}-p"), runs, body, ink("#e4e4e7"))
        }
        Block::Heading { level, text: list } => {
            let mut runs = Runs::default();
            inlines(&mut runs, list, false);
            let mut e = rich::<S>(
                format!("{key}-h"),
                runs,
                heading_style(*level),
                heading_color(*level),
            );
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
                )
                .grow(1.0)
                .min_w(0.0);
                let marker = list_marker(*ordered, i, *start, body);
                let mut row = row()
                    .key(format!("{key}-li-{i}"))
                    .w_full()
                    .items_start()
                    .pl(8.0);
                if let Some(checked) = item.checked {
                    let boxed = rune::row()
                        .items_start()
                        .gap(6.0)
                        .child(checkbox(checked))
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
            let inner = blocks_inner::<S>(
                s,
                which,
                &format!("{key}-quote"),
                list,
                body,
                depth + 1,
                false,
            );
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
                .child(inner.grow(1.0).min_w(0.0))
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
fn blocks_inner<S: TranscriptHost + 'static>(
    s: &S,
    which: Thread,
    key: &str,
    list: &[stark_kit::markdown::Block],
    body: TextStyle,
    depth: usize,
    in_item: bool,
) -> El {
    let _ = depth;
    let mut stack = col().key(format!("{key}-blocks")).w_full();
    let mut prev_mb = 0.0f32;
    let mut after_heading = false;
    for (i, b) in list.iter().enumerate() {
        let (mt, mb) = if in_item {
            match b {
                stark_kit::markdown::Block::Para(_) => (if i == 0 { 0.0 } else { 12.0 }, 12.0),
                stark_kit::markdown::Block::List { .. } => (4.0, 4.0),
                other => margins(other),
            }
        } else {
            margins(b)
        };
        let top = if i == 0 {
            0.0
        } else if matches!(b, stark_kit::markdown::Block::Heading { .. }) {
            mt.max(prev_mb)
        } else if after_heading {
            prev_mb
        } else {
            mt.max(prev_mb)
        };
        let e = block::<S>(s, which, &format!("{key}-b{i}"), b, body, depth + 1);
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
    let blocks = stark_kit::markdown::parse(body);
    if blocks.is_empty() {
        return col().key(format!("{key}-md"));
    }
    blocks_inner::<S>(s, which, key, &blocks, body_style, 0, false)
}

/// A list marker: `•` in a 6pt box or `1.` right-aligned in 12pt.
fn list_marker(ordered: bool, index: usize, start: u64, body: TextStyle) -> El {
    if ordered {
        row()
            .w(12.0)
            .h(body.line_height)
            .justify_end()
            .items_center()
            .child(
                text(format!("{}.", start + index as u64))
                    .font(body)
                    .color(ink("#e4e4e7")),
            )
    } else {
        row()
            .w(6.0)
            .h(body.line_height)
            .center()
            .child(rune::el().size(4.0, 4.0).radius(2.0).fill(ink("#e4e4e7")))
    }
}

/// GFM task checkbox (`input[type=checkbox]`, 13pt, indigo accent).
fn checkbox(checked: bool) -> El {
    let mut e = rune::el()
        .size(13.0, 13.0)
        .mt(3.0)
        .radius(2.0)
        .border(
            1.0,
            if checked {
                ink("#6366f1")
            } else {
                ink("#52525b")
            },
        )
        .fill(if checked {
            ink("#6366f1")
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
                    .stroke_path(&p, 1.4, rune::LineCap::Round, ink("#ffffff"))
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
        .border(1.0, rgba(39.0, 39.0, 42.0, 0.6))
        .border_layout()
        .child(text(label).font(label_style).color(ink("#71717a")));
    let mut spans = Vec::new();
    if highlighted {
        let theme = syntax_theme();
        for (range, paint) in rune_kit::syntax::highlight(Some(lang), code).iter() {
            let color = match paint {
                rune_kit::syntax::Paint::Kind(kind) => theme.color(*kind),
                rune_kit::syntax::Paint::Color(c) => *c,
            };
            spans.push(rune::Span::new(range.start, range.end).color(color));
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
        .border_sides([1.0, 0.0, 1.0, 0.0], rgba(39.0, 39.0, 42.0, 0.6))
        .border_layout()
        .corners([0.0, 0.0, 12.0, 12.0])
        .child(col().px(16.0).py(16.0).child(code_text));
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
#[derive(Clone, Copy, PartialEq, Eq)]
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
    let mut body = col().key(format!("{key}-diff-lines")).w_full();
    for (i, line) in visible.iter().enumerate() {
        let (bg, fg) = line_style(line.kind);
        let fg = fg.unwrap_or(ink("#a1a1aa"));
        let line_no = |n: Option<u32>| {
            row().w(42.0).justify_end().px(10.0).child(
                text(n.map(|n| n.to_string()).unwrap_or_default())
                    .font(mono)
                    .color(ink("#52525b")),
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
        .wrap_anywhere();
        body = body.child(
            row()
                .key(format!("{key}-diff-{i}"))
                .w_full()
                .items_start()
                .fill(bg)
                .child(
                    line_no(line.old)
                        .border_sides([0.0, 0.0, 0.0, 1.0], rgba(39.0, 39.0, 42.0, 0.5)),
                )
                .child(
                    line_no(line.new)
                        .border_sides([0.0, 0.0, 0.0, 1.0], rgba(39.0, 39.0, 42.0, 0.5)),
                )
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
                .font(mono)
                .color(ink("#d4d4d8"))
                .truncate(),
        )
        .child(
            row()
                .items_center()
                .gap(8.0)
                .child(
                    text(format!("+{add}"))
                        .font(TextStyle::new(11.0, 600.0).mono())
                        .color(ink("#34d399")),
                )
                .child(
                    text(format!("-{del}"))
                        .font(TextStyle::new(11.0, 600.0).mono())
                        .color(ink("#f87171")),
                ),
        );
    let mut card = col()
        .key(format!("{key}-diff"))
        .w_full()
        .radius(8.0)
        .border(1.0, ink("#27272a"))
        .fill(rgba(9.0, 9.0, 11.0, 0.6))
        .clip_rounded(8.0)
        .child(head)
        .child(
            col()
                .key(format!("{key}-diff-scroll"))
                .w_full()
                .max_h(420.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(body),
        );
    if lines.len() > MAX_VISIBLE {
        let label = if expanded {
            "Collapse".to_string()
        } else {
            format!("Show all {} lines", lines.len())
        };
        card = card.child(
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
                .child(
                    text(label)
                        .font(TextStyle::new(11.5, 600.0))
                        .color(ink("#818cf8")),
                )
                .on_press(move |s: &mut S| {
                    let set = &mut s.thread_mut(which).expanded_diffs;
                    if !set.remove(&toggle_key) {
                        set.insert(toggle_key.clone());
                    }
                }),
        );
    }
    card
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
        style.line_height = 13.0 * 1.85;
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
    let widths = |list: &[stark_kit::markdown::Inline], style: TextStyle| {
        let mut runs = Runs::default();
        inlines(&mut runs, list, false);
        rune::text::content_widths(&runs.text, &runs.spans, style)
    };
    let mut min = vec![0.0f32; cols];
    let mut max = vec![0.0f32; cols];
    for (j, cell) in headers.iter().enumerate() {
        let mut runs = Runs::default();
        inlines(&mut runs, cell, false);
        let upper = runs.text.to_uppercase();
        let (mi, ma) = rune::text::content_widths(&upper, &runs.spans, head_style);
        min[j] = min[j].max(mi);
        max[j] = max[j].max(ma);
    }
    for row_cells in rows {
        for (j, cell) in row_cells.iter().enumerate() {
            let (mi, ma) = widths(cell, cell_style);
            min[j] = min[j].max(mi);
            max[j] = max[j].max(ma);
        }
    }
    let mut head = row()
        .key(format!("{key}-thead"))
        .w_full()
        .items_start()
        .fill(rgba(24.0, 24.0, 27.0, 0.4));
    for j in 0..cols {
        let cell = headers.get(j).map(Vec::as_slice).unwrap_or(&[]);
        let mut runs = Runs::default();
        inlines(&mut runs, cell, false);
        runs.text = runs.text.to_uppercase();
        let e = rich::<S>(format!("{key}-th-{j}"), runs, head_style, ink("#d4d4d8"))
            .grow((max[j] + 40.0).max(40.0))
            .shrink()
            .min_w(min[j] + 40.0)
            .px(20.0)
            .py(14.0)
            .border_sides([0.0, 0.0, 1.0, 0.0], rgba(255.0, 255.0, 255.0, 0.15))
            .border_layout();
        head = head.child(e);
    }
    let mut table = col().key(format!("{key}-table")).w_full().child(head);
    for (r, cells) in rows.iter().enumerate() {
        let mut tr = row()
            .key(format!("{key}-tr-{r}"))
            .w_full()
            .items_start()
            .hover(rgba(255.0, 255.0, 255.0, 0.02));
        for j in 0..cols {
            let cell = cells.get(j).map(Vec::as_slice).unwrap_or(&[]);
            let mut runs = Runs::default();
            inlines(&mut runs, cell, false);
            let e = rich::<S>(
                format!("{key}-td-{r}-{j}"),
                runs,
                cell_style,
                ink("#d4d4d8"),
            )
            .grow((max[j] + 40.0).max(40.0))
            .shrink()
            .min_w(min[j] + 40.0)
            .px(20.0)
            .py(14.0)
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
        .fill(rgba(9.0, 9.0, 11.0, 0.2))
        .clip_rounded(12.0)
        .child(table)
}
