//! Codex-identical patch application.
//!
//! Ported from `openai/codex` `codex-rs/apply-patch` (`src/parser.rs`,
//! `src/streaming_parser.rs`, `src/file_update.rs`, `src/seek_sequence.rs`,
//! `src/text_file.rs`, `src/lib.rs`), Apache-2.0. Algorithm and error strings
//! follow Codex byte-for-byte so `apply_patch` behaves identically:
//!
//! - Markers `*** Begin Patch / Add File: / Delete File: / Update File: /
//!   Move to: / @@ / *** End of File / *** End Patch`, `*** Environment ID:`.
//! - Strict boundaries plus lenient heredoc strip (`<<EOF`, `<<'EOF'`,
//!   `<<\"EOF\"`, 4+ lines); `PARSE_IN_STRICT_MODE=false`.
//! - Update-line state machine: bare empty line is an empty context line,
//!   indented `*** ...` stays a context line (headers match at column 0
//!   inside updates), duplicate `@@` on an empty chunk errors, `Move to:`
//!   only before any chunk, post-`End of File` only blanks/`@@`.
//! - `seek_sequence` 4 passes (exact, rstrip, trim, unicode normalise),
//!   `eof` starts at `len-pat.len()`, empty pattern matches at `start`.
//! - `SourceFile` keeps `LF/CRLF/CR`, first ending wins, inserted lines use
//!   it, updates force a trailing newline.
//! - `compute_replacements` with trailing-empty sentinel retry, context-line
//!   preservation via `context_line_indices`, sorted by start index.
//! - Success summary `Success. Updated the following files:` with `A/M/D`
//!   rows in hunk order; empty patch fails `No files were modified.`.
//!
//! Stark leniency kept (fixtures `017`/`018`/`020`): whitespace-padded
//! `Begin/End` markers and hunk headers are accepted at top level, and a
//! padded `End Patch` closes an update hunk; inside updates all other
//! headers match at column 0 so indented `*** ...` lines stay context.
//!
//! Intentional Stark divergences (frozen `tool-specs.json` contract):
//! - Paths must stay relative and inside the workspace
//!   (`resolve_patch_path`); Codex resolves absolute paths via `PathUri`.
//! - `Add File` rejects an existing file (fixture `011`); Codex overwrites.
//! - All-or-nothing: every hunk is validated before any write (fixture
//!   `015`); Codex commits sequentially and reports a partial delta.
//! - Shell `argv` heredoc interception (`invocation.rs`, tree-sitter) lives
//!   in shell dispatch, not here; no `tree-sitter` dependency.
//! - No `AppliedPatchDelta` exactness tracking; tool returns text only.

use serde_json::{Value, json};

use crate::paths::resolve_workspace_path;
use crate::{ToolContext, ToolErrorClass, ToolOutcome};

pub(crate) const BEGIN_PATCH_MARKER: &str = "*** Begin Patch";
pub(crate) const END_PATCH_MARKER: &str = "*** End Patch";
pub(crate) const ADD_FILE_MARKER: &str = "*** Add File: ";
pub(crate) const DELETE_FILE_MARKER: &str = "*** Delete File: ";
pub(crate) const UPDATE_FILE_MARKER: &str = "*** Update File: ";
pub(crate) const MOVE_TO_MARKER: &str = "*** Move to: ";
pub(crate) const EOF_MARKER: &str = "*** End of File";
pub(crate) const CHANGE_CONTEXT_MARKER: &str = "@@ ";
pub(crate) const EMPTY_CHANGE_CONTEXT_MARKER: &str = "@@";
const ENVIRONMENT_ID_MARKER: &str = "*** Environment ID:";

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ParseError {
    InvalidPatch(String),
    InvalidHunk { message: String, line_number: usize },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::InvalidPatch(message) => write!(f, "Invalid patch: {message}"),
            ParseError::InvalidHunk {
                message,
                line_number,
            } => write!(f, "Invalid patch hunk on line {line_number}: {message}"),
        }
    }
}

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
        chunks: Vec<UpdateFileChunk>,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct UpdateFileChunk {
    /// Single line of context narrowing the chunk position.
    change_context: Option<String>,
    /// Contiguous block replaced with `new_lines`.
    old_lines: Vec<String>,
    new_lines: Vec<String>,
    /// Pairs of indices into `old_lines`/`new_lines` for context lines.
    context_line_indices: Vec<(usize, usize)>,
    /// `old_lines` must occur at the end of the file.
    is_end_of_file: bool,
}

impl UpdateFileChunk {
    fn push_context_line(&mut self, line: String) {
        self.context_line_indices
            .push((self.old_lines.len(), self.new_lines.len()));
        self.old_lines.push(line.clone());
        self.new_lines.push(line);
    }
}

pub(crate) async fn apply_patch(ctx: &ToolContext, patch: &str) -> ToolOutcome {
    let hunks = match parse_patch(patch) {
        Ok(hunks) => hunks,
        Err(error) => return ToolOutcome::err_class(ToolErrorClass::Args, error.to_string()),
    };
    if hunks.is_empty() {
        return ToolOutcome::err_class(ToolErrorClass::Args, "No files were modified.");
    }

    // Stark gate: every affected path must be unique and pass the edit gate.
    // `Move to:` destinations count so a rename cannot collide silently.
    let mut targets = std::collections::HashSet::new();
    for hunk in &hunks {
        for path in hunk_paths(hunk) {
            let normalized = path.trim().replace('\\', "/").to_ascii_lowercase();
            if !targets.insert(normalized) {
                return ToolOutcome::err_class(
                    ToolErrorClass::Args,
                    format!("Invalid patch: multiple operations target {path}"),
                );
            }
            let resolved = match resolve_patch_path(ctx, path) {
                Ok(path) => path,
                Err(error) => return ToolOutcome::err_class(ToolErrorClass::Args, error),
            };
            let resource = crate::paths::relative_display(&ctx.workspace, &resolved);
            if let Some(blocked) = crate::permissions::gate(ctx, "edit", &[resource], path).await {
                return blocked;
            }
        }
    }

    // Stark gate (all-or-nothing): validate every hunk before mutating
    // anything. A patch that fails in the middle must not leave earlier files
    // changed (fixture `015`; Codex commits sequentially instead).
    for hunk in &hunks {
        if let Err(error) = validate_hunk(ctx, hunk).await {
            return ToolOutcome::err_class(ToolErrorClass::Args, error);
        }
    }

    let mut added: Vec<String> = Vec::new();
    let mut modified: Vec<String> = Vec::new();
    let mut deleted: Vec<String> = Vec::new();
    let mut files_meta: Vec<Value> = Vec::new();
    for hunk in hunks {
        let result = match hunk {
            Hunk::AddFile { path, contents } => add_file(ctx, &path, &contents)
                .await
                .map(|(path, meta)| (path, 'A', meta)),
            Hunk::DeleteFile { path } => delete_file(ctx, &path)
                .await
                .map(|(path, meta)| (path, 'D', meta)),
            Hunk::UpdateFile {
                path,
                move_to,
                chunks,
            } => update_file(ctx, &path, &chunks, move_to.as_deref())
                .await
                .map(|(path, meta)| (path, 'M', meta)),
        };
        match result {
            Ok((path, kind, meta)) => {
                files_meta.push(meta);
                match kind {
                    'A' => added.push(path),
                    'D' => deleted.push(path),
                    _ => modified.push(path),
                }
            }
            Err(error) => return ToolOutcome::err_class(ToolErrorClass::Tool, error),
        }
    }

    // Codex `print_summary` order: added, modified, deleted.
    let mut summary = String::from("Success. Updated the following files:\n");
    for path in &added {
        summary.push_str(&format!("A {path}\n"));
    }
    for path in &modified {
        summary.push_str(&format!("M {path}\n"));
    }
    for path in &deleted {
        summary.push_str(&format!("D {path}\n"));
    }
    let mut outcome = ToolOutcome::ok(summary);
    if !files_meta.is_empty() {
        outcome = outcome.with_metadata(json!({"files": files_meta}));
    }
    outcome
}

async fn validate_hunk(ctx: &ToolContext, hunk: &Hunk) -> Result<(), String> {
    match hunk {
        Hunk::AddFile { path, .. } => {
            // Stark divergence: reject instead of overwriting (Codex
            // `write_file_with_missing_parent_retry` overwrites and records
            // `overwritten_content` in its delta).
            let path = resolve_patch_path(ctx, path)?;
            if path.exists() {
                return Err(format!(
                    "Error: Add File failed: {} already exists. Use Update File instead.",
                    path.display()
                ));
            }
        }
        Hunk::DeleteFile { path } => {
            let resolved = resolve_patch_path(ctx, path)?;
            if resolved.is_dir() || !resolved.exists() {
                return Err(format!("Failed to delete file {path}"));
            }
        }
        Hunk::UpdateFile { path, chunks, .. } => {
            let resolved = resolve_patch_path(ctx, path)?;
            let original = tokio::fs::read_to_string(&resolved)
                .await
                .map_err(|error| format!("Failed to read file to update {path}: {error}"))?;
            let source = SourceFile::parse(&original);
            let _ = compute_replacements(&source.line_texts(), path, chunks)?;
        }
    }
    Ok(())
}

fn hunk_paths(hunk: &Hunk) -> Vec<&str> {
    match hunk {
        Hunk::AddFile { path, .. } | Hunk::DeleteFile { path } => vec![path.as_str()],
        Hunk::UpdateFile { path, move_to, .. } => {
            let mut paths = vec![path.as_str()];
            if let Some(destination) = move_to {
                paths.push(destination.as_str());
            }
            paths
        }
    }
}

fn resolve_patch_path(ctx: &ToolContext, raw: &str) -> Result<std::path::PathBuf, String> {
    if std::path::Path::new(raw).is_absolute() {
        return Err(format!("apply_patch paths must be relative: {raw}"));
    }
    resolve_workspace_path(&ctx.workspace, raw)
}

fn diff_meta(raw_path: &str, before: &str, after: &str, status: &str) -> Value {
    let diff = similar::TextDiff::from_lines(before, after);
    let mut additions = 0usize;
    let mut deletions = 0usize;
    for change in diff.iter_all_changes() {
        match change.tag() {
            similar::ChangeTag::Insert => additions += 1,
            similar::ChangeTag::Delete => deletions += 1,
            similar::ChangeTag::Equal => {}
        }
    }
    let patch = diff
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{raw_path}"), &format!("b/{raw_path}"))
        .to_string();
    json!({
        "file": raw_path,
        "status": status,
        "additions": additions,
        "deletions": deletions,
        "patch": patch,
    })
}

async fn add_file(
    ctx: &ToolContext,
    raw_path: &str,
    contents: &str,
) -> Result<(String, Value), String> {
    let path = resolve_patch_path(ctx, raw_path)?;
    if path.exists() {
        return Err(format!(
            "Error: Add File failed: {raw_path} already exists. Use Update File instead."
        ));
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| format!("Failed to write file {raw_path}: {error}"))?;
    }
    tokio::fs::write(&path, contents)
        .await
        .map_err(|error| format!("Failed to write file {raw_path}: {error}"))?;
    Ok((
        raw_path.to_string(),
        diff_meta(raw_path, "", contents, "added"),
    ))
}

async fn delete_file(ctx: &ToolContext, raw_path: &str) -> Result<(String, Value), String> {
    let path = resolve_patch_path(ctx, raw_path)?;
    if path.is_dir() || !path.exists() {
        return Err(format!("Failed to delete file {raw_path}"));
    }
    let before = tokio::fs::read(&path)
        .await
        .map_err(|error| format!("Failed to read file to delete {raw_path}: {error}"))?;
    tokio::fs::remove_file(&path)
        .await
        .map_err(|error| format!("Failed to delete file {raw_path}: {error}"))?;
    let meta = match std::str::from_utf8(&before) {
        Ok(before) => diff_meta(raw_path, before, "", "deleted"),
        Err(_) => json!({"file": raw_path, "status": "deleted"}),
    };
    Ok((raw_path.to_string(), meta))
}

async fn update_file(
    ctx: &ToolContext,
    raw_path: &str,
    chunks: &[UpdateFileChunk],
    move_to: Option<&str>,
) -> Result<(String, Value), String> {
    let path = resolve_patch_path(ctx, raw_path)?;
    let original = tokio::fs::read_to_string(&path)
        .await
        .map_err(|error| format!("Failed to read file to update {raw_path}: {error}"))?;
    let mut source = SourceFile::parse(&original);
    let replacements = compute_replacements(&source.line_texts(), raw_path, chunks)?;
    source.apply_replacements(&replacements);
    let updated = source.into_contents();

    if let Some(destination) = move_to {
        let destination = resolve_patch_path(ctx, destination)?;
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| format!("Failed to write file {destination:?}: {error}"))?;
        }
        tokio::fs::write(&destination, &updated)
            .await
            .map_err(|error| format!("Failed to write file {destination:?}: {error}"))?;
        tokio::fs::remove_file(&path)
            .await
            .map_err(|error| format!("Failed to remove original {raw_path}: {error}"))?;
        // Codex summary spells renames by destination.
        let destination_path = destination
            .strip_prefix(&ctx.workspace)
            .map(|relative| relative.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| destination.to_string_lossy().to_string());
        return Ok((
            destination_path.clone(),
            diff_meta(&destination_path, &original, &updated, "modified"),
        ));
    }
    tokio::fs::write(&path, &updated)
        .await
        .map_err(|error| format!("Failed to write file {raw_path}: {error}"))?;

    Ok((
        raw_path.to_string(),
        diff_meta(raw_path, &original, &updated, "modified"),
    ))
}

fn parse_patch(patch: &str) -> Result<Vec<Hunk>, ParseError> {
    // Codex `parse_patch_text`: strict boundaries, else lenient heredoc.
    // `trim().lines()` mirrors Codex: trailing newlines vanish, lone `\r`
    // line endings are preserved, one trailing `\r` per line is stripped.
    let raw: Vec<&str> = patch.trim().lines().collect();
    let stripped: Vec<String> = raw
        .iter()
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect();
    let refs: Vec<&str> = stripped.iter().map(String::as_str).collect();
    let patch_lines: Vec<&str> = match check_boundaries_strict(&refs) {
        Ok(lines) => lines.to_vec(),
        Err(first_error) => match check_boundaries_lenient(&refs) {
            Ok(lines) => lines.to_vec(),
            Err(_) => return Err(first_error),
        },
    };

    let mut parser = StreamingPatchParser::default();
    for line in patch_lines {
        parser.process_line(line)?;
    }
    parser.finish()
}

fn check_boundaries_strict<'a>(lines: &'a [&'a str]) -> Result<&'a [&'a str], ParseError> {
    let (first_line, last_line) = match lines {
        [] => (None, None),
        [first] => (Some(first), Some(first)),
        [first, .., last] => (Some(first), Some(last)),
    };
    check_start_and_end_lines_strict(first_line, last_line)?;
    Ok(lines)
}

fn check_boundaries_lenient<'a>(
    original_lines: &'a [&'a str],
) -> Result<&'a [&'a str], ParseError> {
    let original_error = match check_boundaries_strict(original_lines) {
        Ok(lines) => return Ok(lines),
        Err(error) => error,
    };
    match original_lines {
        [first, .., last] => {
            if (first == &"<<EOF" || first == &"<<'EOF'" || first == &"<<\"EOF\"")
                && last.ends_with("EOF")
                && original_lines.len() >= 4
            {
                let inner = &original_lines[1..original_lines.len() - 1];
                check_boundaries_strict(inner)
            } else {
                Err(original_error)
            }
        }
        _ => Err(original_error),
    }
}

fn check_start_and_end_lines_strict(
    first_line: Option<&&str>,
    last_line: Option<&&str>,
) -> Result<(), ParseError> {
    let first_line = first_line.map(|line| line.trim());
    let last_line = last_line.map(|line| line.trim());
    match (first_line, last_line) {
        (Some(first), Some(last)) if first == BEGIN_PATCH_MARKER && last == END_PATCH_MARKER => {
            Ok(())
        }
        (Some(first), _) if first != BEGIN_PATCH_MARKER => Err(ParseError::InvalidPatch(
            "The first line of the patch must be '*** Begin Patch'".to_string(),
        )),
        _ => Err(ParseError::InvalidPatch(
            "The last line of the patch must be '*** End Patch'".to_string(),
        )),
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum StreamingParserMode {
    #[default]
    NotStarted,
    StartedPatch,
    AddFile,
    DeleteFile,
    UpdateFile {
        hunk_line_number: usize,
    },
    EndedPatch,
}

#[derive(Debug, Default, Clone)]
struct StreamingPatchParser {
    state: StreamingParserMode,
    hunks: Vec<Hunk>,
    environment_id: Option<String>,
    line_number: usize,
}

impl StreamingPatchParser {
    fn handle_hunk_headers_and_end_patch(&mut self, trimmed: &str) -> Result<bool, ParseError> {
        if matches!(self.state, StreamingParserMode::StartedPatch)
            && let Some(environment_id) = trimmed.strip_prefix(ENVIRONMENT_ID_MARKER)
        {
            if self.environment_id.is_some() {
                return Err(ParseError::InvalidPatch(
                    "apply_patch environment_id cannot be specified more than once".to_string(),
                ));
            }
            let environment_id = environment_id.trim();
            if environment_id.is_empty() {
                return Err(ParseError::InvalidPatch(
                    "apply_patch environment_id cannot be empty".to_string(),
                ));
            }
            self.environment_id = Some(environment_id.to_string());
            return Ok(true);
        }
        if trimmed == END_PATCH_MARKER {
            self.ensure_update_hunk_is_not_empty(trimmed)?;
            self.state = StreamingParserMode::EndedPatch;
            return Ok(true);
        }
        if let Some(path) = trimmed.strip_prefix(ADD_FILE_MARKER) {
            self.ensure_update_hunk_is_not_empty(trimmed)?;
            self.hunks.push(Hunk::AddFile {
                path: path.to_string(),
                contents: String::new(),
            });
            self.state = StreamingParserMode::AddFile;
            return Ok(true);
        }
        if let Some(path) = trimmed.strip_prefix(DELETE_FILE_MARKER) {
            self.ensure_update_hunk_is_not_empty(trimmed)?;
            self.hunks.push(Hunk::DeleteFile {
                path: path.to_string(),
            });
            self.state = StreamingParserMode::DeleteFile;
            return Ok(true);
        }
        if let Some(path) = trimmed.strip_prefix(UPDATE_FILE_MARKER) {
            self.ensure_update_hunk_is_not_empty(trimmed)?;
            let hunk_line_number = self.line_number;
            self.hunks.push(Hunk::UpdateFile {
                path: path.to_string(),
                move_to: None,
                chunks: Vec::new(),
            });
            self.state = StreamingParserMode::UpdateFile { hunk_line_number };
            return Ok(true);
        }
        Ok(false)
    }

    fn ensure_update_hunk_is_not_empty(&self, line: &str) -> Result<(), ParseError> {
        if let Some(Hunk::UpdateFile { path, chunks, .. }) = self.hunks.last() {
            if chunks.is_empty()
                && let StreamingParserMode::UpdateFile { hunk_line_number } = self.state
            {
                return Err(ParseError::InvalidHunk {
                    message: format!("Update file hunk for path '{path}' is empty"),
                    line_number: hunk_line_number,
                });
            }
            if chunks
                .last()
                .is_some_and(|chunk| chunk.old_lines.is_empty() && chunk.new_lines.is_empty())
            {
                if line == END_PATCH_MARKER {
                    return Err(ParseError::InvalidHunk {
                        message: "Update hunk does not contain any lines".to_string(),
                        line_number: self.line_number,
                    });
                }
                return Err(ParseError::InvalidHunk {
                    message: format!(
                        "Unexpected line found in update hunk: '{line}'. Every line should start with ' ' (context line), '+' (added line), or '-' (removed line)"
                    ),
                    line_number: self.line_number,
                });
            }
        }
        Ok(())
    }

    fn process_line(&mut self, line: &str) -> Result<(), ParseError> {
        // Codex strips one trailing `\r`; `parse_patch` already did.
        self.line_number += 1;
        let trimmed = line.trim();
        match self.state {
            StreamingParserMode::NotStarted => {
                if trimmed == BEGIN_PATCH_MARKER {
                    self.state = StreamingParserMode::StartedPatch;
                    return Ok(());
                }
                Err(ParseError::InvalidPatch(
                    "The first line of the patch must be '*** Begin Patch'".to_string(),
                ))
            }
            StreamingParserMode::StartedPatch => {
                if self.handle_hunk_headers_and_end_patch(trimmed)? {
                    return Ok(());
                }
                Err(ParseError::InvalidHunk {
                    message: format!(
                        "'{trimmed}' is not a valid hunk header. Valid hunk headers: '*** Add File: {{path}}', '*** Delete File: {{path}}', '*** Update File: {{path}}'"
                    ),
                    line_number: self.line_number,
                })
            }
            StreamingParserMode::AddFile => {
                if self.handle_hunk_headers_and_end_patch(trimmed)? {
                    return Ok(());
                }
                if let Some(line_to_add) = line.strip_prefix('+')
                    && let Some(Hunk::AddFile { contents, .. }) = self.hunks.last_mut()
                {
                    contents.push_str(line_to_add);
                    contents.push('\n');
                    return Ok(());
                }
                Err(ParseError::InvalidHunk {
                    message: format!(
                        "'{trimmed}' is not a valid hunk header. Valid hunk headers: '*** Add File: {{path}}', '*** Delete File: {{path}}', '*** Update File: {{path}}'"
                    ),
                    line_number: self.line_number,
                })
            }
            StreamingParserMode::DeleteFile => {
                if self.handle_hunk_headers_and_end_patch(trimmed)? {
                    return Ok(());
                }
                Err(ParseError::InvalidHunk {
                    message: format!(
                        "'{trimmed}' is not a valid hunk header. Valid hunk headers: '*** Add File: {{path}}', '*** Delete File: {{path}}', '*** Update File: {{path}}'"
                    ),
                    line_number: self.line_number,
                })
            }
            StreamingParserMode::UpdateFile { hunk_line_number } => {
                // Stark leniency (fixtures `017`/`018`/`020`): a padded
                // `End Patch` still closes the patch. Other headers stay
                // column-0 inside updates so indented `*** ...` lines remain
                // context lines (Codex parity).
                if line.trim() == END_PATCH_MARKER {
                    self.ensure_update_hunk_is_not_empty(END_PATCH_MARKER)?;
                    self.state = StreamingParserMode::EndedPatch;
                    return Ok(());
                }
                // Inside updates headers match at column 0 only, so indented
                // `*** ...` lines stay context lines (Codex parity).
                let update_line = line.trim_end();
                if self.handle_hunk_headers_and_end_patch(update_line)? {
                    return Ok(());
                }

                if let Some(Hunk::UpdateFile {
                    move_to, chunks, ..
                }) = self.hunks.last_mut()
                {
                    if chunks.last().is_some_and(|chunk| chunk.is_end_of_file) {
                        if update_line.is_empty() {
                            return Ok(());
                        }
                        if update_line != EMPTY_CHANGE_CONTEXT_MARKER
                            && !update_line.starts_with(CHANGE_CONTEXT_MARKER)
                        {
                            return Err(ParseError::InvalidHunk {
                                message: format!(
                                    "Expected update hunk to start with a @@ context marker, got: '{line}'"
                                ),
                                line_number: self.line_number,
                            });
                        }
                    }

                    if chunks.is_empty()
                        && move_to.is_none()
                        && let Some(move_to_path) = update_line.strip_prefix(MOVE_TO_MARKER)
                    {
                        *move_to = Some(move_to_path.to_string());
                        self.state = StreamingParserMode::UpdateFile { hunk_line_number };
                        return Ok(());
                    }

                    if (update_line == EMPTY_CHANGE_CONTEXT_MARKER
                        || update_line.starts_with(CHANGE_CONTEXT_MARKER))
                        && chunks.last().is_some_and(|chunk| {
                            chunk.old_lines.is_empty() && chunk.new_lines.is_empty()
                        })
                    {
                        return Err(ParseError::InvalidHunk {
                            message: format!(
                                "Unexpected line found in update hunk: '{line}'. Every line should start with ' ' (context line), '+' (added line), or '-' (removed line)"
                            ),
                            line_number: self.line_number,
                        });
                    }

                    if update_line == EMPTY_CHANGE_CONTEXT_MARKER {
                        chunks.push(UpdateFileChunk::default());
                        self.state = StreamingParserMode::UpdateFile { hunk_line_number };
                        return Ok(());
                    }

                    if let Some(change_context) = update_line.strip_prefix(CHANGE_CONTEXT_MARKER) {
                        chunks.push(UpdateFileChunk {
                            change_context: Some(change_context.to_string()),
                            ..UpdateFileChunk::default()
                        });
                        self.state = StreamingParserMode::UpdateFile { hunk_line_number };
                        return Ok(());
                    }

                    if update_line == EOF_MARKER {
                        if chunks.last().is_some_and(|chunk| {
                            chunk.old_lines.is_empty() && chunk.new_lines.is_empty()
                        }) {
                            return Err(ParseError::InvalidHunk {
                                message: "Update hunk does not contain any lines".to_string(),
                                line_number: self.line_number,
                            });
                        }
                        if let Some(chunk) = chunks.last_mut() {
                            chunk.is_end_of_file = true;
                        }
                        self.state = StreamingParserMode::UpdateFile { hunk_line_number };
                        return Ok(());
                    }

                    if line.is_empty() {
                        if chunks.is_empty() {
                            chunks.push(UpdateFileChunk::default());
                        }
                        if let Some(chunk) = chunks.last_mut() {
                            chunk.push_context_line(String::new());
                        }
                        self.state = StreamingParserMode::UpdateFile { hunk_line_number };
                        return Ok(());
                    }

                    if let Some(line_to_add) = line.strip_prefix(' ') {
                        if chunks.is_empty() {
                            chunks.push(UpdateFileChunk::default());
                        }
                        if let Some(chunk) = chunks.last_mut() {
                            chunk.push_context_line(line_to_add.to_string());
                        }
                        self.state = StreamingParserMode::UpdateFile { hunk_line_number };
                        return Ok(());
                    }

                    if let Some(line_to_add) = line.strip_prefix('+') {
                        if chunks.is_empty() {
                            chunks.push(UpdateFileChunk::default());
                        }
                        if let Some(chunk) = chunks.last_mut() {
                            chunk.new_lines.push(line_to_add.to_string());
                        }
                        self.state = StreamingParserMode::UpdateFile { hunk_line_number };
                        return Ok(());
                    }

                    if let Some(line_to_remove) = line.strip_prefix('-') {
                        if chunks.is_empty() {
                            chunks.push(UpdateFileChunk::default());
                        }
                        if let Some(chunk) = chunks.last_mut() {
                            chunk.old_lines.push(line_to_remove.to_string());
                        }
                        self.state = StreamingParserMode::UpdateFile { hunk_line_number };
                        return Ok(());
                    }

                    if chunks.last().is_some_and(|chunk| {
                        !chunk.old_lines.is_empty() || !chunk.new_lines.is_empty()
                    }) {
                        return Err(ParseError::InvalidHunk {
                            message: format!(
                                "Expected update hunk to start with a @@ context marker, got: '{line}'"
                            ),
                            line_number: self.line_number,
                        });
                    }
                }
                Err(ParseError::InvalidHunk {
                    message: format!(
                        "Unexpected line found in update hunk: '{line}'. Every line should start with ' ' (context line), '+' (added line), or '-' (removed line)"
                    ),
                    line_number: self.line_number,
                })
            }
            StreamingParserMode::EndedPatch => {
                if trimmed.is_empty() {
                    Ok(())
                } else {
                    Err(ParseError::InvalidPatch(
                        "The last line of the patch must be '*** End Patch'".to_string(),
                    ))
                }
            }
        }
    }

    fn finish(self) -> Result<Vec<Hunk>, ParseError> {
        if !matches!(self.state, StreamingParserMode::EndedPatch) {
            return Err(ParseError::InvalidPatch(
                "The last line of the patch must be '*** End Patch'".to_string(),
            ));
        }
        Ok(self.hunks)
    }
}

fn compute_replacements(
    original_lines: &[String],
    path: &str,
    chunks: &[UpdateFileChunk],
) -> Result<Vec<(usize, usize, Vec<String>)>, String> {
    let mut replacements = Vec::new();
    let mut line_index = 0usize;

    for chunk in chunks {
        if let Some(context) = &chunk.change_context {
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
        let mut found = seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);

        if found.is_none() && pattern.last().is_some_and(String::is_empty) {
            pattern = &pattern[..pattern.len() - 1];
            if new_slice.last().is_some_and(String::is_empty) {
                new_slice = &new_slice[..new_slice.len() - 1];
            }
            found = seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);
        }

        let Some(start) = found else {
            return Err(format!(
                "Failed to find expected lines in {path}:\n{}",
                chunk.old_lines.join("\n")
            ));
        };

        // Context lines occur on both sides; keep the original lines so exact
        // contents and terminators survive mixed line endings.
        let mut old_start = 0usize;
        let mut new_start = 0usize;
        for &(old_context, new_context) in &chunk.context_line_indices {
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

    replacements.sort_by_key(|(index, _, _)| *index);

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
    /// Splits contents into logical lines, retaining each ending. The first
    /// existing ending becomes the preferred style for inserted lines.
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

    /// Rebuilds from source-ordered, non-overlapping replacements. Unchanged
    /// lines keep their endings, inserted lines use the preferred ending, and
    /// every resulting line gets an ending (historical trailing newline).
    fn apply_replacements(&mut self, replacements: &[(usize, usize, Vec<String>)]) {
        let mut source = std::mem::take(&mut self.lines).into_iter();
        let mut output = Vec::new();
        let mut source_index = 0usize;

        for (start, old_len, new_lines) in replacements {
            debug_assert!(*start >= source_index);
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
    use pretty_assertions::assert_eq;

    fn context(workspace: &std::path::Path) -> ToolContext {
        ToolContext::new(workspace, "test", Arc::new(QuestionBroker::new()))
    }

    #[tokio::test]
    async fn add_rejects_existing_file() {
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
        assert!(result.is_error, "{}", result.content);
        assert!(
            result.content.contains("already exists"),
            "{}",
            result.content
        );
        assert_eq!(
            tokio::fs::read_to_string(dir.path().join("duplicate.txt"))
                .await
                .unwrap(),
            "old content\n"
        );
    }

    #[tokio::test]
    async fn enforces_codex_patch_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let missing = apply_patch(&ctx, "*** Add File: a.txt\n+hello\n*** End Patch").await;
        assert!(missing.is_error, "{}", missing.content);
        assert!(missing.content.contains("first line"));
        // Codex: a non-`+` line in Add File is a hunk-header error, not a
        // `must start with '+'` message.
        let no_plus = apply_patch(
            &ctx,
            "*** Begin Patch\n*** Add File: a.txt\nhello\n*** End Patch",
        )
        .await;
        assert!(no_plus.is_error, "{}", no_plus.content);
        assert!(no_plus.content.contains("not a valid hunk header"));
    }

    #[tokio::test]
    async fn rejects_duplicate_targets_and_absolute_paths() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let duplicate = apply_patch(
            &ctx,
            "*** Begin Patch\n*** Add File: a.txt\n+one\n*** Update File: a.txt\n@@\n-one\n+two\n*** End Patch",
        )
        .await;
        assert!(duplicate.is_error, "{}", duplicate.content);
        assert!(duplicate.content.contains("multiple operations"));
        let absolute = if cfg!(windows) {
            "C:/absolute.txt"
        } else {
            "/absolute.txt"
        };
        let out = apply_patch(
            &ctx,
            &format!("*** Begin Patch\n*** Add File: {absolute}\n+x\n*** End Patch"),
        )
        .await;
        assert!(out.is_error, "{}", out.content);
        assert!(out.content.contains("must be relative"));
    }

    #[tokio::test]
    async fn accepts_environment_id_marker() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let out = apply_patch(
            &ctx,
            "*** Begin Patch\n*** Environment ID: local\n*** Add File: env.txt\n+ok\n*** End Patch",
        )
        .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("env.txt")).unwrap(),
            "ok\n"
        );
    }

    #[tokio::test]
    async fn rejects_empty_and_duplicate_environment_id() {
        let empty = parse_patch(
            "*** Begin Patch\n*** Environment ID:   \n*** Add File: hello.txt\n+hello\n*** End Patch",
        );
        assert_eq!(
            empty,
            Err(ParseError::InvalidPatch(
                "apply_patch environment_id cannot be empty".to_string()
            ))
        );
        let duplicate = parse_patch(
            "*** Begin Patch\n*** Environment ID: a\n*** Environment ID: b\n*** Add File: hello.txt\n+hello\n*** End Patch",
        );
        assert_eq!(
            duplicate,
            Err(ParseError::InvalidPatch(
                "apply_patch environment_id cannot be specified more than once".to_string()
            ))
        );
    }

    #[tokio::test]
    async fn empty_patch_modifies_no_files() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let out = apply_patch(&ctx, "*** Begin Patch\n*** End Patch").await;
        assert!(out.is_error, "{}", out.content);
        assert!(out.content.contains("No files were modified"));
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
    async fn failed_patch_leaves_no_changes() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let result = apply_patch(
            &ctx,
            "*** Begin Patch\n*** Add File: created.txt\n+hello\n*** Update File: missing.txt\n@@\n-old\n+new\n*** End Patch",
        )
        .await;
        assert!(result.is_error);
        assert!(
            !dir.path().join("created.txt").exists(),
            "a failed patch must not leave earlier files behind"
        );
    }

    // --- Codex-verbatim parser coverage ---

    #[test]
    fn parse_empty_patch_yields_no_hunks() {
        assert_eq!(
            parse_patch("*** Begin Patch\n*** End Patch"),
            Ok(Vec::new())
        );
    }

    #[test]
    fn parse_bad_boundaries() {
        assert_eq!(
            parse_patch("bad"),
            Err(ParseError::InvalidPatch(
                "The first line of the patch must be '*** Begin Patch'".to_string()
            ))
        );
        assert_eq!(
            parse_patch("*** Begin Patch\nbad"),
            Err(ParseError::InvalidPatch(
                "The last line of the patch must be '*** End Patch'".to_string()
            ))
        );
    }

    #[test]
    fn parse_add_delete_update_move_combo() {
        assert_eq!(
            parse_patch(
                "*** Begin Patch\n\
                 *** Add File: path/add.py\n\
                 +abc\n\
                 +def\n\
                 *** Delete File: path/delete.py\n\
                 *** Update File: path/update.py\n\
                 *** Move to: path/update2.py\n\
                 @@ def f():\n\
                 -    pass\n\
                 +    return 123\n\
                 *** End Patch"
            ),
            Ok(vec![
                Hunk::AddFile {
                    path: "path/add.py".to_string(),
                    contents: "abc\ndef\n".to_string()
                },
                Hunk::DeleteFile {
                    path: "path/delete.py".to_string()
                },
                Hunk::UpdateFile {
                    path: "path/update.py".to_string(),
                    move_to: Some("path/update2.py".to_string()),
                    chunks: vec![UpdateFileChunk {
                        change_context: Some("def f():".to_string()),
                        old_lines: vec!["    pass".to_string()],
                        new_lines: vec!["    return 123".to_string()],
                        context_line_indices: vec![],
                        is_end_of_file: false
                    }]
                }
            ])
        );
    }

    #[test]
    fn parse_update_then_add_and_headerless_first_chunk() {
        assert_eq!(
            parse_patch(
                "*** Begin Patch\n\
                 *** Update File: file.py\n\
                 @@\n\
                 +line\n\
                 *** Add File: other.py\n\
                 +content\n\
                 *** End Patch"
            ),
            Ok(vec![
                Hunk::UpdateFile {
                    path: "file.py".to_string(),
                    move_to: None,
                    chunks: vec![UpdateFileChunk {
                        change_context: None,
                        old_lines: vec![],
                        new_lines: vec!["line".to_string()],
                        context_line_indices: vec![],
                        is_end_of_file: false
                    }],
                },
                Hunk::AddFile {
                    path: "other.py".to_string(),
                    contents: "content\n".to_string()
                }
            ])
        );
        // No explicit `@@` for the first chunk: leading ` ` line is context.
        assert_eq!(
            parse_patch(
                "*** Begin Patch\n*** Update File: file2.py\n import foo\n+bar\n*** End Patch"
            ),
            Ok(vec![Hunk::UpdateFile {
                path: "file2.py".to_string(),
                move_to: None,
                chunks: vec![UpdateFileChunk {
                    change_context: None,
                    old_lines: vec!["import foo".to_string()],
                    new_lines: vec!["import foo".to_string(), "bar".to_string()],
                    context_line_indices: vec![(0, 0)],
                    is_end_of_file: false,
                }],
            }])
        );
    }

    #[test]
    fn parse_preserves_end_of_file_marker() {
        assert_eq!(
            parse_patch(
                "*** Begin Patch\n*** Update File: file.txt\n@@\n+quux\n*** End of File\n\n*** End Patch"
            ),
            Ok(vec![Hunk::UpdateFile {
                path: "file.txt".to_string(),
                move_to: None,
                chunks: vec![UpdateFileChunk {
                    change_context: None,
                    old_lines: vec![],
                    new_lines: vec!["quux".to_string()],
                    context_line_indices: vec![],
                    is_end_of_file: true,
                }],
            }])
        );
    }

    #[test]
    fn parse_accepts_relative_and_absolute_hunk_paths() {
        let absolute = if cfg!(windows) {
            "C:/absolute-update.py"
        } else {
            "/absolute-update.py"
        };
        let parsed = parse_patch(&format!(
            "*** Begin Patch\n*** Update File: {absolute}\n@@\n-old\n+new\n*** End Patch"
        ))
        .unwrap();
        assert_eq!(
            parsed,
            vec![Hunk::UpdateFile {
                path: absolute.to_string(),
                move_to: None,
                chunks: vec![UpdateFileChunk {
                    change_context: None,
                    old_lines: vec!["old".to_string()],
                    new_lines: vec!["new".to_string()],
                    context_line_indices: vec![],
                    is_end_of_file: false
                }],
            }]
        );
    }

    #[test]
    fn parse_lenient_heredoc_wrappers() {
        let body = "*** Begin Patch\n*** Update File: f.py\n@@\n-old\n+new\n*** End Patch";
        for opener in ["<<EOF", "<<'EOF'", "<<\"EOF\""] {
            let wrapped = format!("{opener}\n{body}\nEOF\n");
            assert!(
                parse_patch(&wrapped).is_ok(),
                "opener {opener} should parse leniently"
            );
        }
        // Mismatched quotes stay strict.
        assert_eq!(
            parse_patch(&format!("<<\"EOF'\n{body}\nEOF\n")),
            Err(ParseError::InvalidPatch(
                "The first line of the patch must be '*** Begin Patch'".to_string()
            ))
        );
    }

    #[test]
    fn parse_rejects_empty_update_and_bad_headers() {
        assert_eq!(
            parse_patch("*** Begin Patch\n*** Update File: foo.txt\n*** End Patch"),
            Err(ParseError::InvalidHunk {
                message: "Update file hunk for path 'foo.txt' is empty".to_string(),
                line_number: 2,
            })
        );
        assert_eq!(
            parse_patch("*** Begin Patch\n*** Frobnicate File: foo\n*** End Patch"),
            Err(ParseError::InvalidHunk {
                message: "'*** Frobnicate File: foo' is not a valid hunk header. Valid hunk headers: '*** Add File: {path}', '*** Delete File: {path}', '*** Update File: {path}'".to_string(),
                line_number: 2,
            })
        );
        // Duplicate `@@` on an empty chunk.
        assert_eq!(
            parse_patch("*** Begin Patch\n*** Update File: f.txt\n@@\n@@\n*** End Patch"),
            Err(ParseError::InvalidHunk {
                message: "Unexpected line found in update hunk: '@@'. Every line should start with ' ' (context line), '+' (added line), or '-' (removed line)".to_string(),
                line_number: 4,
            })
        );
    }

    #[test]
    fn parse_keeps_indented_markers_as_context() {
        assert_eq!(
            parse_patch(
                "*** Begin Patch\n*** Update File: a.txt\n@@\n-old a\n+new a\n *** Update File: b.txt\n@@\n-old b\n+new b\n*** End Patch"
            ),
            Ok(vec![Hunk::UpdateFile {
                path: "a.txt".to_string(),
                move_to: None,
                chunks: vec![
                    UpdateFileChunk {
                        change_context: None,
                        old_lines: vec!["old a".to_string(), "*** Update File: b.txt".to_string()],
                        new_lines: vec!["new a".to_string(), "*** Update File: b.txt".to_string()],
                        context_line_indices: vec![(1, 1)],
                        is_end_of_file: false,
                    },
                    UpdateFileChunk {
                        change_context: None,
                        old_lines: vec!["old b".to_string()],
                        new_lines: vec!["new b".to_string()],
                        context_line_indices: vec![],
                        is_end_of_file: false,
                    },
                ],
            }])
        );
    }

    #[test]
    fn parse_preserves_bare_empty_update_lines() {
        assert_eq!(
            parse_patch(
                "*** Begin Patch\n*** Update File: file.txt\n@@\n context before\n\n context after\n*** End Patch"
            ),
            Ok(vec![Hunk::UpdateFile {
                path: "file.txt".to_string(),
                move_to: None,
                chunks: vec![UpdateFileChunk {
                    change_context: None,
                    old_lines: vec![
                        "context before".to_string(),
                        String::new(),
                        "context after".to_string(),
                    ],
                    new_lines: vec![
                        "context before".to_string(),
                        String::new(),
                        "context after".to_string(),
                    ],
                    context_line_indices: vec![(0, 0), (1, 1), (2, 2)],
                    is_end_of_file: false,
                }],
            }])
        );
    }

    // --- Codex-verbatim matcher/file coverage ---

    #[test]
    fn seek_sequence_passes() {
        let lines = ["foo", "bar", "baz"]
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let pattern = ["bar", "baz"]
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(1));
        // Trailing whitespace ignored on second pass.
        let lines = ["foo   ".to_string(), "bar\t\t".to_string()];
        let pattern = ["foo".to_string(), "bar".to_string()];
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
        // Pattern longer than input returns None instead of panicking.
        assert_eq!(
            seek_sequence(
                &["just one line".to_string()],
                &["too".to_string(), "many".to_string(), "lines".to_string()],
                0,
                false
            ),
            None
        );
    }

    #[tokio::test]
    async fn update_line_with_unicode_dash() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let path = dir.path().join("unicode.py");
        let original = "import asyncio  # local import \u{2013} avoids top\u{2011}level dep\n";
        tokio::fs::write(&path, original).await.unwrap();
        let result = apply_patch(
            &ctx,
            "*** Begin Patch\n*** Update File: unicode.py\n@@\n-import asyncio  # local import - avoids top-level dep\n+import asyncio  # HELLO\n*** End Patch",
        )
        .await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(
            tokio::fs::read_to_string(&path).await.unwrap(),
            "import asyncio  # HELLO\n"
        );
    }

    #[tokio::test]
    async fn overlapping_end_of_file_chunks_fail_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        tokio::fs::write(dir.path().join("overlapping.txt"), "one\n")
            .await
            .unwrap();
        let result = apply_patch(
            &ctx,
            "*** Begin Patch\n*** Update File: overlapping.txt\n@@\n-one\n+first\n@@\n-one\n+second\n*** End of File\n*** End Patch",
        )
        .await;
        assert!(result.is_error, "{}", result.content);
        assert!(result.content.contains("Failed to find expected lines"));
        // All-or-nothing: the first chunk must not commit either.
        assert_eq!(
            tokio::fs::read_to_string(dir.path().join("overlapping.txt"))
                .await
                .unwrap(),
            "one\n"
        );
    }

    #[tokio::test]
    async fn reports_missing_context_and_delete() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        tokio::fs::write(dir.path().join("modify.txt"), "line1\nline2\n")
            .await
            .unwrap();
        let missing = apply_patch(
            &ctx,
            "*** Begin Patch\n*** Update File: modify.txt\n@@\n-missing\n+changed\n*** End Patch",
        )
        .await;
        assert!(missing.is_error);
        assert!(missing.content.contains("Failed to find expected lines"));
        assert_eq!(
            tokio::fs::read_to_string(dir.path().join("modify.txt"))
                .await
                .unwrap(),
            "line1\nline2\n"
        );
        let delete = apply_patch(
            &ctx,
            "*** Begin Patch\n*** Delete File: missing.txt\n*** End Patch",
        )
        .await;
        assert!(delete.is_error);
        assert!(delete.content.contains("Failed to delete file"));
    }

    #[tokio::test]
    async fn success_summary_lists_files_codex_style() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        tokio::fs::write(dir.path().join("modify.txt"), "line1\nline2\n")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("delete.txt"), "obsolete\n")
            .await
            .unwrap();
        let out = apply_patch(
            &ctx,
            "*** Begin Patch\n*** Add File: nested/new.txt\n+created\n*** Delete File: delete.txt\n*** Update File: modify.txt\n@@\n-line2\n+changed\n*** End Patch",
        )
        .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(
            out.content,
            "Success. Updated the following files:\nA nested/new.txt\nM modify.txt\nD delete.txt\n"
        );
    }

    #[tokio::test]
    async fn move_reports_destination_and_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        tokio::fs::create_dir_all(dir.path().join("old"))
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("old/name.txt"), "from\n")
            .await
            .unwrap();
        tokio::fs::create_dir_all(dir.path().join("renamed/dir"))
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("renamed/dir/name.txt"), "existing\n")
            .await
            .unwrap();
        let out = apply_patch(
            &ctx,
            "*** Begin Patch\n*** Update File: old/name.txt\n*** Move to: renamed/dir/name.txt\n@@\n-from\n+new\n*** End Patch",
        )
        .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("M renamed/dir/name.txt"));
        assert!(!dir.path().join("old/name.txt").exists());
        assert_eq!(
            tokio::fs::read_to_string(dir.path().join("renamed/dir/name.txt"))
                .await
                .unwrap(),
            "new\n"
        );
    }

    #[tokio::test]
    async fn apply_patch_scenarios_match() {
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
