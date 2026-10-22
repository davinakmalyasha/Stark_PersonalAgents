use crate::paths::resolve_workspace_path;
use crate::{ToolContext, ToolOutcome};

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
enum Hunk {
    AddFile {
        path: String,
        contents: String,
    },
    DeleteFile {
        path: String,
    },
    UpdateFile {
        path: String,
        move_to: Option<String>,
        chunks: Vec<UpdateChunk>,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct UpdateChunk {
    context: Option<String>,
    old_lines: Vec<String>,
    new_lines: Vec<String>,
    context_indices: Vec<(usize, usize)>,
    end_of_file: bool,
}

pub(crate) async fn apply_patch(ctx: &ToolContext, patch: &str) -> ToolOutcome {
    let hunks = match parse_patch(patch) {
        Ok(hunks) => hunks,
        Err(error) => return ToolOutcome::err(format!("Error: invalid patch: {error}")),
    };
    if hunks.is_empty() {
        return ToolOutcome::err("Error: invalid patch: no hunks");
    }

    let mut first_path = String::new();
    let mut changed_lines = 0usize;
    for hunk in hunks {
        let result = match hunk {
            Hunk::AddFile { path, contents } => add_file(ctx, &path, &contents).await,
            Hunk::DeleteFile { path } => delete_file(ctx, &path).await,
            Hunk::UpdateFile {
                path,
                move_to,
                chunks,
            } => update_file(ctx, &path, &chunks, move_to.as_deref()).await,
        };
        match result {
            Ok((path, lines)) => {
                if first_path.is_empty() {
                    first_path = path;
                }
                changed_lines += lines;
            }
            Err(error) => return ToolOutcome::err(error),
        }
    }

    ToolOutcome::ok(format!(
        "[OK] Applied diff to {first_path} ({changed_lines} lines)"
    ))
}

async fn add_file(
    ctx: &ToolContext,
    raw_path: &str,
    contents: &str,
) -> Result<(String, usize), String> {
    let path = resolve_workspace_path(&ctx.workspace, raw_path)?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| format!("Error: {raw_path}: {error}"))?;
    }
    tokio::fs::write(&path, contents)
        .await
        .map_err(|error| format!("Error: {raw_path}: {error}"))?;
    Ok((raw_path.to_string(), contents.lines().count()))
}

async fn delete_file(ctx: &ToolContext, raw_path: &str) -> Result<(String, usize), String> {
    let path = resolve_workspace_path(&ctx.workspace, raw_path)?;
    if !path.exists() {
        return Err(format!(
            "Error: Delete File failed: {raw_path} does not exist"
        ));
    }
    tokio::fs::remove_file(&path)
        .await
        .map_err(|error| format!("Error: Delete File failed for {raw_path}: {error}"))?;
    Ok((raw_path.to_string(), 0))
}

async fn update_file(
    ctx: &ToolContext,
    raw_path: &str,
    chunks: &[UpdateChunk],
    move_to: Option<&str>,
) -> Result<(String, usize), String> {
    let path = resolve_workspace_path(&ctx.workspace, raw_path)?;
    let original = tokio::fs::read_to_string(&path)
        .await
        .map_err(|error| format!("Error: Update File failed for {raw_path}: {error}"))?;
    let mut source = SourceFile::parse(&original);
    let replacements = compute_replacements(&source.line_texts(), raw_path, chunks)?;
    source.apply_replacements(&replacements);
    let updated = source.into_contents();

    if let Some(destination) = move_to {
        let destination = resolve_workspace_path(&ctx.workspace, destination)?;
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| format!("Error: {destination:?}: {error}"))?;
        }
        tokio::fs::write(&destination, &updated)
            .await
            .map_err(|error| format!("Error: Move to failed: {error}"))?;
        tokio::fs::remove_file(&path)
            .await
            .map_err(|error| format!("Error: Move to failed: {error}"))?;
    } else {
        tokio::fs::write(&path, &updated)
            .await
            .map_err(|error| format!("Error: Update File failed for {raw_path}: {error}"))?;
    }

    Ok((raw_path.to_string(), chunks.len()))
}

fn parse_patch(patch: &str) -> Result<Vec<Hunk>, String> {
    let normalized = strip_heredoc(patch);
    let trimmed = normalized.trim();
    let lines: Vec<&str> = trimmed.lines().collect();
    let lines = strip_boundaries(&lines);
    let mut index = 0;
    let mut hunks = Vec::new();

    while index < lines.len() {
        let line = lines[index].trim();
        if line.is_empty() || line == "*** Begin Patch" || line == "*** End Patch" {
            index += 1;
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Add File:") {
            let path = path.trim().to_string();
            index += 1;
            let mut body = Vec::new();
            while index < lines.len() && !lines[index].trim_start().starts_with("*** ") {
                let raw = lines[index];
                body.push(raw.strip_prefix('+').unwrap_or(raw).to_string());
                index += 1;
            }
            let mut contents = body.join("\n");
            if !contents.is_empty() {
                contents.push('\n');
            }
            hunks.push(Hunk::AddFile { path, contents });
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Delete File:") {
            hunks.push(Hunk::DeleteFile {
                path: path.trim().to_string(),
            });
            index += 1;
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Update File:") {
            let path = path.trim().to_string();
            index += 1;
            let mut move_to = None;
            let mut chunks = Vec::new();
            let mut current: Option<UpdateChunk> = None;

            while index < lines.len() {
                let raw = lines[index];
                let value = raw.trim_end();
                if let Some(destination) = value.trim_start().strip_prefix("*** Move to:") {
                    if let Some(chunk) = current.take() {
                        chunks.push(chunk);
                    }
                    move_to = Some(destination.trim().to_string());
                    index += 1;
                    continue;
                }
                if value.trim_start() == "*** End of File" {
                    if let Some(chunk) = current.as_mut() {
                        chunk.end_of_file = true;
                    }
                    index += 1;
                    continue;
                }
                if value.trim_start().starts_with("*** ") {
                    break;
                }
                index += 1;

                if let Some(context) = value.trim_start().strip_prefix("@@") {
                    if let Some(chunk) = current.take() {
                        chunks.push(chunk);
                    }
                    let context = context.trim();
                    current = Some(UpdateChunk {
                        context: if context.is_empty() {
                            None
                        } else {
                            Some(context.to_string())
                        },
                        ..UpdateChunk::default()
                    });
                    continue;
                }

                if value.is_empty() {
                    continue;
                }
                let chunk = current.get_or_insert_with(UpdateChunk::default);
                let (prefix, text) = value.split_at(1);
                match prefix {
                    " " => {
                        chunk
                            .context_indices
                            .push((chunk.old_lines.len(), chunk.new_lines.len()));
                        chunk.old_lines.push(text.to_string());
                        chunk.new_lines.push(text.to_string());
                    }
                    "-" => chunk.old_lines.push(text.to_string()),
                    "+" => chunk.new_lines.push(text.to_string()),
                    _ => return Err(format!("invalid update line: {value}")),
                }
            }

            if let Some(chunk) = current {
                chunks.push(chunk);
            }
            if chunks.is_empty() {
                return Err("Update File hunk has no chunks".to_string());
            }
            hunks.push(Hunk::UpdateFile {
                path,
                move_to,
                chunks,
            });
            continue;
        }

        return Err(format!("unknown patch marker: {line}"));
    }

    Ok(hunks)
}

fn strip_heredoc(patch: &str) -> String {
    let trimmed = patch.trim();
    if !trimmed.starts_with("<<") {
        return trimmed.to_string();
    }
    let mut lines = trimmed.lines();
    let first = lines.next().unwrap_or_default();
    if !first.contains("EOF") {
        return trimmed.to_string();
    }
    let body = lines.collect::<Vec<_>>().join("\n");
    body.trim_end_matches("EOF").trim().to_string()
}

fn strip_boundaries<'a>(lines: &'a [&'a str]) -> &'a [&'a str] {
    let mut start = 0;
    let mut end = lines.len();
    if lines
        .first()
        .is_some_and(|line| line.trim() == "*** Begin Patch")
    {
        start = 1;
    }
    if end > start && lines[end - 1].trim() == "*** End Patch" {
        end -= 1;
    }
    &lines[start..end]
}

fn compute_replacements(
    original_lines: &[String],
    path: &str,
    chunks: &[UpdateChunk],
) -> Result<Vec<(usize, usize, Vec<String>)>, String> {
    let mut replacements = Vec::new();
    let mut line_index = 0usize;

    for chunk in chunks {
        if let Some(context) = &chunk.context {
            let Some(index) = seek_sequence(
                original_lines,
                std::slice::from_ref(context),
                line_index,
                false,
            ) else {
                return Err(format!("Failed to find context '{context}' in {path}"));
            };
            line_index = index + 1;
        }

        if chunk.old_lines.is_empty() {
            replacements.push((original_lines.len(), 0, chunk.new_lines.clone()));
            continue;
        }

        let mut pattern: &[String] = &chunk.old_lines;
        let mut new_slice: &[String] = &chunk.new_lines;
        let mut found = seek_sequence(original_lines, pattern, line_index, chunk.end_of_file);

        if found.is_none() && pattern.last().is_some_and(String::is_empty) {
            pattern = &pattern[..pattern.len() - 1];
            if new_slice.last().is_some_and(String::is_empty) {
                new_slice = &new_slice[..new_slice.len() - 1];
            }
            found = seek_sequence(original_lines, pattern, line_index, chunk.end_of_file);
        }

        let Some(start) = found else {
            return Err(format!(
                "Failed to find expected lines in {path}:\n{}",
                chunk.old_lines.join("\n")
            ));
        };

        let mut old_start = 0usize;
        let mut new_start = 0usize;
        for &(old_context, new_context) in &chunk.context_indices {
            if old_context >= pattern.len() || new_context >= new_slice.len() {
                break;
            }
            if old_start != old_context || new_start != new_context {
                replacements.push((
                    start + old_start,
                    old_context - old_start,
                    new_slice[new_start..new_context].to_vec(),
                ));
            }
            old_start = old_context + 1;
            new_start = new_context + 1;
        }
        if old_start != pattern.len() || new_start != new_slice.len() {
            replacements.push((
                start + old_start,
                pattern.len() - old_start,
                new_slice[new_start..].to_vec(),
            ));
        }
        line_index = start + pattern.len();
    }

    Ok(replacements)
}

fn seek_sequence(lines: &[String], pattern: &[String], start: usize, eof: bool) -> Option<usize> {
    if pattern.is_empty() {
        return Some(start);
    }
    if pattern.len() > lines.len() {
        return None;
    }
    let search_start = if eof && lines.len() >= pattern.len() {
        (lines.len() - pattern.len()).max(start)
    } else {
        start
    };
    let last_start = lines.len().saturating_sub(pattern.len());

    for index in search_start..=last_start {
        if lines[index..index + pattern.len()] == *pattern {
            return Some(index);
        }
    }
    for index in search_start..=last_start {
        let matched = pattern
            .iter()
            .enumerate()
            .all(|(offset, line)| lines[index + offset].trim_end() == line.trim_end());
        if matched {
            return Some(index);
        }
    }
    for index in search_start..=last_start {
        let matched = pattern
            .iter()
            .enumerate()
            .all(|(offset, line)| lines[index + offset].trim() == line.trim());
        if matched {
            return Some(index);
        }
    }
    for index in search_start..=last_start {
        let matched = pattern
            .iter()
            .enumerate()
            .all(|(offset, line)| normalise(&lines[index + offset]) == normalise(line));
        if matched {
            return Some(index);
        }
    }
    None
}

fn normalise(value: &str) -> String {
    value
        .trim()
        .chars()
        .map(|character| match character {
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
            | '\u{2212}' => '-',
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            '\u{00A0}' | '\u{2002}' | '\u{2003}' | '\u{2004}' | '\u{2005}' | '\u{2006}'
            | '\u{2007}' | '\u{2008}' | '\u{2009}' | '\u{200A}' | '\u{202F}' | '\u{205F}'
            | '\u{3000}' => ' ',
            other => other,
        })
        .collect()
}

struct SourceFile {
    lines: Vec<SourceLine>,
    preferred_ending: &'static str,
}

struct SourceLine {
    text: String,
    ending: Option<&'static str>,
}

impl SourceFile {
    fn parse(contents: &str) -> Self {
        let mut lines = Vec::new();
        let mut preferred = None;
        let mut start = 0usize;
        let mut cursor = 0usize;
        let bytes = contents.as_bytes();

        while cursor < bytes.len() {
            let (ending, length) = match bytes[cursor] {
                b'\r' if bytes.get(cursor + 1) == Some(&b'\n') => ("\r\n", 2),
                b'\r' => ("\r", 1),
                b'\n' => ("\n", 1),
                _ => {
                    cursor += 1;
                    continue;
                }
            };
            preferred.get_or_insert(ending);
            lines.push(SourceLine {
                text: contents[start..cursor].to_string(),
                ending: Some(ending),
            });
            cursor += length;
            start = cursor;
        }
        if start < contents.len() {
            lines.push(SourceLine {
                text: contents[start..].to_string(),
                ending: None,
            });
        }

        Self {
            lines,
            preferred_ending: preferred.unwrap_or("\n"),
        }
    }

    fn line_texts(&self) -> Vec<String> {
        self.lines.iter().map(|line| line.text.clone()).collect()
    }

    fn apply_replacements(&mut self, replacements: &[(usize, usize, Vec<String>)]) {
        let mut source = std::mem::take(&mut self.lines).into_iter();
        let mut output = Vec::new();
        let mut source_index = 0usize;

        for (start, old_len, new_lines) in replacements {
            for line in source.by_ref().take(start.saturating_sub(source_index)) {
                output.push(line);
            }
            for _ in source.by_ref().take(*old_len) {}
            output.extend(new_lines.iter().map(|text| SourceLine {
                text: text.clone(),
                ending: Some(self.preferred_ending),
            }));
            source_index = start + old_len;
        }
        output.extend(source);
        for line in &mut output {
            line.ending.get_or_insert(self.preferred_ending);
        }
        self.lines = output;
    }

    fn into_contents(self) -> String {
        let mut contents = String::new();
        for line in self.lines {
            contents.push_str(&line.text);
            if let Some(ending) = line.ending {
                contents.push_str(ending);
            }
        }
        contents
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::QuestionBroker;

    fn context(workspace: &std::path::Path) -> ToolContext {
        ToolContext::new(workspace, "test", Arc::new(QuestionBroker::new()))
    }

    #[tokio::test]
    async fn add_overwrites_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        tokio::fs::write(dir.path().join("duplicate.txt"), "old content\n")
            .await
            .unwrap();
        let result = apply_patch(
            &ctx,
            "*** Begin Patch\n*** Add File: duplicate.txt\n+new content\n*** End Patch",
        )
        .await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(
            tokio::fs::read_to_string(dir.path().join("duplicate.txt"))
                .await
                .unwrap(),
            "new content\n"
        );
    }

    #[tokio::test]
    async fn pure_addition_appends_at_end() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        tokio::fs::write(dir.path().join("input.txt"), "line1\nline2\n")
            .await
            .unwrap();
        let result = apply_patch(
            &ctx,
            "*** Begin Patch\n*** Update File: input.txt\n@@\n+added line 1\n+added line 2\n*** End Patch",
        )
        .await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(
            tokio::fs::read_to_string(dir.path().join("input.txt"))
                .await
                .unwrap(),
            "line1\nline2\nadded line 1\nadded line 2\n"
        );
    }

    #[tokio::test]
    async fn preserves_crlf_and_mixed_line_endings() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        tokio::fs::write(dir.path().join("lines.txt"), b"one\r\ntwo\rthree\nfour\r\n")
            .await
            .unwrap();
        let result = apply_patch(
            &ctx,
            "*** Begin Patch\n*** Update File: lines.txt\n@@\n one\n two\n-three\n+THREE\n four\n*** End Patch",
        )
        .await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(
            tokio::fs::read(dir.path().join("lines.txt")).await.unwrap(),
            b"one\r\ntwo\rTHREE\r\nfour\r\n"
        );
    }

    #[tokio::test]
    async fn end_of_file_marker_matches_tail() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        tokio::fs::write(dir.path().join("tail.txt"), "first\nsecond\n")
            .await
            .unwrap();
        let result = apply_patch(
            &ctx,
            "*** Begin Patch\n*** Update File: tail.txt\n@@\n first\n-second\n+second updated\n*** End of File\n*** End Patch",
        )
        .await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(
            tokio::fs::read_to_string(dir.path().join("tail.txt"))
                .await
                .unwrap(),
            "first\nsecond updated\n"
        );
    }

    #[tokio::test]
    async fn whitespace_insensitive_context() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        tokio::fs::write(dir.path().join("ws.txt"), "  one  \n  two  \n")
            .await
            .unwrap();
        let result = apply_patch(
            &ctx,
            "*** Begin Patch\n*** Update File: ws.txt\n@@\n-one\n+ONE\n two\n*** End Patch",
        )
        .await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(
            tokio::fs::read_to_string(dir.path().join("ws.txt"))
                .await
                .unwrap(),
            "ONE\n  two  \n"
        );
    }

    #[tokio::test]
    async fn failure_after_partial_success_leaves_changes() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let result = apply_patch(
            &ctx,
            "*** Begin Patch\n*** Add File: created.txt\n+hello\n*** Update File: missing.txt\n@@\n-old\n+new\n*** End Patch",
        )
        .await;
        assert!(result.is_error);
        assert_eq!(
            tokio::fs::read_to_string(dir.path().join("created.txt"))
                .await
                .unwrap(),
            "hello\n"
        );
    }

    #[tokio::test]
    async fn codex_apply_patch_scenarios_match() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("apply_patch");
        let mut scenarios: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
            .collect();
        scenarios.sort_by_key(|entry| entry.file_name());

        for scenario in scenarios {
            let name = scenario.file_name();
            let patch = std::fs::read_to_string(scenario.path().join("patch.txt"))
                .unwrap_or_else(|_| String::new());
            let dir = tempfile::tempdir().unwrap();
            let input = scenario.path().join("input");
            if input.is_dir() {
                copy_dir(&input, dir.path());
            }
            let ctx = context(dir.path());
            let _ = apply_patch(&ctx, &patch).await;
            assert_eq!(
                snapshot(dir.path()),
                snapshot(&scenario.path().join("expected")),
                "scenario {}",
                name.to_string_lossy()
            );
        }
    }

    fn copy_dir(source: &std::path::Path, destination: &std::path::Path) {
        for entry in std::fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let target = destination.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                std::fs::create_dir_all(&target).unwrap();
                copy_dir(&entry.path(), &target);
            } else {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).unwrap();
                }
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    fn snapshot(root: &std::path::Path) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
        let mut out = std::collections::BTreeMap::new();
        if root.is_dir() {
            snapshot_into(root, root, &mut out);
        }
        out
    }

    fn snapshot_into(
        base: &std::path::Path,
        dir: &std::path::Path,
        out: &mut std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>,
    ) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                snapshot_into(base, &path, out);
            } else {
                let relative = path.strip_prefix(base).unwrap().to_path_buf();
                out.insert(relative, std::fs::read(&path).unwrap());
            }
        }
    }
}
