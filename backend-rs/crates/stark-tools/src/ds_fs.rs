//! DeepSeek Harness-compatible filesystem tools: read, write, edit, glob,
//! and grep. `read` handles text and images. The old read_file/edit_file
//! pair remains available to agents that have not migrated.
//!
//! `glob` file discovery and `grep` content search follow `openai/codex`
//! `codex-rs/file-search` walker semantics (Apache-2.0, see `search.rs`):
//! `WalkBuilder` with `threads(2)`, `hidden(false)`, `follow_links(true)`,
//! `require_git(true)`. `glob` includes hidden/ignored files per the frozen
//! contract (`respect_gitignore=false`); `grep` respects gitignore
//! (`respect_gitignore=true`). Enumeration comes from the warm
//! `search::warm_entries` index (re-walked only on FS change); the Stark
//! `skip_dir_name` denylist is applied on top. `glob` ranks with the same
//! nucleo fuzzy scorer as Codex (score desc, path asc). `grep` prefers the
//! `rg` binary when present (Codex search provider) and falls back to the
//! same walker + ripgrep-syntax `regex` otherwise.

use std::collections::BTreeMap;
use std::num::NonZero;
use std::path::{Path, PathBuf};

use globset::Glob;
use nucleo::{
    Config, Matcher,
    pattern::{AtomKind, CaseMatching, Normalization, Pattern},
};
use regex::RegexBuilder;
use serde_json::Value;

use crate::paths::{relative_display, resolve_workspace_path};
use crate::{ToolContext, ToolErrorClass, ToolOutcome};

const DEFAULT_READ_LIMIT: usize = 2000;
const MAX_READ_LIMIT: usize = 10_000;
const MAX_LINE_CHARS: usize = 2000;
const GLOB_LIMIT: usize = 100;
const GREP_LIMIT: usize = 250;

fn resolve_in(ctx: &ToolContext, base: &Path, raw: &str) -> Result<PathBuf, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("path is empty".to_string());
    }
    let candidate = Path::new(raw);
    let root = if base == ctx.workspace {
        ctx.current_dir()
    } else {
        base.to_path_buf()
    };
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    resolve_workspace_path(base, &joined.to_string_lossy())
}

/// Resolve an optional `workspace`/`workspace_id` argument for read-only
/// cross-workspace inspection. Writable agents stay inside their own
/// workspace; the Orchestrator (and other read-only scopes) may read any
/// registered workspace by id or exact name.
fn read_base(ctx: &ToolContext, args: &Value) -> Result<PathBuf, String> {
    let raw = args
        .get("workspace")
        .or_else(|| args.get("workspace_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(raw) = raw else {
        return Ok(ctx.workspace.clone());
    };
    if ctx.access != "read" {
        return Err(
            "workspace override is only available to read-only agents; use your current workspace"
                .to_string(),
        );
    }
    let registry = stark_store::WorkspaceRegistry::new(&ctx.harness_root);
    if let Some(record) = registry.get(raw) {
        return Ok(PathBuf::from(record.path));
    }
    let records = registry.list();
    let matches: Vec<_> = records
        .iter()
        .filter(|record| record.name.eq_ignore_ascii_case(raw))
        .collect();
    match matches.as_slice() {
        [record] => Ok(PathBuf::from(&record.path)),
        [] => Err(format!(
            "unknown workspace '{raw}'. Use workspace action=\"list\" to see registered ids and names."
        )),
        _ => Err(format!(
            "workspace name '{raw}' is ambiguous. Retry with an exact id."
        )),
    }
}

fn trim_line(line: &str) -> String {
    let line = line.trim_end_matches('\r');
    if line.chars().count() <= MAX_LINE_CHARS {
        return line.to_string();
    }
    let mut out: String = line.chars().take(MAX_LINE_CHARS).collect();
    out.push_str("... (line truncated to 2000 chars)");
    out
}

pub async fn read(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let Some(raw) = args.get("file_path").and_then(Value::as_str) else {
        return ToolOutcome::err_class(ToolErrorClass::Args, "Error: read requires file_path");
    };
    let base = match read_base(ctx, args) {
        Ok(base) => base,
        Err(error) => {
            return ToolOutcome::err_class(ToolErrorClass::Args, format!("Error: {error}"));
        }
    };
    let path = match resolve_in(ctx, &base, raw) {
        Ok(path) => path,
        Err(error) => {
            return ToolOutcome::err_class(ToolErrorClass::Args, format!("Error: {error}"));
        }
    };
    let resource = crate::paths::relative_display(&base, &path);
    if let Some(blocked) = crate::permissions::gate(ctx, "read", &[resource], raw).await {
        return blocked;
    }
    if path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| crate::images::mime_for_ext(ext).is_some())
    {
        let forwarded = serde_json::json!({"file_path": path.to_string_lossy()});
        // `read_file` resolves against ctx.workspace; for a cross-workspace
        // read, give it a context rooted at the selected workspace.
        let mut image_ctx = ctx.clone();
        image_ctx.workspace = base.clone();
        image_ctx.cwd = std::sync::Arc::new(std::sync::Mutex::new(base.clone()));
        return crate::read::read_file(&image_ctx, &forwarded).await;
    }
    if let Ok(metadata) = tokio::fs::metadata(&path).await
        && metadata.len() > 10 * 1024 * 1024
    {
        return ToolOutcome::err_class(
            ToolErrorClass::Args,
            format!(
                "Error: {raw} is too large to read ({:.1} MB). Use grep/glob to find the relevant section, or read it with offset/limit.",
                metadata.len() as f64 / (1024.0 * 1024.0)
            ),
        );
    }
    let content = match tokio::fs::read_to_string(&path).await {
        Ok(content) => content,
        Err(error) => {
            let class = if error.kind() == std::io::ErrorKind::NotFound {
                ToolErrorClass::Args
            } else {
                ToolErrorClass::Tool
            };
            return ToolOutcome::err_class(class, format!("Error: {raw}: {error}"));
        }
    };
    ctx.mark_read(path.clone());

    let lines: Vec<String> = content.lines().map(trim_line).collect();
    let total = lines.len();
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
        .unwrap_or(DEFAULT_READ_LIMIT)
        .min(MAX_READ_LIMIT);

    if total > 0 && offset > total {
        return ToolOutcome::err_class(
            ToolErrorClass::Args,
            format!("Error: offset {offset} is beyond the end of {raw} ({total} lines)"),
        );
    }
    let start = offset.saturating_sub(1).min(total);
    let end = (start + limit).min(total);
    let mut output = format!("[FILE] {raw} (lines {}-{} of {total})\n", start + 1, end);
    if end < total {
        output.push_str(&format!(
            "[PARTIAL view: returned lines {}-{} of {total}. Use read(offset={}) to continue.]\n",
            start + 1,
            end,
            end + 1
        ));
    }
    for (index, line) in lines[start..end].iter().enumerate() {
        output.push_str(&format!("{}\t{}\n", start + index + 1, line));
    }
    ToolOutcome::ok(output)
}

pub async fn edit(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let Some(raw) = args.get("file_path").and_then(Value::as_str) else {
        return ToolOutcome::err_class(ToolErrorClass::Args, "Error: edit requires file_path");
    };
    let Some(old_string) = args.get("old_string").and_then(Value::as_str) else {
        return ToolOutcome::err_class(ToolErrorClass::Args, "Error: edit requires old_string");
    };
    let Some(new_string) = args.get("new_string").and_then(Value::as_str) else {
        return ToolOutcome::err_class(ToolErrorClass::Args, "Error: edit requires new_string");
    };
    if old_string.is_empty() {
        return ToolOutcome::err_class(ToolErrorClass::Args, "Error: old_string must not be empty");
    }
    let path = match resolve_in(ctx, &ctx.workspace, raw) {
        Ok(path) => path,
        Err(error) => {
            return ToolOutcome::err_class(ToolErrorClass::Args, format!("Error: {error}"));
        }
    };
    let resource = crate::paths::relative_display(&ctx.workspace, &path);
    if let Some(blocked) = crate::permissions::gate(ctx, "edit", &[resource], raw).await {
        return blocked;
    }
    if !ctx.was_read(&path) {
        return ToolOutcome::err_class(
            ToolErrorClass::Args,
            format!("Error: read {raw} before editing it"),
        );
    }
    let content = match tokio::fs::read_to_string(&path).await {
        Ok(content) => content,
        Err(error) => {
            let class = if error.kind() == std::io::ErrorKind::NotFound {
                ToolErrorClass::Args
            } else {
                ToolErrorClass::Tool
            };
            return ToolOutcome::err_class(class, format!("Error: {raw}: {error}"));
        }
    };
    let replace_all = args
        .get("replace_all")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let count = content.matches(old_string).count();
    if count == 0 {
        return ToolOutcome::err_class(
            ToolErrorClass::Args,
            format!("Error: old_string not found in {raw}. The text must match exactly."),
        );
    }
    if count > 1 && !replace_all {
        return ToolOutcome::err_class(
            ToolErrorClass::Args,
            format!(
                "Error: old_string matches {count} times in {raw}. Include more context or set replace_all=true."
            ),
        );
    }
    let updated = if replace_all {
        content.replace(old_string, new_string)
    } else {
        content.replacen(old_string, new_string, 1)
    };
    if let Err(error) = tokio::fs::write(&path, updated).await {
        return ToolOutcome::err_class(ToolErrorClass::Tool, format!("Error: {raw}: {error}"));
    }
    ToolOutcome::ok(format!(
        "[OK] Edited {raw} ({} replacement{})",
        count,
        if count == 1 { "" } else { "s" }
    ))
}

/// Stark's explosion guard: the denylist from the old `codex_walk` prune.
/// Directory entries with these names (and everything beneath them) are
/// hidden from `glob`/`grep`, so `target/node_modules/.git` never explode a
/// result set. The enumeration itself now comes from the warm
/// `search::warm_entries` index (same Codex walker flags:
/// `threads(2)`, `hidden(false)`, `follow_links(true)`,
/// `require_git(true)`), filtered here per consumer.
fn skip_dir_name(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | "node_modules"
            | "target"
            | "venv"
            | ".venv"
            | ".deep_agents"
            | "dist"
            | "build"
            | "__pycache__"
            | ".next"
            | ".turbo"
    )
}

/// Mirror of the old `filter_entry` prune for cached entries: a denied
/// directory hides its whole subtree. Files are checked on proper ancestor
/// components only, so a top-level *file* named e.g. `target` still shows
/// (the cold walker only pruned directories).
fn under_denylist(relative: &str, is_dir: bool) -> bool {
    if relative.is_empty() {
        return false;
    }
    let mut parts: Vec<&str> = relative.split(['/', '\\']).collect();
    if !is_dir {
        parts.pop();
    }
    parts.iter().any(|part| skip_dir_name(part))
}

/// Warm file list for `root`: `(root-relative path, is_dir)` pairs from
/// `search::warm_entries` (blocking walk on a cold/dirty slot, so run it in
/// `spawn_blocking`).
async fn warm_list(
    root: &Path,
    respect_ignore: bool,
    tool: &str,
) -> Result<Vec<(String, bool)>, ToolOutcome> {
    let root_clone = root.to_path_buf();
    match tokio::task::spawn_blocking(move || {
        crate::search::warm_entries(&root_clone, &[], respect_ignore, 2)
    })
    .await
    {
        Ok(entries) => Ok(entries
            .into_iter()
            .map(|(path, match_type)| (path, match_type == crate::search::MatchType::Directory))
            .collect()),
        Err(error) => Err(ToolOutcome::err_class(
            ToolErrorClass::Transport,
            format!("Error: {tool} failed: {error}"),
        )),
    }
}

fn scratch_path(ctx: &ToolContext, prefix: &str) -> PathBuf {
    let stamp = chrono::Utc::now().timestamp_millis();
    ctx.harness_root
        .join(".deep_agents")
        .join("scratch")
        .join(format!("{prefix}-{stamp}.txt"))
}

async fn write_scratch(ctx: &ToolContext, prefix: &str, content: &str) -> Option<String> {
    // Read-only agents must not write, even scratch files.
    if ctx.access == "read" {
        return None;
    }
    let path = scratch_path(ctx, prefix);
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    tokio::fs::write(&path, content).await.ok()?;
    Some(path.to_string_lossy().to_string())
}

fn has_glob_meta(pattern: &str) -> bool {
    pattern.contains(['*', '?', '[', '{'])
}

/// Strip glob metacharacters for fuzzy ranking: `**/*.ts` -> `ts`.
fn fuzzy_core(pattern: &str) -> String {
    pattern
        .chars()
        .filter(|c| !matches!(c, '*' | '?' | '[' | ']' | '{' | '}' | '!' | ',' | '+' | '@'))
        .collect::<String>()
        .replace(['/', '\\'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Score one haystack with Codex nucleo settings. `None` = no fuzzy match.
fn nucleo_score(needle: &str, haystack: &str) -> Option<u32> {
    if needle.trim().is_empty() {
        return Some(0);
    }
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let pattern = Pattern::new(
        needle,
        CaseMatching::Ignore,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );
    let mut buf = Vec::new();
    let hay = nucleo::Utf32Str::new(haystack, &mut buf);
    pattern.score(hay, &mut matcher)
}

pub async fn glob(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let Some(pattern) = args.get("pattern").and_then(Value::as_str) else {
        return ToolOutcome::err_class(ToolErrorClass::Args, "Error: glob requires pattern");
    };
    if let Some(blocked) =
        crate::permissions::gate(ctx, "glob", &[pattern.to_string()], pattern).await
    {
        return blocked;
    }
    let base = match read_base(ctx, args) {
        Ok(base) => base,
        Err(error) => {
            return ToolOutcome::err_class(ToolErrorClass::Args, format!("Error: {error}"));
        }
    };
    let raw_root = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let root = match resolve_in(ctx, &base, raw_root) {
        Ok(path) => path,
        Err(error) => {
            return ToolOutcome::err_class(ToolErrorClass::Args, format!("Error: {error}"));
        }
    };

    // Pure fuzzy query (no glob metacharacters): delegate byte-for-byte to
    // Codex `search::run` (ignore + nucleo, basename bonus, score desc/path
    // asc). Contract still promises files-only, so directories are filtered.
    if !has_glob_meta(pattern) {
        let limit = NonZero::new(GLOB_LIMIT).expect("GLOB_LIMIT non-zero");
        let options = crate::search::FileSearchOptions {
            limit,
            exclude: Vec::new(),
            threads: NonZero::new(2).expect("2 non-zero"),
            compute_indices: false,
            respect_gitignore: false,
        };
        let root_clone = root.clone();
        let pattern_owned = pattern.to_string();
        let base_clone = base.clone();
        let run_result = tokio::task::spawn_blocking(move || {
            crate::search::run_warm(&pattern_owned, vec![root_clone], options, None)
        })
        .await;
        let results = match run_result {
            Ok(Ok(results)) => results,
            Ok(Err(error)) => {
                return ToolOutcome::err_class(
                    ToolErrorClass::Transport,
                    format!("Error: glob failed: {error}"),
                );
            }
            Err(error) => {
                return ToolOutcome::err_class(
                    ToolErrorClass::Transport,
                    format!("Error: glob failed: {error}"),
                );
            }
        };
        let mut files: Vec<String> = results
            .matches
            .iter()
            .filter(|m| m.match_type == crate::search::MatchType::File)
            .map(|m| relative_display(&base_clone, &m.full_path()))
            .collect();
        // `search::run` already returns score-desc/path-asc; keep that order.
        files.truncate(GLOB_LIMIT);
        if files.is_empty() {
            return ToolOutcome::ok(format!("No files found matching: {pattern}"));
        }
        let mut output = files.join("\n");
        if results.total_match_count > files.len()
            && let Some(path) = write_scratch(ctx, "glob", &files.join("\n")).await
        {
            output.push_str(&format!(
                "\n\n[TRUNCATED] {} files matched; complete list saved to {path}",
                results.total_match_count
            ));
        }
        return ToolOutcome::ok(output);
    }

    let anchored = pattern.contains('/') || pattern.contains('\\');
    let matcher = match Glob::new(pattern) {
        Ok(glob) => glob.compile_matcher(),
        Err(error) => {
            return ToolOutcome::err_class(
                ToolErrorClass::Args,
                format!("Error: invalid glob: {error}"),
            );
        }
    };
    // Glob-syntax path: warm index (Codex walker flags, hidden+ignored
    // included) + Glob filter + nucleo fuzzy rank over the literal core
    // (`**/*.ts` ranks by `ts`).
    let core = fuzzy_core(pattern);
    let mut scored: Vec<(u32, String)> = Vec::new();
    let cached = match warm_list(&root, false, "glob").await {
        Ok(entries) => entries,
        Err(outcome) => return outcome,
    };
    for (rel_path, is_dir) in &cached {
        if under_denylist(rel_path, *is_dir) || *is_dir {
            continue;
        }
        let abs = root.join(rel_path);
        let rel = relative_display(&base, &abs);
        let matched = if anchored {
            matcher.is_match(&rel)
        } else {
            matcher.is_match(
                abs.file_name()
                    .map(|name| name.to_string_lossy())
                    .as_deref()
                    .unwrap_or(""),
            )
        };
        if !matched {
            continue;
        }
        let score = nucleo_score(&core, &rel).unwrap_or(0);
        scored.push((score, rel));
    }
    scored.sort_by(crate::search::cmp_by_score_desc_then_path_asc(
        |t: &(u32, String)| t.0,
        |t: &(u32, String)| t.1.as_str(),
    ));
    let total = scored.len();
    let shown: Vec<String> = scored
        .iter()
        .take(GLOB_LIMIT)
        .map(|(_, rel)| rel.clone())
        .collect();
    if total == 0 {
        return ToolOutcome::ok(format!("No files found matching: {pattern}"));
    }
    let mut output = shown.join("\n");
    if total > GLOB_LIMIT {
        let all: Vec<String> = scored.iter().map(|(_, rel)| rel.clone()).collect();
        if let Some(path) = write_scratch(ctx, "glob", &all.join("\n")).await {
            output.push_str(&format!(
                "\n\n[TRUNCATED] {total} files matched; complete list saved to {path}"
            ));
        } else {
            output.push_str(&format!("\n\n[TRUNCATED] {total} files matched"));
        }
    }
    ToolOutcome::ok(output)
}

/// Try the `rg` binary first (Codex search provider: ripgrep with gitignore
/// semantics). Returns `Some` when `rg` handled the query (match, no-match,
/// or hard error); `None` when `rg` is missing so the caller falls back to
/// the walker. Flags mirror Codex walker parity:
/// `hidden(false)` -> `--hidden`, `follow_links(true)` -> `--follow`,
/// `require_git(true)` + respect gitignore -> default `rg` gitignore handling
/// (no `--no-ignore`), exit 1 = neutral no-match, 30s timeout.
async fn try_rg_grep(
    ctx: &ToolContext,
    base: &Path,
    root: &Path,
    pattern: &str,
    include: Option<&str>,
    literal: bool,
    case_sensitive: bool,
) -> Option<ToolOutcome> {
    let root_is_file = root.is_file();
    let (current_dir, target): (&Path, String) = if root_is_file {
        match (root.parent(), root.file_name()) {
            (Some(parent), Some(name)) => (parent, name.to_string_lossy().into_owned()),
            _ => (root, ".".to_string()),
        }
    } else {
        (root, ".".to_string())
    };
    let mut cmd = tokio::process::Command::new("rg");
    cmd.arg("--line-number")
        .arg("--no-heading")
        .arg("--hidden")
        .arg("--follow")
        .arg("--no-messages")
        .arg("--max-columns")
        .arg(MAX_LINE_CHARS.to_string())
        .arg("--regexp")
        .arg(pattern)
        .current_dir(current_dir);
    if literal {
        cmd.arg("--fixed-strings");
    }
    if !case_sensitive {
        cmd.arg("--ignore-case");
    } else {
        cmd.arg("--case-sensitive");
    }
    if let Some(include) = include {
        // Codex/Windows quoting: pass `-g <glob>` as separate args, never a
        // combined positional glob (avoids `os error 123` on PowerShell).
        cmd.arg("--glob").arg(include);
    }
    // Stable, parseable output: `path:line: text` with forward slashes.
    cmd.arg("--").arg(&target);
    // Stream matches instead of buffering `rg` stdout: a pathological search
    // (huge repo, very common token) used to allocate the entire match set
    // before truncation. Lines are parsed as they arrive and the child is
    // killed once GREP_LIMIT is reached, so memory stays bounded.
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            return Some(ToolOutcome::err_class(
                ToolErrorClass::Transport,
                format!("Error: grep failed: {error}"),
            ));
        }
    };
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let stderr_task = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let reader = tokio::io::BufReader::new(stderr);
        let mut buf = Vec::new();
        let _ = reader.take(64 * 1024).read_to_end(&mut buf).await;
        buf
    });
    use tokio::io::AsyncBufReadExt;

    let mut lines = tokio::io::BufReader::new(stdout).lines();
    let mut hits: BTreeMap<String, Vec<(usize, String)>> = BTreeMap::new();
    let mut total = 0usize;
    // When `root` is a file, `rg` paths are relative to its parent.
    let join_base: &Path = if root_is_file { current_dir } else { root };
    let mut limit_hit = false;
    let read = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while let Some(line) = lines.next_line().await? {
            // `path:line: text` — path may contain `:` on Windows (`C:\...`),
            // so split from the right for the line number.
            let mut parts = line.splitn(3, ':');
            let (Some(path_part), Some(line_part), Some(text)) =
                (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let Ok(line_no) = line_part.parse::<usize>() else {
                // Windows absolute path swallowed the first split (`C`); retry
                // by finding the first `:digits:` pair.
                let Some((p, l, t)) = split_windows_rg_line(&line) else {
                    continue;
                };
                push_hit(ctx, base, join_base, &mut hits, &mut total, p, l, t);
                if total >= GREP_LIMIT {
                    limit_hit = true;
                    let _ = child.kill().await;
                    break;
                }
                continue;
            };
            push_hit(
                ctx, base, join_base, &mut hits, &mut total, path_part, line_no, text,
            );
            if total >= GREP_LIMIT {
                limit_hit = true;
                let _ = child.kill().await;
                break;
            }
        }
        Ok::<(), std::io::Error>(())
    })
    .await;
    let stderr = stderr_task.await.unwrap_or_default();
    match read {
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Some(ToolOutcome::err_class(
                ToolErrorClass::Timeout,
                "Error: grep timed out after 30s. Narrow path/include.",
            ));
        }
        Ok(Err(error)) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Some(ToolOutcome::err_class(
                ToolErrorClass::Transport,
                format!("Error: grep failed: {error}"),
            ));
        }
        Ok(Ok(())) => {}
    }
    if limit_hit {
        // The child was killed inside the read loop; reap it so no zombie
        // outlives the tool call.
        let _ = child.wait().await;
    }
    if !limit_hit {
        match child
            .wait()
            .await
            .map(|status| status.code())
            .unwrap_or(None)
        {
            Some(0) => {}
            // ripgrep exit 1 = no matches (neutral, not a failure).
            Some(1) => {
                return Some(ToolOutcome::ok(format!(
                    "No matches found for pattern: {pattern}"
                )));
            }
            Some(code) => {
                return Some(ToolOutcome::err_class(
                    ToolErrorClass::Args,
                    format!(
                        "Error: grep failed (rg exit {code}): {}",
                        String::from_utf8_lossy(&stderr).trim()
                    ),
                ));
            }
            None => {
                return Some(ToolOutcome::err_class(
                    ToolErrorClass::Tool,
                    "Error: grep failed: signal",
                ));
            }
        }
    }
    if total == 0 {
        return Some(ToolOutcome::ok(format!(
            "No matches found for pattern: {pattern}"
        )));
    }
    Some(format_hits(ctx, pattern, hits, total).await)
}

fn split_windows_rg_line(line: &str) -> Option<(&str, usize, &str)> {
    // Find `:<digits>:` scanning left to right; path is everything before.
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b':' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 1 && j < bytes.len() && bytes[j] == b':' {
                let line_no: usize = line[i + 1..j].parse().ok()?;
                return Some((&line[..i], line_no, &line[j + 1..]));
            }
        }
        i += 1;
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn push_hit(
    _ctx: &ToolContext,
    base: &Path,
    root: &Path,
    hits: &mut BTreeMap<String, Vec<(usize, String)>>,
    total: &mut usize,
    path_part: &str,
    line_no: usize,
    text: &str,
) {
    let abs = root.join(path_part);
    let rel = relative_display(base, &abs);
    *total += 1;
    hits.entry(rel)
        .or_default()
        .push((line_no, trim_line(text.trim_end())));
}

async fn format_hits(
    ctx: &ToolContext,
    pattern: &str,
    hits: BTreeMap<String, Vec<(usize, String)>>,
    total: usize,
) -> ToolOutcome {
    let mut output = String::new();
    for (path, lines) in &hits {
        for (line, text) in lines {
            output.push_str(&format!("{path}:{line}: {text}\n"));
        }
    }
    if total >= GREP_LIMIT
        && let Some(path) = write_scratch(ctx, "grep", &output).await
    {
        output.push_str(&format!(
            "\n[TRUNCATED] {total} matches shown; complete list saved to {path}"
        ));
    }
    let _ = pattern;
    ToolOutcome::ok(output)
}

pub async fn grep(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let Some(pattern) = args.get("pattern").and_then(Value::as_str) else {
        return ToolOutcome::err_class(ToolErrorClass::Args, "Error: grep requires pattern");
    };
    if let Some(blocked) =
        crate::permissions::gate(ctx, "grep", &[pattern.to_string()], pattern).await
    {
        return blocked;
    }
    let base = match read_base(ctx, args) {
        Ok(base) => base,
        Err(error) => {
            return ToolOutcome::err_class(ToolErrorClass::Args, format!("Error: {error}"));
        }
    };
    let raw_root = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let root = match resolve_in(ctx, &base, raw_root) {
        Ok(path) => path,
        Err(error) => {
            return ToolOutcome::err_class(ToolErrorClass::Args, format!("Error: {error}"));
        }
    };
    let include = args.get("include").and_then(Value::as_str);
    if let Some(include) = include
        && Glob::new(include).is_err()
    {
        return ToolOutcome::err_class(
            ToolErrorClass::Args,
            format!("Error: invalid include glob: {include}"),
        );
    }
    // Forward-compat flags (opencode parity): contract only requires
    // `pattern`, but accept `literal`/`caseSensitive` when the model sends
    // them instead of miscompiling the regex.
    let literal = args
        .get("literal")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let case_sensitive = args
        .get("caseSensitive")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    // Codex search provider: `rg` first (exit 1 = no matches, neutral).
    if let Some(outcome) =
        try_rg_grep(ctx, &base, &root, pattern, include, literal, case_sensitive).await
    {
        return outcome;
    }

    // Fallback walker: Codex flags + ripgrep-syntax regex, binary-aware.
    let regex = if literal {
        RegexBuilder::new(&regex::escape(pattern))
            .case_insensitive(!case_sensitive)
            .build()
    } else {
        RegexBuilder::new(pattern)
            .case_insensitive(!case_sensitive)
            .build()
    };
    let regex = match regex {
        Ok(regex) => regex,
        Err(error) => {
            return ToolOutcome::err_class(
                ToolErrorClass::Args,
                format!("Error: Invalid regex pattern: {error}"),
            );
        }
    };
    let include_matcher = match include.map(Glob::new).transpose() {
        Ok(matcher) => matcher.map(|glob| glob.compile_matcher()),
        Err(_) => {
            return ToolOutcome::err_class(
                ToolErrorClass::Args,
                format!("Error: invalid include glob: {}", include.unwrap_or("")),
            );
        }
    };
    let mut hits: BTreeMap<String, Vec<(usize, String)>> = BTreeMap::new();
    let mut total = 0usize;
    let cached = match warm_list(&root, true, "grep").await {
        Ok(entries) => entries,
        Err(outcome) => return outcome,
    };
    'outer: for (rel_path, is_dir) in &cached {
        if *is_dir || under_denylist(rel_path, false) {
            continue;
        }
        let abs = root.join(rel_path);
        let rel = relative_display(&base, &abs);
        if let Some(include) = &include_matcher
            && !include.is_match(&rel)
            && !include.is_match(
                abs.file_name()
                    .map(|name| name.to_string_lossy())
                    .as_deref()
                    .unwrap_or(""),
            )
        {
            continue;
        }
        if std::fs::metadata(&abs)
            .ok()
            .is_some_and(|metadata| metadata.len() > 4 * 1024 * 1024)
        {
            continue;
        }
        // ripgrep skips binaries: NUL byte in the first 8KB => binary.
        let bytes = match std::fs::read(&abs) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };
        if bytes.iter().take(8192).any(|b| *b == 0) {
            continue;
        }
        let Ok(content) = String::from_utf8(bytes) else {
            continue;
        };
        for (index, line) in content.lines().enumerate() {
            if regex.is_match(line) {
                total += 1;
                hits.entry(rel.clone())
                    .or_default()
                    .push((index + 1, trim_line(line.trim_end())));
                if total >= GREP_LIMIT {
                    break 'outer;
                }
            }
        }
    }
    if total == 0 {
        return ToolOutcome::ok(format!("No matches found for pattern: {pattern}"));
    }
    format_hits(ctx, pattern, hits, total).await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::QuestionBroker;

    fn context(dir: &std::path::Path) -> ToolContext {
        ToolContext::new(dir, "v2-fs", Arc::new(QuestionBroker::new()))
    }

    #[tokio::test]
    async fn read_is_one_based_and_line_numbered() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        let ctx = context(dir.path());
        let out = read(
            &ctx,
            &serde_json::json!({"file_path": "a.txt", "offset": 2, "limit": 1}),
        )
        .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("2\ttwo"), "{}", out.content);
        assert!(!out.content.contains("1\tone"), "{}", out.content);
    }

    #[tokio::test]
    async fn read_returns_images_inline() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("shot.png"), b"\x89PNG\r\n\x1a\nfake").unwrap();
        let ctx = context(dir.path());
        let out = read(&ctx, &serde_json::json!({"file_path": "shot.png"})).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("[IMAGE]"), "{}", out.content);
    }

    #[tokio::test]
    async fn edit_supports_unique_and_replace_all() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x x x").unwrap();
        let ctx = context(dir.path());
        let _ = read(&ctx, &serde_json::json!({"file_path": "a.txt"})).await;
        let ambiguous = edit(
            &ctx,
            &serde_json::json!({"file_path": "a.txt", "old_string": "x", "new_string": "y"}),
        )
        .await;
        assert!(ambiguous.is_error);
        let all = edit(
            &ctx,
            &serde_json::json!({"file_path": "a.txt", "old_string": "x", "new_string": "y", "replace_all": true}),
        )
        .await;
        assert!(!all.is_error, "{}", all.content);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "y y y"
        );
    }

    #[tokio::test]
    async fn read_only_agent_can_inspect_a_registered_workspace() {
        let harness = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("notes.txt"), "project note\n").unwrap();
        std::fs::create_dir_all(harness.path().join(".deep_agents")).unwrap();
        let registry = serde_json::json!({
            "workspaces": [{
                "id": "ws-4ceria",
                "name": "4Ceria",
                "path": project.path().to_string_lossy(),
                "addedAt": "2026-01-01"
            }]
        });
        std::fs::write(
            harness.path().join(".deep_agents/workspaces.json"),
            serde_json::to_string(&registry).unwrap(),
        )
        .unwrap();
        let ctx = ToolContext::new(harness.path(), "orch", Arc::new(QuestionBroker::new()))
            .with_harness_root(harness.path())
            .with_access("read");
        let out = read(
            &ctx,
            &serde_json::json!({"workspace": "4Ceria", "file_path": "notes.txt"}),
        )
        .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("project note"), "{}", out.content);

        // A writable agent cannot use the selector to escape its workspace.
        let writable = ToolContext::new(harness.path(), "dev", Arc::new(QuestionBroker::new()))
            .with_harness_root(harness.path())
            .with_access("write");
        let denied = read(
            &writable,
            &serde_json::json!({"workspace": "4Ceria", "file_path": "notes.txt"}),
        )
        .await;
        assert!(denied.is_error, "{}", denied.content);
        assert!(
            denied
                .content
                .contains("only available to read-only agents")
        );
    }

    #[tokio::test]
    async fn glob_and_grep_find_files_and_content() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "fn hello() {}\n").unwrap();
        std::fs::write(dir.path().join("README.md"), "hello\n").unwrap();
        let ctx = context(dir.path());
        let globbed = glob(&ctx, &serde_json::json!({"pattern": "*.rs"})).await;
        assert!(!globbed.is_error, "{}", globbed.content);
        assert!(
            globbed.content.contains("src/lib.rs"),
            "{}",
            globbed.content
        );
        let grepped = grep(
            &ctx,
            &serde_json::json!({"pattern": "hello", "include": "*.rs"}),
        )
        .await;
        assert!(!grepped.is_error, "{}", grepped.content);
        assert!(
            grepped.content.contains("src/lib.rs:1"),
            "{}",
            grepped.content
        );
        assert!(
            !grepped.content.contains("README.md"),
            "{}",
            grepped.content
        );
    }

    #[tokio::test]
    async fn glob_fuzzy_query_matches_codex_ranking() {
        // Codex `file-search` basename boost: among files-only (contract says
        // files, not directories), `ChatViewModel.kt` (basename contains the
        // query) outranks `viewmodel/build.gradle.kts` (query only in the
        // directory part) even with 20 crowding `parent-*/viewmodel` folders.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("viewmodel")).unwrap();
        std::fs::write(dir.path().join("viewmodel/build.gradle.kts"), "").unwrap();
        std::fs::create_dir_all(dir.path().join("feature/src")).unwrap();
        std::fs::write(dir.path().join("feature/src/ChatViewModel.kt"), "").unwrap();
        for i in 0..20 {
            std::fs::create_dir_all(dir.path().join(format!("parent-{i}/viewmodel"))).unwrap();
        }
        let ctx = context(dir.path());
        let out = glob(&ctx, &serde_json::json!({"pattern": "ViewModel"})).await;
        assert!(!out.is_error, "{}", out.content);
        let first = out.content.lines().next().unwrap_or("");
        assert!(
            first.contains("ChatViewModel.kt"),
            "basename match first, got: {first}\n{}",
            out.content
        );
    }

    #[tokio::test]
    async fn glob_scores_before_paths_on_ties() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b_path.txt"), "").unwrap();
        std::fs::write(dir.path().join("a_path.txt"), "").unwrap();
        let ctx = context(dir.path());
        // Glob-syntax path still orders score-desc then path-asc.
        let out = glob(&ctx, &serde_json::json!({"pattern": "*.txt"})).await;
        assert!(!out.is_error, "{}", out.content);
        let a = out.content.find("a_path.txt");
        let b = out.content.find("b_path.txt");
        assert!(a.is_some() && b.is_some(), "{}", out.content);
        assert!(a.unwrap() < b.unwrap(), "{}", out.content);
    }

    #[tokio::test]
    async fn grep_skips_binaries_and_limits_to_spec() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bin.dat"), b"\x00\x01binary hello\x00").unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        let ctx = context(dir.path());
        let out = grep(&ctx, &serde_json::json!({"pattern": "hello"})).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("a.txt:1"), "{}", out.content);
        assert!(!out.content.contains("bin.dat"), "{}", out.content);
    }

    #[tokio::test]
    async fn grep_reports_no_matches_neutrally() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        let ctx = context(dir.path());
        // ripgrep exit 1 is neutral, not a failure indicator.
        let out = grep(&ctx, &serde_json::json!({"pattern": "zzz-no-match-zzz"})).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("No matches found"), "{}", out.content);
    }
}
