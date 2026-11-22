//! Full GFM markdown for transcript rows.
//!
//! Parsing is `pulldown-cmark` (tables, strikethrough, task lists, autolinks,
//! nested lists, quotes, rules, links, images). Rendering stays in
//! `stark-rune-app` so every margin, colour and line box can match the reference UI's
//! reference layout classes exactly. `std` only; byte offsets are always
//! char-boundary safe.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// Inline segment with byte offsets into its block text.
#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    Text(String),
    Bold(Vec<Inline>),
    Italic(Vec<Inline>),
    Strike(Vec<Inline>),
    Code(String),
    Link {
        href: String,
        text: Vec<Inline>,
    },
    Image {
        src: String,
        alt: String,
    },
    /// `  \n` / `<br>`: a hard line break.
    Break,
}

/// One list item: its blocks and a GFM task checkbox when the source marks it.
/// `tight` mirrors remark: no blank lines around the item content, so the reference UI
/// renders no `<p>` wrapper and no paragraph margins apply.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ListItem {
    pub checked: Option<bool>,
    pub blocks: Vec<Block>,
    pub tight: bool,
}

/// Table column alignment (`:---`, `:---:`, `---:`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    None,
    Left,
    Center,
    Right,
}

/// One rendered block.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Para(Vec<Inline>),
    Heading {
        level: u8,
        text: Vec<Inline>,
    },
    List {
        ordered: bool,
        start: u64,
        items: Vec<ListItem>,
    },
    Code {
        lang: String,
        text: String,
    },
    Table {
        headers: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
        align: Vec<Align>,
    },
    Quote(Vec<Block>),
    Rule,
    Image {
        src: String,
        alt: String,
    },
}

fn options() -> Options {
    let mut o = Options::empty();
    o.insert(Options::ENABLE_TABLES);
    o.insert(Options::ENABLE_STRIKETHROUGH);
    o.insert(Options::ENABLE_TASKLISTS);
    o.insert(Options::ENABLE_FOOTNOTES);
    // remark-gfm's autolink literals and GitHub's other extensions.
    o.insert(Options::ENABLE_GFM);
    o
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn align(a: Alignment) -> Align {
    match a {
        Alignment::None => Align::None,
        Alignment::Left => Align::Left,
        Alignment::Center => Align::Center,
        Alignment::Right => Align::Right,
    }
}

enum Frame {
    Root(Vec<Block>),
    Quote(Vec<Block>),
    Item {
        checked: Option<bool>,
        blocks: Vec<Block>,
        /// Tight list items have no `Paragraph` event; their inline text collects here
        /// and flushes to a `Para` before the next block (or when the item ends).
        inline: Vec<Inline>,
        /// False once a real `Paragraph` event lands inside the item (loose
        /// items render `<p>` wrappers with margins; tight ones do not).
        tight: bool,
    },
    Para(Vec<Inline>),
    Heading {
        level: u8,
        text: Vec<Inline>,
    },
    Emphasis(Vec<Inline>),
    Strong(Vec<Inline>),
    Strike(Vec<Inline>),
    Link {
        href: String,
        text: Vec<Inline>,
    },
    Image {
        src: String,
        alt: String,
    },
    Code {
        lang: String,
        text: String,
    },
    List {
        ordered: bool,
        start: u64,
        items: Vec<ListItem>,
    },
    Table {
        align: Vec<Align>,
        header: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
        in_head: bool,
    },
    TableRow(Vec<Vec<Inline>>),
    TableCell(Vec<Inline>),
}

/// Append an inline to the nearest inline container (or an image's alt text).
fn push_inline(frames: &mut [Frame], inline: Inline) {
    if let Some(Frame::Image { alt, .. }) = frames.last_mut() {
        match inline {
            Inline::Text(t) | Inline::Code(t) => alt.push_str(&t),
            Inline::Break => alt.push(' '),
            _ => {}
        }
        return;
    }
    for frame in frames.iter_mut().rev() {
        match frame {
            Frame::Para(v)
            | Frame::Heading { text: v, .. }
            | Frame::Emphasis(v)
            | Frame::Strong(v)
            | Frame::Strike(v)
            | Frame::Link { text: v, .. }
            | Frame::TableCell(v) => {
                push_or_merge(v, inline);
                return;
            }
            // A tight list item: text arrives with no `Paragraph` event.
            Frame::Item { inline: v, .. } => {
                push_or_merge(v, inline);
                return;
            }
            Frame::Root(_) | Frame::Quote(_) => break,
            _ => {}
        }
    }
}

/// Push an inline, merging adjacent plain text so soft breaks stay one run.
fn push_or_merge(target: &mut Vec<Inline>, inline: Inline) {
    if let Inline::Text(text) = &inline {
        if let Some(Inline::Text(last)) = target.last_mut() {
            last.push_str(text);
            return;
        }
    }
    target.push(inline);
}

/// Split bare `http(s)://` and `www.` literals out of text the way remark-gfm's
/// autolink literals do. Explicit links are never re-linked.
fn push_text(frames: &mut [Frame], text: &str) {
    if frames.iter().rev().any(|f| matches!(f, Frame::Link { .. })) {
        push_inline(frames, Inline::Text(text.to_string()));
        return;
    }
    let mut rest = text;
    while !rest.is_empty() {
        let Some((at, prefix)) = find_url(rest) else {
            push_inline(frames, Inline::Text(rest.to_string()));
            return;
        };
        if at > 0 {
            push_inline(frames, Inline::Text(rest[..at].to_string()));
        }
        let tail = &rest[at..];
        let mut end = tail
            .find(|c: char| c.is_whitespace() || c == '<' || c == '"')
            .unwrap_or(tail.len());
        while end > 0 {
            let c = tail[..end].chars().next_back().unwrap_or(' ');
            if matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}') {
                end -= c.len_utf8();
            } else {
                break;
            }
        }
        let url = &tail[..end];
        if url.len() > prefix.len() {
            let href = if prefix == "www." {
                format!("https://{url}")
            } else {
                url.to_string()
            };
            push_inline(
                frames,
                Inline::Link {
                    href,
                    text: vec![Inline::Text(url.to_string())],
                },
            );
        } else {
            push_inline(frames, Inline::Text(url.to_string()));
        }
        rest = &tail[end..];
    }
}

fn find_url(text: &str) -> Option<(usize, &'static str)> {
    let http = text.find("http://").map(|i| (i, "http://"));
    let https = text.find("https://").map(|i| (i, "https://"));
    let www = text.find("www.").map(|i| (i, "www."));
    [http, https, www]
        .into_iter()
        .flatten()
        .min_by_key(|(i, _)| *i)
}

/// Flush a tight list item's implicit paragraph before another block joins it.
fn flush_item_inline(frames: &mut [Frame]) {
    for frame in frames.iter_mut().rev() {
        if let Frame::Item { inline, blocks, .. } = frame {
            if !inline.is_empty() {
                blocks.push(Block::Para(std::mem::take(inline)));
            }
            return;
        }
        if matches!(frame, Frame::Root(_) | Frame::Quote(_)) {
            return;
        }
    }
}

/// Append a block to the nearest block container (root, quote or list item).
fn push_block(frames: &mut [Frame], block: Block) {
    flush_item_inline(frames);
    for frame in frames.iter_mut().rev() {
        match frame {
            Frame::Root(v) | Frame::Quote(v) | Frame::Item { blocks: v, .. } => {
                v.push(block);
                return;
            }
            _ => {}
        }
    }
}

fn end_emphasis(frames: &mut Vec<Frame>) {
    let Some(frame) = frames.pop() else { return };
    match frame {
        Frame::Emphasis(text) => push_inline(frames, Inline::Italic(text)),
        Frame::Strong(text) => push_inline(frames, Inline::Bold(text)),
        Frame::Strike(text) => push_inline(frames, Inline::Strike(text)),
        _ => {}
    }
}

/// Parse message markdown into blocks.
pub fn parse(text: &str) -> Vec<Block> {
    let mut frames = vec![Frame::Root(Vec::new())];
    for event in Parser::new_ext(text, options()) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    // A real paragraph inside an item makes it loose (the reference UI
                    // wraps loose content in `<p>`); the nearest item owns it.
                    for frame in frames.iter_mut().rev() {
                        if let Frame::Item { tight, .. } = frame {
                            *tight = false;
                            break;
                        }
                        if matches!(frame, Frame::Root(_) | Frame::Quote(_)) {
                            break;
                        }
                    }
                    frames.push(Frame::Para(Vec::new()))
                }
                Tag::Heading { level, .. } => frames.push(Frame::Heading {
                    level: heading_level(level),
                    text: Vec::new(),
                }),
                Tag::BlockQuote(_) => frames.push(Frame::Quote(Vec::new())),
                Tag::CodeBlock(kind) => {
                    let lang = match kind {
                        CodeBlockKind::Fenced(info) => info
                            .split_whitespace()
                            .next()
                            .unwrap_or("")
                            .trim_matches(|c| c == '{' || c == '}')
                            .to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    frames.push(Frame::Code {
                        lang,
                        text: String::new(),
                    });
                }
                Tag::List(start) => frames.push(Frame::List {
                    ordered: start.is_some(),
                    start: start.unwrap_or(1),
                    items: Vec::new(),
                }),
                Tag::Item => frames.push(Frame::Item {
                    checked: None,
                    blocks: Vec::new(),
                    inline: Vec::new(),
                    tight: true,
                }),
                Tag::Emphasis => frames.push(Frame::Emphasis(Vec::new())),
                Tag::Strong => frames.push(Frame::Strong(Vec::new())),
                Tag::Strikethrough => frames.push(Frame::Strike(Vec::new())),
                Tag::Link { dest_url, .. } => frames.push(Frame::Link {
                    href: dest_url.to_string(),
                    text: Vec::new(),
                }),
                Tag::Image { dest_url, .. } => frames.push(Frame::Image {
                    src: dest_url.to_string(),
                    alt: String::new(),
                }),
                Tag::Table(aligns) => frames.push(Frame::Table {
                    align: aligns.into_iter().map(align).collect(),
                    header: Vec::new(),
                    rows: Vec::new(),
                    in_head: false,
                }),
                Tag::TableHead => {
                    for frame in frames.iter_mut().rev() {
                        if let Frame::Table { in_head, .. } = frame {
                            *in_head = true;
                            break;
                        }
                    }
                }
                Tag::TableRow => frames.push(Frame::TableRow(Vec::new())),
                Tag::TableCell => frames.push(Frame::TableCell(Vec::new())),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => {
                    if let Some(Frame::Para(text)) = frames.pop() {
                        push_block(&mut frames, Block::Para(text));
                    }
                }
                TagEnd::Heading(_) => {
                    if let Some(Frame::Heading { level, text }) = frames.pop() {
                        push_block(&mut frames, Block::Heading { level, text });
                    }
                }
                TagEnd::BlockQuote(_) => {
                    if let Some(Frame::Quote(blocks)) = frames.pop() {
                        push_block(&mut frames, Block::Quote(blocks));
                    }
                }
                TagEnd::CodeBlock => {
                    if let Some(Frame::Code { lang, text }) = frames.pop() {
                        push_block(&mut frames, Block::Code { lang, text });
                    }
                }
                TagEnd::List(_) => {
                    if let Some(Frame::List {
                        ordered,
                        start,
                        items,
                    }) = frames.pop()
                    {
                        push_block(
                            &mut frames,
                            Block::List {
                                ordered,
                                start,
                                items,
                            },
                        );
                    }
                }
                TagEnd::Item => {
                    flush_item_inline(&mut frames);
                    if let Some(Frame::Item {
                        checked,
                        blocks,
                        tight,
                        ..
                    }) = frames.pop()
                    {
                        if let Some(Frame::List { items, .. }) = frames.last_mut() {
                            items.push(ListItem {
                                checked,
                                blocks,
                                tight,
                            });
                        }
                    }
                }
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                    end_emphasis(&mut frames);
                }
                TagEnd::Link => {
                    if let Some(Frame::Link { href, text }) = frames.pop() {
                        push_inline(&mut frames, Inline::Link { href, text });
                    }
                }
                TagEnd::Image => {
                    if let Some(Frame::Image { src, alt }) = frames.pop() {
                        push_inline(&mut frames, Inline::Image { src, alt });
                    }
                }
                TagEnd::Table => {
                    if let Some(Frame::Table {
                        align,
                        header,
                        rows,
                        ..
                    }) = frames.pop()
                    {
                        push_block(
                            &mut frames,
                            Block::Table {
                                headers: header,
                                rows,
                                align,
                            },
                        );
                    }
                }
                TagEnd::TableHead => {
                    for frame in frames.iter_mut().rev() {
                        if let Frame::Table { in_head, .. } = frame {
                            *in_head = false;
                            break;
                        }
                    }
                }
                TagEnd::TableRow => {
                    if let Some(Frame::TableRow(row)) = frames.pop() {
                        if let Some(Frame::Table { rows, .. }) = frames.last_mut() {
                            rows.push(row);
                        }
                    }
                }
                TagEnd::TableCell => {
                    if let Some(Frame::TableCell(cell)) = frames.pop() {
                        if let Some(Frame::Table {
                            in_head: true,
                            header,
                            ..
                        }) = frames.last_mut()
                        {
                            header.push(cell);
                        } else if let Some(Frame::TableRow(row)) = frames.last_mut() {
                            row.push(cell);
                        }
                    }
                }
                _ => {}
            },
            Event::Text(text) => {
                if let Some(Frame::Code { text: code, .. }) = frames.last_mut() {
                    code.push_str(&text);
                } else {
                    push_text(&mut frames, &text);
                }
            }
            Event::Code(text) => push_inline(&mut frames, Inline::Code(text.to_string())),
            Event::SoftBreak => push_inline(&mut frames, Inline::Text(" ".to_string())),
            Event::HardBreak => push_inline(&mut frames, Inline::Break),
            Event::Rule => push_block(&mut frames, Block::Rule),
            Event::TaskListMarker(checked) => {
                for frame in frames.iter_mut().rev() {
                    if let Frame::Item { checked: slot, .. } = frame {
                        *slot = Some(checked);
                        break;
                    }
                }
            }
            Event::Html(text) | Event::InlineHtml(text) => {
                push_inline(&mut frames, Inline::Text(text.to_string()));
            }
            Event::FootnoteReference(name) => {
                push_inline(&mut frames, Inline::Text(format!("[^{name}]")));
            }
            Event::InlineMath(text) | Event::DisplayMath(text) => {
                push_inline(&mut frames, Inline::Text(text.to_string()));
            }
        }
    }
    let mut root = Vec::new();
    for frame in frames {
        if let Frame::Root(mut blocks) = frame {
            root.append(&mut blocks);
        }
    }
    root
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Inline {
        Inline::Text(s.to_string())
    }

    #[test]
    fn plain_para_stays_single_text() {
        let blocks = parse("hello world");
        assert_eq!(blocks, vec![Block::Para(vec![text("hello world")])]);
    }

    #[test]
    fn bold_italic_strike_and_code_inlines() {
        let blocks = parse("a **b** c *d* e ~~f~~ g `h`");
        let Block::Para(inl) = &blocks[0] else {
            panic!("{blocks:?}");
        };
        assert_eq!(
            inl,
            &vec![
                text("a "),
                Inline::Bold(vec![text("b")]),
                text(" c "),
                Inline::Italic(vec![text("d")]),
                text(" e "),
                Inline::Strike(vec![text("f")]),
                text(" g "),
                Inline::Code("h".into()),
            ]
        );
    }

    #[test]
    fn link_and_autolink() {
        let blocks = parse("see [docs](https://example.com) and https://opencode.ai");
        let Block::Para(inl) = &blocks[0] else {
            panic!("{blocks:?}");
        };
        assert!(matches!(
            &inl[1],
            Inline::Link { href, .. } if href == "https://example.com"
        ));
        assert!(inl
            .iter()
            .any(|i| matches!(i, Inline::Link { href, .. } if href == "https://opencode.ai")));
    }

    #[test]
    fn headings_carry_their_level() {
        let blocks = parse("# One\n\n### Three");
        assert_eq!(
            blocks,
            vec![
                Block::Heading {
                    level: 1,
                    text: vec![text("One")]
                },
                Block::Heading {
                    level: 3,
                    text: vec![text("Three")]
                },
            ]
        );
    }

    #[test]
    fn unordered_ordered_and_task_lists_nest() {
        let blocks = parse("- one\n- two\n  - nested\n\n1. first\n2. [x] done\n3. [ ] todo");
        assert_eq!(blocks.len(), 2);
        let Block::List {
            ordered: false,
            items,
            ..
        } = &blocks[0]
        else {
            panic!("{blocks:?}");
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].checked, None);
        assert!(matches!(
            &items[1].blocks[1],
            Block::List { ordered: false, .. }
        ));
        let Block::List {
            ordered: true,
            start: 1,
            items,
        } = &blocks[1]
        else {
            panic!("{blocks:?}");
        };
        assert_eq!(items[1].checked, Some(true));
        assert_eq!(items[2].checked, Some(false));
    }

    #[test]
    fn tight_items_skip_paragraph_margins_loose_ones_do_not() {
        let tight = parse("- one\n- two\n  - nested\n");
        let Block::List { items, .. } = &tight[0] else {
            panic!("{tight:?}");
        };
        assert!(items.iter().all(|item| item.tight));
        let loose = parse("- one\n\n- two\n");
        let Block::List { items, .. } = &loose[0] else {
            panic!("{loose:?}");
        };
        assert!(items.iter().all(|item| !item.tight));
    }

    #[test]
    fn fenced_code_keeps_language_and_body() {
        let blocks = parse("```rust\nlet x = 1;\n```");
        assert_eq!(
            blocks,
            vec![Block::Code {
                lang: "rust".into(),
                text: "let x = 1;\n".into()
            }]
        );
    }

    #[test]
    fn unclosed_fence_keeps_content() {
        let blocks = parse("```\nstill here");
        assert_eq!(
            blocks,
            vec![Block::Code {
                lang: String::new(),
                text: "still here".into()
            }]
        );
    }

    #[test]
    fn gfm_table_parses_header_rows_and_alignment() {
        let blocks = parse("| a | b |\n|:--|--:|\n| 1 | 2 |");
        assert_eq!(
            blocks,
            vec![Block::Table {
                headers: vec![vec![text("a")], vec![text("b")]],
                rows: vec![vec![vec![text("1")], vec![text("2")]]],
                align: vec![Align::Left, Align::Right],
            }]
        );
    }

    #[test]
    fn quote_and_rule() {
        let blocks = parse("> quoted\n\n---");
        assert_eq!(
            blocks,
            vec![
                Block::Quote(vec![Block::Para(vec![text("quoted")])]),
                Block::Rule,
            ]
        );
    }

    #[test]
    fn soft_break_is_a_space_hard_break_is_a_break() {
        let blocks = parse("one\ntwo  \nthree");
        let Block::Para(inl) = &blocks[0] else {
            panic!("{blocks:?}");
        };
        assert_eq!(inl, &vec![text("one two"), Inline::Break, text("three")]);
    }

    #[test]
    fn lone_markers_stay_literal() {
        let blocks = parse("a * b ** c ` d");
        let Block::Para(inl) = &blocks[0] else {
            panic!("{blocks:?}");
        };
        assert!(inl.iter().all(|i| matches!(i, Inline::Text(_))));
    }
}
