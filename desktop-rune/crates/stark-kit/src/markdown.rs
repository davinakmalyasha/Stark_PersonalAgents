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
    Heading {
        level: u8,
        text: Vec<Inline>,
    },
    List(Vec<Vec<Inline>>),
    Code {
        lang: String,
        text: String,
    },
    /// GFM table: header cells + body rows (cells already inline-parsed, so
    /// `**bold**` and `` `code` `` work inside cells like `Markdown.tsx`).
    Table {
        headers: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
    },
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

/// Split a `| a | b |` row into trimmed cells. Leading/trailing pipes are
/// optional; a line without any pipe is not a row (`None`).
fn split_row(line: &str) -> Option<Vec<String>> {
    if !line.contains('|') {
        return None;
    }
    let mut cells: Vec<String> = line.split('|').map(|c| c.trim().to_string()).collect();
    // Drop the empties from outer pipes (`| a |` → ["", "a", ""]).
    if cells.first().is_some_and(|c| c.is_empty()) {
        cells.remove(0);
    }
    if cells.last().is_some_and(|c| c.is_empty()) {
        cells.pop();
    }
    if cells.is_empty() {
        return None;
    }
    Some(cells)
}

/// A delimiter row: every cell is `:?-+:?` (`|---|---|`, `---|:---:`).
fn is_delimiter(line: &str) -> bool {
    let Some(cells) = split_row(line) else {
        return false;
    };
    !cells.is_empty()
        && cells.iter().all(|c| {
            let c = c.trim();
            let c = c.strip_prefix(':').unwrap_or(c);
            let c = c.strip_suffix(':').unwrap_or(c);
            c.len() >= 3 && c.chars().all(|ch| ch == '-')
        })
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

    let lines: Vec<&str> = text.split('\n').collect();
    let mut li = 0;
    while li < lines.len() {
        let line = lines[li];
        li += 1;
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
        } else if split_row(line).is_some() && lines.get(li).is_some_and(|next| is_delimiter(next))
        {
            // GFM table: header row + delimiter + body rows. Consumes the
            // delimiter and every following pipe row; a non-row line ends it.
            let headers = split_row(line)
                .unwrap_or_default()
                .into_iter()
                .map(|c| parse_inlines(&c))
                .collect();
            li += 1; // delimiter
            let mut rows = Vec::new();
            while let Some(cells) = lines.get(li).and_then(|l| split_row(l)) {
                rows.push(cells.into_iter().map(|c| parse_inlines(&c)).collect());
                li += 1;
            }
            flush_para(&mut para, &mut blocks);
            flush_list(&mut list, &mut blocks);
            blocks.push(Block::Table { headers, rows });
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
    fn gfm_table_parses_header_and_rows() {
        let blocks = parse("| a | b |\n|---|---|\n| 1 | **x** |\n| 2 | `y` |");
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            Block::Table { headers, rows } => {
                assert_eq!(headers.len(), 2);
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0][1], vec![Inline::Bold("x".to_string())]);
                assert_eq!(rows[1][1], vec![Inline::Code("y".to_string())]);
            }
            b => panic!("expected table, got {b:?}"),
        }
    }

    #[test]
    fn pipes_without_delimiter_stay_para() {
        let blocks = parse("a | b\nc | d");
        assert_eq!(blocks.len(), 1);
        assert!(matches!(blocks[0], Block::Para(_)));
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
