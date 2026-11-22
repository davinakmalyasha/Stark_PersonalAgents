use base64::Engine;
use serde_json::Value;

use crate::paths::resolve_workspace_path;
use crate::{ToolContext, ToolErrorClass, ToolOutcome};

const DEFAULT_LIMIT: usize = 2000;
const MAX_LIMIT: usize = 10000;
const MAX_LINE_CHARS: usize = 2000;

/// A read failure with its repair class. A missing file or bad window is an
/// argument problem; an unreadable file is a tool problem. Untyped errors left
/// the harness with no repair hint.
type ReadFailure = (ToolErrorClass, String);

fn io_failure(error: std::io::Error) -> ReadFailure {
    let class = if error.kind() == std::io::ErrorKind::NotFound {
        ToolErrorClass::Args
    } else {
        ToolErrorClass::Tool
    };
    (class, error.to_string())
}

pub async fn read_file(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    if let Some(paths) = args.get("file_paths").and_then(Value::as_array) {
        let mut parts = Vec::new();
        let mut is_error = false;
        let mut first_class = None;
        for path in paths {
            if let Some(path) = path.as_str().filter(|path| !path.trim().is_empty()) {
                let outcome = read_one(ctx, path, args).await;
                if outcome.is_error && first_class.is_none() {
                    first_class = outcome.class;
                }
                is_error |= outcome.is_error;
                parts.push(outcome.content);
            }
        }
        if parts.is_empty() {
            return ToolOutcome::err_class(
                ToolErrorClass::Args,
                "Error: file_paths empty or invalid",
            );
        }
        // Keep the first failure's class so a batch read still gets the right
        // repair hint instead of becoming untyped.
        let mut combined = ToolOutcome::with_code(parts.join("\n\n"), is_error, None);
        combined.class = first_class;
        return combined;
    }

    if let Some(path) = args.get("file_paths").and_then(Value::as_str) {
        return read_one(ctx, path, args).await;
    }

    match args.get("file_path").and_then(Value::as_str) {
        Some(path) if !path.trim().is_empty() => read_one(ctx, path, args).await,
        _ => ToolOutcome::err_class(
            ToolErrorClass::Args,
            "Error: read_file requires file_path or file_paths",
        ),
    }
}

async fn read_one(ctx: &ToolContext, raw_path: &str, args: &Value) -> ToolOutcome {
    let path = match resolve_workspace_path(&ctx.workspace, raw_path) {
        Ok(path) => path,
        Err(error) => {
            return ToolOutcome::err_class(ToolErrorClass::Args, format!("Error: {error}"));
        }
    };

    if let Some(ext) = path.extension().and_then(|ext| ext.to_str())
        && let Some(mime) = crate::images::mime_for_ext(ext)
    {
        return read_image(ctx, &path, raw_path, mime).await;
    }

    let offset = args
        .get("offset")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1) as usize;
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .filter(|limit| *limit > 0)
        .map(|limit| limit as usize)
        .unwrap_or(DEFAULT_LIMIT)
        .min(MAX_LIMIT);

    let (lines, total) = match read_window(&path, offset, limit).await {
        Ok(value) => value,
        Err((class, error)) => {
            return ToolOutcome::err_class(class, format!("Error: {raw_path}: {error}"));
        }
    };

    if total > 0 && offset > total {
        return ToolOutcome::err_class(
            ToolErrorClass::Args,
            format!("Error: offset {offset} is beyond the end of {raw_path} ({total} lines)"),
        );
    }

    let start = offset.saturating_sub(1).min(total);
    let end = (start + limit).min(total);
    let start_line = if total == 0 { 0 } else { start + 1 };
    let mut output = format!("[FILE] {raw_path} (lines {start_line}-{end} of {total})\n");
    if end < total {
        output.push_str(&format!(
            "[PARTIAL view: returned lines {start_line}-{end} of {total}. \
             Use read_file(offset={}) to continue, or search_code to locate specific lines.]\n",
            end + 1
        ));
    }
    if start < end {
        debug_assert_eq!(lines.len(), end - start);
        output.push_str(&lines.join("\n"));
        output.push('\n');
    }
    ToolOutcome::ok(output)
}

/// Bounded line-window scan. The old implementation did `read_to_string` plus
/// a full `Vec<String>` duplicate, so a 1 GiB file could allocate over 2 GiB
/// even for a 50-line view. This scans 64 KiB chunks, counts every line, and
/// retains only the requested window. A retained line keeps at most
/// `RETAIN_CAP` prefix bytes (2000 chars fit in 8000 bytes); oversized lines
/// are truncated from the prefix without ever buffering the rest. Output is
/// byte-identical to the old `trim_line` mapping.
async fn read_window(
    path: &std::path::Path,
    offset: usize,
    limit: usize,
) -> Result<(Vec<String>, usize), ReadFailure> {
    use tokio::io::AsyncReadExt;

    // 2000 chars fit in 8000 UTF-8 bytes; 16 KiB leaves room for a char
    // boundary cut. Oversized lines never buffer past this.
    const RETAIN_CAP: usize = 16 * 1024;
    const CHUNK: usize = 64 * 1024;

    let start = offset.saturating_sub(1);
    let end = start.saturating_add(limit);

    let mut file = tokio::fs::File::open(path).await.map_err(io_failure)?;
    let mut chunk = vec![0u8; CHUNK];
    // Streaming UTF-8 validation: any invalid byte anywhere is the same
    // `read_to_string` error as before. Carry holds a split multi-byte
    // sequence (at most 3 bytes) across chunk boundaries.
    let mut carry: Vec<u8> = Vec::new();
    let mut line_index = 0usize;
    let mut newline_count = 0usize;
    let mut last_was_newline = false;
    let mut file_empty = true;
    let mut retained: Vec<String> = Vec::new();
    let mut cur_buf: Vec<u8> = Vec::new();
    let mut cur_oversized = false;
    let mut cur_retained = line_index >= start && line_index < end;

    loop {
        let n = file.read(&mut chunk).await.map_err(io_failure)?;
        if n == 0 {
            break;
        }
        file_empty = false;
        let bytes = &chunk[..n];

        // Validate (carry + bytes), keeping only a possible split sequence.
        {
            let mut combined = std::mem::take(&mut carry);
            combined.extend_from_slice(bytes);
            match std::str::from_utf8(&combined) {
                Ok(_) => {}
                Err(error) => {
                    if error.error_len().is_some() {
                        return Err((
                            ToolErrorClass::Tool,
                            "stream did not contain valid UTF-8".to_string(),
                        ));
                    }
                    let valid_up_to = error.valid_up_to();
                    carry = combined[valid_up_to..].to_vec();
                }
            }
        }

        for &byte in bytes {
            if byte == b'\n' {
                newline_count += 1;
                last_was_newline = true;
                if cur_retained {
                    retained.push(decode_retained(&cur_buf, cur_oversized, RETAIN_CAP)?);
                }
                line_index += 1;
                cur_buf.clear();
                cur_oversized = false;
                cur_retained = line_index >= start && line_index < end;
            } else {
                last_was_newline = false;
                if cur_retained && !cur_oversized {
                    // `\r` is stripped at line end; keeping it here and
                    // trimming there matches `trim_line` exactly.
                    cur_buf.push(byte);
                    if cur_buf.len() > RETAIN_CAP {
                        cur_oversized = true;
                    }
                }
            }
        }
    }
    if !carry.is_empty() {
        return Err((
            ToolErrorClass::Tool,
            "stream did not contain valid UTF-8".to_string(),
        ));
    }

    let total = if file_empty {
        0
    } else if last_was_newline {
        newline_count
    } else {
        // Final line without a terminator, same as `str::lines`.
        if cur_retained {
            retained.push(decode_retained(&cur_buf, cur_oversized, RETAIN_CAP)?);
        }
        newline_count + 1
    };
    Ok((retained, total))
}

/// Decode one retained line exactly like `trim_line`: strip trailing `\r`,
/// cap at 2000 chars with the same suffix. `oversized` means only the prefix
/// was buffered; the first 2000 chars are inside it, so truncation matches.
fn decode_retained(raw: &[u8], oversized: bool, cap: usize) -> Result<String, ReadFailure> {
    // Oversized lines buffered one byte past the cap; keep only the prefix.
    // 2000 chars fit in 8000 bytes, so the truncation below never needs the
    // discarded tail.
    let mut end = if oversized {
        cap.min(raw.len())
    } else {
        raw.len()
    };
    if std::str::from_utf8(&raw[..end]).is_err() {
        match std::str::from_utf8(&raw[..end]) {
            Ok(_) => {}
            Err(error) => {
                if error.error_len().is_some() {
                    return Err((
                        ToolErrorClass::Tool,
                        "stream did not contain valid UTF-8".to_string(),
                    ));
                }
                end = error.valid_up_to();
            }
        }
    }
    let line = std::str::from_utf8(&raw[..end])
        .map_err(|_| {
            (
                ToolErrorClass::Tool,
                "stream did not contain valid UTF-8".to_string(),
            )
        })?
        .trim_end_matches('\r');
    if line.chars().count() <= MAX_LINE_CHARS {
        return Ok(line.to_string());
    }
    let mut out: String = line.chars().take(MAX_LINE_CHARS).collect();
    out.push_str("... (line truncated to 2000 chars)");
    Ok(out)
}

/// Image reads queue the file for the model's next step (tool results are
/// text-only) and return a short note. The agent loop drains the queue into
/// a real `image_url` part, so no pixels travel through chat text.
async fn read_image(
    ctx: &ToolContext,
    path: &std::path::Path,
    raw_path: &str,
    mime: &str,
) -> ToolOutcome {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) => {
            let class = if error.kind() == std::io::ErrorKind::NotFound {
                ToolErrorClass::Args
            } else {
                ToolErrorClass::Tool
            };
            return ToolOutcome::err_class(class, format!("Error: {raw_path}: {error}"));
        }
    };
    if bytes.is_empty() {
        return ToolOutcome::err_class(ToolErrorClass::Args, format!("Error: {raw_path} is empty"));
    }
    if bytes.len() > crate::images::MAX_INLINE_IMAGE_BYTES {
        return ToolOutcome::err_class(
            ToolErrorClass::Args,
            format!(
                "Error: {raw_path} is too large for inline vision ({} bytes, max {}); downscale it first",
                bytes.len(),
                crate::images::MAX_INLINE_IMAGE_BYTES
            ),
        );
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(raw_path)
        .to_string();
    crate::screen::queue_image(
        &ctx.chat_id,
        format!(
            "data:{};base64,{}",
            mime,
            base64::engine::general_purpose::STANDARD.encode(&bytes)
        ),
        name.clone(),
    );
    ToolOutcome::ok(format!(
        "[IMAGE] {raw_path} ({mime}, {} bytes) queued for the model's next step",
        bytes.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use serde_json::json;

    use crate::QuestionBroker;

    fn context(dir: &std::path::Path) -> ToolContext {
        ToolContext::new(dir, "read-test", Arc::new(QuestionBroker::new()))
    }

    async fn read_text(dir: &std::path::Path, name: &str, args: Value) -> ToolOutcome {
        let ctx = context(dir);
        read_one(&ctx, name, &args).await
    }

    #[tokio::test]
    async fn empty_file_has_zero_lines() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("empty.txt"), "").unwrap();
        let out = read_text(dir.path(), "empty.txt", json!({})).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("(lines 0-0 of 0)"), "{}", out.content);
    }

    #[tokio::test]
    async fn offset_and_limit_slice_lines() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\nfour\n").unwrap();
        let out = read_text(dir.path(), "a.txt", json!({"offset": 2, "limit": 2})).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("(lines 2-3 of 4)"), "{}", out.content);
        assert!(out.content.contains("[PARTIAL view"), "{}", out.content);
        assert!(out.content.ends_with("two\nthree\n"), "{}", out.content);
    }

    #[tokio::test]
    async fn no_trailing_newline_counts_the_last_line() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\ntwo").unwrap();
        let out = read_text(dir.path(), "a.txt", json!({})).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("(lines 1-2 of 2)"), "{}", out.content);
        assert!(out.content.ends_with("one\ntwo\n"), "{}", out.content);
    }

    #[tokio::test]
    async fn crlf_is_stripped_like_lines() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\r\ntwo\r\n").unwrap();
        let out = read_text(dir.path(), "a.txt", json!({})).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(!out.content.contains('\r'), "{}", out.content);
        assert!(out.content.contains("(lines 1-2 of 2)"), "{}", out.content);
    }

    #[tokio::test]
    async fn long_lines_are_truncated_at_2000_chars() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.txt"),
            format!("{}\nshort\n", "x".repeat(2500)),
        )
        .unwrap();
        let out = read_text(dir.path(), "a.txt", json!({})).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content.contains("... (line truncated to 2000 chars)"),
            "{}",
            out.content
        );
        assert!(out.content.contains("(lines 1-2 of 2)"), "{}", out.content);
    }

    #[tokio::test]
    async fn oversized_single_line_streams_without_buffering_it_all() {
        let dir = tempfile::tempdir().unwrap();
        // 2 MiB single line: the old `read_to_string` + `Vec<String>` kept the
        // whole line twice; the scanner keeps only the 2000-char prefix.
        std::fs::write(
            dir.path().join("big.txt"),
            format!("{}\n", "y".repeat(2_000_000)),
        )
        .unwrap();
        let out = read_text(dir.path(), "big.txt", json!({"limit": 1})).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("(lines 1-1 of 1)"), "{}", out.content);
        assert!(
            out.content.contains("... (line truncated to 2000 chars)"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn large_file_window_returns_only_the_window() {
        let dir = tempfile::tempdir().unwrap();
        let mut text = String::new();
        for index in 1..=5000 {
            text.push_str(&format!("line-{index}\n"));
        }
        std::fs::write(dir.path().join("big.txt"), &text).unwrap();
        let out = read_text(dir.path(), "big.txt", json!({"offset": 4999, "limit": 2})).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content.contains("(lines 4999-5000 of 5000)"),
            "{}",
            out.content
        );
        assert!(
            out.content.ends_with("line-4999\nline-5000\n"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn offset_past_the_end_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        let out = read_text(dir.path(), "a.txt", json!({"offset": 5})).await;
        assert!(out.is_error, "{}", out.content);
        assert!(out.content.contains("beyond the end"), "{}", out.content);
    }

    #[tokio::test]
    async fn invalid_utf8_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), [0xff, 0xfe, b'\n']).unwrap();
        let out = read_text(dir.path(), "a.txt", json!({})).await;
        assert!(out.is_error, "{}", out.content);
    }

    #[tokio::test]
    async fn missing_file_is_an_argument_failure() {
        let dir = tempfile::tempdir().unwrap();
        let out = read_text(dir.path(), "missing.txt", json!({})).await;
        assert!(out.is_error, "{}", out.content);
        assert_eq!(out.class, Some(ToolErrorClass::Args), "{}", out.content);
    }

    #[tokio::test]
    async fn offset_past_the_end_is_typed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        let out = read_text(dir.path(), "a.txt", json!({"offset": 5})).await;
        assert_eq!(out.class, Some(ToolErrorClass::Args), "{}", out.content);
    }
}
