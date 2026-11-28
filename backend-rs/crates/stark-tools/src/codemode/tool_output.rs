//! OpenCode `ToolOutput.truncate` for the single text part Stark tools return.
//!
//! OpenCode applies this to every normalized tool result before it enters
//! session history (`packages/core/src/tool-output.ts`). The algorithm is a
//! single cut at the line or UTF-8 byte limit, a marker, and a spilled copy of
//! the full text. Stark's tool results are one string, so there is no content
//! part regrouping; the model-visible text is `shown + "\n" + marker`, which is
//! exactly how OpenCode's text parts serialize.
//!
//! MIT attribution: truncation semantics © OpenCode authors (MIT), ported from
//! `packages/core/src/tool-output.ts` at dev commit `9964772`.

use std::path::{Path, PathBuf};

use unicode_segmentation::UnicodeSegmentation;

/// OpenCode `ToolOutput.MAX_LINES`.
pub const MAX_LINES: usize = 2_000;
/// OpenCode `ToolOutput.MAX_BYTES` (50 KiB).
pub const MAX_BYTES: usize = 50 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Truncated {
    /// The model-visible text.
    pub text: String,
    pub truncated: bool,
    /// Set when the full text was written to disk.
    pub output_path: Option<PathBuf>,
    pub total_lines: usize,
    pub shown_lines: usize,
}

/// OpenCode `countLines`: an empty string has zero lines and a final line
/// break does not add one.
fn count_lines(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let mut count = 1;
    let mut search = 0;
    while let Some(offset) = text[search..].find('\n') {
        let index = search + offset;
        if index >= text.len() - 1 {
            break;
        }
        count += 1;
        search = index + 1;
    }
    count
}

/// OpenCode `afterLines`: the byte index where the line after the first
/// `count` lines starts, or the end of the text.
fn after_lines(text: &str, count: usize) -> usize {
    let mut index = 0;
    for _ in 0..count {
        match text[index..].find('\n') {
            Some(offset) => index += offset + 1,
            None => return text.len(),
        }
    }
    index
}

fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// The byte offset of a UTF-16 index, rounded up when the index falls inside
/// a surrogate pair. OpenCode's `String.prototype.slice` can produce a lone
/// surrogate there; Rust strings cannot, so include the whole scalar.
fn byte_at_utf16(text: &str, target: usize) -> usize {
    let mut units = 0;
    for (offset, character) in text.char_indices() {
        if units >= target {
            return offset;
        }
        let next = units + character.len_utf16();
        if target < next {
            return offset + character.len_utf8();
        }
        units = next;
    }
    text.len()
}

/// OpenCode `byteBoundary`: the longest prefix that fits `budget` UTF-8 bytes
/// without splitting a code point, then backed off to a grapheme boundary.
///
/// OpenCode reads `TextEncoder.encodeInto(...).read`, a UTF-16 index, and uses
/// it to slice the original string. Mirroring those units matters for
/// grapheme clusters whose code points are not one UTF-16 unit each.
fn byte_boundary(text: &str, budget: usize) -> usize {
    let mut index = 0;
    for (offset, character) in text.char_indices() {
        let next = offset + character.len_utf8();
        if next > budget {
            break;
        }
        index = next;
    }
    if index == text.len() || index == 0 {
        return index;
    }
    let before = text[..index].chars().next_back().unwrap_or('\0');
    let after = text[index..].chars().next().unwrap_or('\0');
    // Two ASCII characters always have a boundary between them, except the
    // two halves of CRLF.
    if before.is_ascii() && after.is_ascii() {
        return if before == '\r' && after == '\n' {
            index - 1
        } else {
            index
        };
    }
    let start = text[..index].rfind('\n').map(|at| at + 1).unwrap_or(0);
    let start_unit = utf16_len(&text[..start]);
    let unit_index = utf16_len(&text[..index]);
    let total_units = utf16_len(text);
    let slice_end = byte_at_utf16(text, (unit_index + 2).min(total_units));
    let target = unit_index - start_unit;
    let mut units = 0;
    for (offset, grapheme) in text[start..slice_end].grapheme_indices(true) {
        let grapheme_units = utf16_len(grapheme);
        if units == target {
            return start + offset;
        }
        if units < target && target < units + grapheme_units {
            return start + offset;
        }
        units += grapheme_units;
    }
    index
}

/// OpenCode `fileName` without the extension: `tool_` plus an ascending
/// identifier.
fn file_name() -> String {
    format!("tool_{}", super::jobs::new_identifier())
}

/// Truncate one tool-result text, spilling the full text into `directory`.
///
/// A write failure keeps the truncated preview and puts OpenCode's
/// "could not be saved" marker in place of the path.
pub fn truncate(text: &str, directory: &Path) -> Truncated {
    let total_lines = count_lines(text);
    if total_lines <= MAX_LINES && text.len() <= MAX_BYTES {
        return Truncated {
            text: text.to_string(),
            truncated: false,
            output_path: None,
            total_lines,
            shown_lines: total_lines,
        };
    }

    // Cut once, at the line or byte limit, whichever comes first, so a line
    // too long for the space left shows its start. The preview never ends on
    // a line break.
    let end = after_lines(text, MAX_LINES).min(byte_boundary(text, MAX_BYTES));
    let shown_end = if text[..end].ends_with("\r\n") {
        end.saturating_sub(2)
    } else if text[..end].ends_with('\n') {
        end.saturating_sub(1)
    } else {
        end
    };
    let shown = &text[..shown_end];
    let shown_lines = count_lines(&text[..end]);
    let last_line_cut = shown_lines > 0
        && shown.len() < text.len()
        && !text[shown.len()..].starts_with('\n')
        && !text[shown.len()..].starts_with("\r\n");
    let range = if shown_lines == 0 {
        "0 lines".to_string()
    } else {
        format!("lines 1-{shown_lines}")
    };

    let path = directory.join(file_name());
    let failure = std::fs::create_dir_all(directory)
        .and_then(|_| std::fs::write(&path, text))
        .err()
        .map(|error| error.to_string());
    let marker = format!(
        "[showing {range} of {total_lines}{}; {}]",
        if last_line_cut {
            " (last line cut)"
        } else {
            ""
        },
        match &failure {
            Some(error) => format!("full output could not be saved: {error}"),
            None => format!("full output saved to {}", path.display()),
        }
    );
    let text = if shown.is_empty() {
        marker
    } else {
        format!("{shown}\n{marker}")
    };
    Truncated {
        text,
        truncated: true,
        output_path: failure.is_none().then_some(path),
        total_lines,
        shown_lines,
    }
}

/// OpenCode `ToolOutput.DIRECTORY` under Stark's app-data root.
pub fn directory(harness_root: &Path) -> PathBuf {
    harness_root.join(".deep_agents").join("tool-output")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_text_is_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let result = truncate("hello\nworld", dir.path());
        assert!(!result.truncated);
        assert_eq!(result.text, "hello\nworld");
        assert_eq!(result.total_lines, 2);
        assert!(result.output_path.is_none());
    }

    #[test]
    fn line_limit_cuts_without_a_trailing_break() {
        let dir = tempfile::tempdir().unwrap();
        let input = (0..3_000)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let result = truncate(&input, dir.path());
        assert!(result.truncated);
        assert!(result.text.starts_with("line 0\n"));
        assert!(result.text.contains("line 1999\n"));
        assert!(
            result
                .text
                .contains("[showing lines 1-2000 of 3000; full output saved to")
        );
        assert!(result.output_path.as_ref().unwrap().is_file());
        assert_eq!(
            std::fs::read_to_string(result.output_path.unwrap()).unwrap(),
            input
        );
    }

    #[test]
    fn byte_limit_marks_a_cut_line_and_never_splits_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let input = format!("{}\nend", "😀".repeat(40_000));
        let result = truncate(&input, dir.path());
        assert!(result.truncated);
        assert!(result.text.contains("(last line cut)"));
        // The marker follows a whole number of emoji, not a replacement char.
        assert!(!result.text.contains('\u{FFFD}'));
        assert!(result.text.starts_with('😀'));
        assert!(result.output_path.as_ref().unwrap().is_file());
    }

    #[test]
    fn crlf_at_the_cut_is_not_split() {
        let dir = tempfile::tempdir().unwrap();
        // The byte budget lands between the CR and LF: OpenCode backs off one
        // byte, so the preview never ends in a lone CR.
        let input = format!("{}\r\nsecond", "a".repeat(MAX_BYTES - 1));
        let result = truncate(&input, dir.path());
        assert!(result.truncated);
        assert!(result.text.contains("a\n[showing"));
        assert!(!result.text.contains("a\r\n[showing"));
    }

    #[test]
    fn truncation_matches_opencode_source() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("testdata/opencode_tool_output.json")).unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let input = case["text"].as_str().unwrap();
            let expected = &case["expected"];
            let dir = tempfile::tempdir().unwrap();
            let got = truncate(input, &directory(dir.path()));
            let mut text = got.text;
            if let Some(path) = &got.output_path {
                text = text.replace(
                    &path.to_string_lossy().into_owned(),
                    "tool-output/tool_FILE",
                );
            }
            assert_eq!(
                text,
                expected["text"].as_str().unwrap(),
                "case {}",
                case["name"].as_str().unwrap()
            );
            assert_eq!(
                got.truncated,
                expected["truncated"].as_bool().unwrap(),
                "case {}",
                case["name"].as_str().unwrap()
            );
        }
    }

    /// The source fixture replaces the whole path with `tool-output/tool_FILE`,
    /// so it cannot show that the production directory has OpenCode's shape.
    /// This test pins the layout and identifier format the marker embeds.
    #[test]
    fn spill_path_matches_opencode_layout() {
        let dir = tempfile::tempdir().unwrap();
        let output_dir = directory(dir.path());
        let input = (0..MAX_LINES + 1)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");

        let result = truncate(&input, &output_dir);
        let path = result.output_path.expect("spill file");

        assert_eq!(path.parent(), Some(output_dir.as_path()));
        assert!(output_dir.ends_with(Path::new(".deep_agents").join("tool-output")));

        let name = path.file_name().unwrap().to_str().unwrap();
        let rest = name.strip_prefix("tool_").expect("tool_ prefix");
        assert_eq!(rest.len(), 26, "OpenCode Identifier length");
        // `Identifier.create`: 12 lowercase hex time digits then 14 base62.
        assert!(
            rest.as_bytes()[..12]
                .iter()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        );
        assert!(rest.as_bytes()[12..].iter().all(u8::is_ascii_alphanumeric));

        assert!(
            result
                .text
                .ends_with(&format!("full output saved to {}]", path.display()))
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), input);
    }

    /// OpenCode keeps the truncated preview and swaps the path for the OS
    /// error text when the spill write fails. The exact text is runtime
    /// specific (`ENOSPC: ...` on Node, `No space left on device (os error
    /// 28)` on Rust), so this pins the Stark-stable shape instead of the
    /// bytes: still truncated, no `output_path`, preview preserved, and the
    /// `could not be saved` marker in place of the path.
    #[test]
    fn spill_failure_keeps_preview_with_stark_error_text() {
        let dir = tempfile::tempdir().unwrap();
        // Occupy the directory path with a file so `create_dir_all` fails.
        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, "not a directory").unwrap();
        let input = (0..MAX_LINES + 1)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");

        let result = truncate(&input, &blocked);
        assert!(result.truncated);
        assert!(result.output_path.is_none());
        assert!(result.text.starts_with("line 0\n"));
        assert!(result.text.contains("full output could not be saved: "));
        assert!(!result.text.contains("full output saved to"));
    }

    #[test]
    fn count_lines_matches_opencode() {
        assert_eq!(count_lines(""), 0);
        assert_eq!(count_lines("a"), 1);
        assert_eq!(count_lines("a\n"), 1);
        assert_eq!(count_lines("\n"), 1);
        assert_eq!(count_lines("\n\n"), 2);
        assert_eq!(count_lines("a\nb\n"), 2);
    }
}
