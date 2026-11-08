//! Minimal markdown for transcript rows — mirrors the `Markdown.tsx`
//! (`react-markdown` + `remark-gfm`) subset agent replies actually use.
//!
//! Supported: fenced code blocks (```lang), `#` headings, `- ` list items,
//! paragraphs, and inline `**bold**` + `` `code` ``. Everything else passes
//! through as plain text. `std` only; byte offsets are always char-boundary
//! safe. Full GFM tables/syntax-highlighting arrive later.

/// Inline segment with byte offsets into its block text.
#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    Text(String),
    Bold(String),
    Code(String),
}

/// One rendered block.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Para(Vec<Inline>),
    Heading { level: u8, text: Vec<Inline> },
    List(Vec<Vec<Inline>>),
    Code { lang: String, text: String },
}

/// Split `text` into inline segments, tracking byte offsets.
fn parse_inlines(text: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0usize;
    let mut plain_start = 0usize;
    let flush_plain = |out: &mut Vec<Inline>, plain_start: &mut usize, end: usize| {
        if *plain_start < end {
            out.push(Inline::Text(text[*plain_start..end].to_string()));
            *plain_start = end;
        }
    };
    while i < bytes.len() {
        if text[i..].starts_with("**") {
            if let Some(close) = text[i + 2..].find("**") {
                let inner = &text[i + 2..i + 2 + close];
                if !inner.is_empty() && !inner.contains('\n') {
                    flush_plain(&mut out, &mut plain_start, i);
                    out.push(Inline::Bold(inner.to_string()));
                    i += 2 + close + 2;
                    plain_start = i;
                    continue;
                }
            }
            i += 2;
        } else if bytes[i] == b'`' {
            if let Some(close) = text[i + 1..].find('`') {
                let inner = &text[i + 1..i + 1 + close];
                if !inner.is_empty() && !inner.contains('\n') {
                    flush_plain(&mut out, &mut plain_start, i);
                    out.push(Inline::Code(inner.to_string()));
                    i += 1 + close + 1;
                    plain_start = i;
                    continue;
                }
            }
            i += 1;
        } else {
            // Advance one char to keep offsets boundary-safe.
            let ch_len = text[i..].chars().next().map_or(1, |c| c.len_utf8());
            i += ch_len;
        }
    }
    flush_plain(&mut out, &mut plain_start, text.len());
    if out.is_empty() {
        out.push(Inline::Text(String::new()));
    }
    out
}

/// Parse message markdown into blocks.
pub fn parse(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut para: Vec<String> = Vec::new();
    let mut list: Vec<String> = Vec::new();
    let mut fence: Option<(String, Vec<String>)> = None;

    let flush_para = |para: &mut Vec<String>, blocks: &mut Vec<Block>| {
        if !para.is_empty() {
            let joined = para.join("\n");
            if !joined.trim().is_empty() {
                blocks.push(Block::Para(parse_inlines(&joined)));
            }
            para.clear();
        }
    };
    let flush_list = |list: &mut Vec<String>, blocks: &mut Vec<Block>| {
        if !list.is_empty() {
            blocks.push(Block::List(
                list.iter().map(|item| parse_inlines(item)).collect(),
            ));
            list.clear();
        }
    };

    for line in text.split('\n') {
        if let Some((lang, buf)) = fence.as_mut() {
            if line.trim_start().starts_with("```") {
                let text = buf.join("\n");
                blocks.push(Block::Code {
                    lang: lang.clone(),
                    text,
                });
                fence = None;
            } else {
                buf.push(line.to_string());
            }
            continue;
        }
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("```") {
            flush_para(&mut para, &mut blocks);
            flush_list(&mut list, &mut blocks);
            fence = Some((rest.trim().to_string(), Vec::new()));
        } else if let Some(item) = trimmed.strip_prefix("- ") {
            flush_para(&mut para, &mut blocks);
            list.push(item.to_string());
        } else if let Some(hashes) = trimmed.strip_prefix('#') {
            let level = 1 + hashes.chars().take_while(|&c| c == '#').count().min(5) as u8;
            let body = trimmed[level as usize..]
                .trim_start_matches(' ')
                .to_string();
            flush_para(&mut para, &mut blocks);
            flush_list(&mut list, &mut blocks);
            blocks.push(Block::Heading {
                level,
                text: parse_inlines(&body),
            });
        } else if line.trim().is_empty() {
            flush_para(&mut para, &mut blocks);
            flush_list(&mut list, &mut blocks);
        } else {
            if !list.is_empty() && !line.starts_with(' ') && !line.starts_with('\t') {
                flush_list(&mut list, &mut blocks);
            }
            para.push(line.to_string());
        }
    }
    // Unclosed fence renders as code (never drops content).
    if let Some((lang, buf)) = fence {
        blocks.push(Block::Code {
            lang,
            text: buf.join("\n"),
        });
    }
    flush_para(&mut para, &mut blocks);
    flush_list(&mut list, &mut blocks);
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_para_stays_single_text() {
        let blocks = parse("hello world");
        assert_eq!(
            blocks,
            vec![Block::Para(vec![Inline::Text("hello world".to_string())])]
        );
    }

    #[test]
    fn bold_and_code_inlines() {
        let blocks = parse("run **cargo test** with `グリーン` ok");
        assert_eq!(
            blocks,
            vec![Block::Para(vec![
                Inline::Text("run ".to_string()),
                Inline::Bold("cargo test".to_string()),
                Inline::Text(" with ".to_string()),
                Inline::Code("グリーン".to_string()),
                Inline::Text(" ok".to_string()),
            ])]
        );
    }

    #[test]
    fn fence_heading_list_blocks() {
        let blocks = parse("# T\n\n- a\n- b\n\n```rs\nlet x = 1;\n```\ntail");
        assert_eq!(blocks.len(), 4);
        assert!(matches!(blocks[0], Block::Heading { level: 1, .. }));
        assert!(matches!(blocks[1], Block::List(_)));
        match &blocks[2] {
            Block::Code { lang, text } => {
                assert_eq!(lang, "rs");
                assert_eq!(text, "let x = 1;");
            }
            b => panic!("expected code, got {b:?}"),
        }
        assert!(matches!(blocks[3], Block::Para(_)));
    }

    #[test]
    fn unclosed_fence_keeps_content() {
        let blocks = parse("```py\nprint(1)");
        assert!(matches!(blocks[0], Block::Code { .. }));
    }

    #[test]
    fn lone_markers_stay_literal() {
        let blocks = parse("a ** b ` c");
        assert_eq!(
            blocks,
            vec![Block::Para(vec![Inline::Text("a ** b ` c".to_string())])]
        );
    }
}
