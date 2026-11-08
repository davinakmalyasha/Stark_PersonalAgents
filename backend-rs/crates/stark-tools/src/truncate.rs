//! Tool-output truncation: Pi-class budgets with Codex-class markers.
//!
//! Limits follow Pi (`50KB / 2000 lines`, whichever hits first) and
//! Claude-class MCP budgets — deliberately generous compared to Codex's
//! `10KiB / 256 lines` default, so truncation never hides the log lines or
//! guardrails the agent needs to stay smart.
//!
//! Markers follow Codex: head + tail with the omission in the middle, carrying
//! exact omitted counts. Lines are never split, except for a single
//! pathological line which is hard-capped so one minified file cannot blow
//! the context.

/// Pi parity: byte budget per tool result.
pub const MAX_TOOL_BYTES: usize = 51_200;
/// Pi parity: line budget per tool result.
pub const MAX_TOOL_LINES: usize = 2_000;
/// Hard cap for one pathological line (minified files, giant JSON rows).
pub const MAX_LINE_CHARS: usize = 8_000;

#[allow(dead_code)]
pub struct Truncated {
    pub text: String,
    pub was_truncated: bool,
    pub total_bytes: usize,
    pub total_lines: usize,
    pub shown_head_lines: usize,
    pub shown_tail_lines: usize,
}

/// Line-aware head + tail truncation. Returns the input unchanged when it
/// fits both budgets.
pub fn truncate_tool_output(value: &str) -> Truncated {
    truncate_with_spill(value, None)
}

/// Same as [`truncate_tool_output`], but the middle marker also points at a
/// spilled full-output file (Pi spill pattern) when truncation fires.
pub fn truncate_with_spill(value: &str, spill: Option<&std::path::Path>) -> Truncated {
    let total_bytes = value.len();
    let total_lines = value.lines().count();
    if total_bytes <= MAX_TOOL_BYTES && total_lines <= MAX_TOOL_LINES {
        return Truncated {
            text: cap_long_lines(value),
            was_truncated: false,
            total_bytes,
            total_lines,
            shown_head_lines: total_lines,
            shown_tail_lines: 0,
        };
    }
    let lines: Vec<&str> = value.lines().collect();
    let half = MAX_TOOL_BYTES / 2;
    let mut head: Vec<&str> = Vec::new();
    let mut head_bytes = 0usize;
    for line in &lines {
        let capped = cap_one_line(line);
        let cost = capped.len() + 1;
        if head_bytes + cost > half && !head.is_empty() {
            break;
        }
        head_bytes += cost;
        head.push(line);
    }
    let mut tail: Vec<&str> = Vec::new();
    let mut tail_bytes = 0usize;
    for line in lines.iter().rev() {
        let capped = cap_one_line(line);
        let cost = capped.len() + 1;
        if tail_bytes + cost > half && !tail.is_empty() {
            break;
        }
        tail_bytes += cost;
        tail.push(line);
    }
    tail.reverse();
    // Head and tail must not overlap; shrink the tail when the output is
    // only slightly over budget.
    let head_count = head.len().min(lines.len());
    let mut tail_count = tail.len();
    if head_count + tail_count > lines.len() {
        tail_count = lines.len().saturating_sub(head_count);
    }
    let omitted_lines = lines.len().saturating_sub(head_count + tail_count);
    let omitted_bytes = total_bytes.saturating_sub(head_bytes + tail_bytes);
    let spill_note = spill
        .map(|path| format!(" Full output saved to {}.", path.to_string_lossy()))
        .unwrap_or_default();
    let mut out = String::new();
    for line in head.iter().take(head_count) {
        out.push_str(&cap_one_line(line));
        out.push('\n');
    }
    out.push_str(&format!(
        "[... {} lines / {} bytes omitted (showing {} head + {} tail of {} lines, {} bytes total).{spill_note} Request a narrower query or read the saved file with an offset/limit.]\n",
        omitted_lines, omitted_bytes, head_count, tail_count, total_lines, total_bytes
    ));
    for line in tail.iter().skip(tail.len().saturating_sub(tail_count)) {
        out.push_str(&cap_one_line(line));
        out.push('\n');
    }
    // Keep the trailing-newline shape of the input.
    if !value.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    Truncated {
        text: out,
        was_truncated: true,
        total_bytes,
        total_lines,
        shown_head_lines: head_count,
        shown_tail_lines: tail_count,
    }
}

fn cap_one_line(line: &str) -> String {
    if line.chars().count() <= MAX_LINE_CHARS {
        return line.to_string();
    }
    let head: String = line.chars().take(MAX_LINE_CHARS).collect();
    format!(
        "{head}\n[... {} chars omitted from one long line ...]",
        line.len().saturating_sub(MAX_LINE_CHARS)
    )
}

fn cap_long_lines(value: &str) -> String {
    if value.chars().count() <= MAX_TOOL_BYTES {
        let mut capped = false;
        let mut out = String::new();
        for line in value.lines() {
            if line.chars().count() > MAX_LINE_CHARS {
                out.push_str(&cap_one_line(line));
                capped = true;
            } else {
                out.push_str(line);
            }
            out.push('\n');
        }
        if !capped {
            return value.to_string();
        }
        if !value.ends_with('\n') && out.ends_with('\n') {
            out.pop();
        }
        return out;
    }
    value.to_string()
}

/// Writes the full (untruncated) output to the harness scratch dir so a
/// truncated marker can point the agent at it (Pi spill pattern). Returns
/// the file path, or `None` when the write fails (callers still return the
/// truncated text without a path).
pub fn spill_full_output(
    harness_root: &std::path::Path,
    stem: &str,
    contents: &str,
) -> Option<std::path::PathBuf> {
    let directory = harness_root.join(".deep_agents").join("scratch");
    if std::fs::create_dir_all(&directory).is_err() {
        return None;
    }
    let name = format!(
        "{stem}_{}.log",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    );
    let path = directory.join(name);
    std::fs::write(&path, contents).ok()?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_output_passes_through() {
        let out = truncate_tool_output("hello\nworld");
        assert!(!out.was_truncated);
        assert_eq!(out.text, "hello\nworld");
    }

    #[test]
    fn truncation_keeps_head_and_tail_with_counts() {
        let lines: Vec<String> = (0..3000).map(|i| format!("line {i}")).collect();
        let input = lines.join("\n");
        let out = truncate_tool_output(&input);
        assert!(out.was_truncated);
        assert!(out.text.starts_with("line 0\n"));
        assert!(out.text.contains("line 2999"));
        assert!(out.text.contains("3000 lines"), "{}", out.text);
        assert!(out.text.contains("head +"), "{}", out.text);
        // Marker sits between head and tail.
        let marker = out.text.find("omitted").unwrap();
        let first = out.text.find("line 0").unwrap();
        let last = out.text.rfind("line 2999").unwrap();
        assert!(first < marker && marker < last);
    }

    #[test]
    fn lines_are_never_split() {
        let input = format!("{}\nend", "x".repeat(MAX_TOOL_BYTES));
        let out = truncate_tool_output(&input);
        assert!(out.was_truncated);
        for line in out.text.lines() {
            assert!(
                line.len() <= MAX_LINE_CHARS + 80,
                "split or overlong line: {} chars",
                line.len()
            );
        }
    }

    #[test]
    fn spill_path_appears_in_marker() {
        let dir = tempfile::tempdir().unwrap();
        let lines: Vec<String> = (0..3000).map(|i| format!("row {i}")).collect();
        let input = lines.join("\n");
        let spilled = spill_full_output(dir.path(), "browser", &input).unwrap();
        assert!(spilled.is_file());
        let out = truncate_with_spill(&input, Some(&spilled));
        assert!(out.was_truncated);
        assert!(out.text.contains(&spilled.to_string_lossy().to_string()));
        assert_eq!(std::fs::read_to_string(&spilled).unwrap(), input);
    }
}
