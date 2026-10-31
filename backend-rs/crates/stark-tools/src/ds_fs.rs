//! DeepSeek Harness-compatible filesystem tools: read, write, edit, glob,
//! and grep. `read` handles text and images. The old read_file/edit_file
//! pair remains available to agents that have not migrated.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use globset::Glob;
use ignore::WalkBuilder;
use regex::RegexBuilder;
use serde_json::Value;

use crate::paths::{relative_display, resolve_workspace_path};
use crate::{ToolContext, ToolOutcome};

const DEFAULT_READ_LIMIT: usize = 2000;
const MAX_READ_LIMIT: usize = 10_000;
const MAX_LINE_CHARS: usize = 2000;
const GLOB_LIMIT: usize = 300;
const GREP_LIMIT: usize = 500;

fn resolve(ctx: &ToolContext, raw: &str) -> Result<PathBuf, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("path is empty".to_string());
    }
    let candidate = Path::new(raw);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        ctx.current_dir().join(candidate)
    };
    resolve_workspace_path(&ctx.workspace, &joined.to_string_lossy())
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
        return ToolOutcome::err("Error: read requires file_path");
    };
    let path = match resolve(ctx, raw) {
        Ok(path) => path,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };
    if path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| crate::images::mime_for_ext(ext).is_some())
    {
        let forwarded = serde_json::json!({"file_path": path.to_string_lossy()});
        return crate::read::read_file(ctx, &forwarded).await;
    }
    let content = match tokio::fs::read_to_string(&path).await {
        Ok(content) => content,
        Err(error) => return ToolOutcome::err(format!("Error: {raw}: {error}")),
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
        return ToolOutcome::err(format!(
            "Error: offset {offset} is beyond the end of {raw} ({total} lines)"
        ));
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

pub async fn write(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let Some(raw) = args.get("file_path").and_then(Value::as_str) else {
        return ToolOutcome::err("Error: write requires file_path");
    };
    let Some(content) = args.get("content").and_then(Value::as_str) else {
        return ToolOutcome::err("Error: write requires content");
    };
    let path = match resolve(ctx, raw) {
        Ok(path) => path,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };
    if path.exists() && !ctx.was_read(&path) {
        return ToolOutcome::err(format!("Error: read {raw} before overwriting it"));
    }
    if let Some(parent) = path.parent()
        && let Err(error) = tokio::fs::create_dir_all(parent).await
    {
        return ToolOutcome::err(format!("Error: {raw}: {error}"));
    }
    if let Err(error) = tokio::fs::write(&path, content).await {
        return ToolOutcome::err(format!("Error: {raw}: {error}"));
    }
    ctx.mark_read(path);
    ToolOutcome::ok(format!(
        "[OK] Wrote {raw} ({} lines, {} chars)",
        content.lines().count(),
        content.chars().count()
    ))
}

pub async fn edit(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let Some(raw) = args.get("file_path").and_then(Value::as_str) else {
        return ToolOutcome::err("Error: edit requires file_path");
    };
    let Some(old_string) = args.get("old_string").and_then(Value::as_str) else {
        return ToolOutcome::err("Error: edit requires old_string");
    };
    let Some(new_string) = args.get("new_string").and_then(Value::as_str) else {
        return ToolOutcome::err("Error: edit requires new_string");
    };
    if old_string.is_empty() {
        return ToolOutcome::err("Error: old_string must not be empty");
    }
    let path = match resolve(ctx, raw) {
        Ok(path) => path,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };
    if !ctx.was_read(&path) {
        return ToolOutcome::err(format!("Error: read {raw} before editing it"));
    }
    let content = match tokio::fs::read_to_string(&path).await {
        Ok(content) => content,
        Err(error) => return ToolOutcome::err(format!("Error: {raw}: {error}")),
    };
    let replace_all = args
        .get("replace_all")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let count = content.matches(old_string).count();
    if count == 0 {
        return ToolOutcome::err(format!(
            "Error: old_string not found in {raw}. The text must match exactly."
        ));
    }
    if count > 1 && !replace_all {
        return ToolOutcome::err(format!(
            "Error: old_string matches {count} times in {raw}. Include more context or set replace_all=true."
        ));
    }
    let updated = if replace_all {
        content.replace(old_string, new_string)
    } else {
        content.replacen(old_string, new_string, 1)
    };
    if let Err(error) = tokio::fs::write(&path, updated).await {
        return ToolOutcome::err(format!("Error: {raw}: {error}"));
    }
    ToolOutcome::ok(format!(
        "[OK] Edited {raw} ({} replacement{})",
        count,
        if count == 1 { "" } else { "s" }
    ))
}

fn walk(root: &Path, respect_ignore: bool) -> impl Iterator<Item = ignore::DirEntry> {
    let mut builder = WalkBuilder::new(root);
    if respect_ignore {
        builder.standard_filters(true);
    } else {
        builder
            .hidden(false)
            .ignore(false)
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .parents(false);
    }
    builder
        .filter_entry(|entry| !skip_entry(entry))
        .build()
        .filter_map(Result::ok)
}

fn skip_entry(entry: &ignore::DirEntry) -> bool {
    if !entry.file_type().is_some_and(|kind| kind.is_dir()) {
        return false;
    }
    let name = entry.file_name().to_string_lossy();
    matches!(
        name.as_ref(),
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

fn scratch_path(ctx: &ToolContext, prefix: &str) -> PathBuf {
    let stamp = chrono::Utc::now().timestamp_millis();
    ctx.harness_root
        .join(".deep_agents")
        .join("scratch")
        .join(format!("{prefix}-{stamp}.txt"))
}

async fn write_scratch(ctx: &ToolContext, prefix: &str, content: &str) -> Option<String> {
    let path = scratch_path(ctx, prefix);
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    tokio::fs::write(&path, content).await.ok()?;
    Some(path.to_string_lossy().to_string())
}

pub async fn glob(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let Some(pattern) = args.get("pattern").and_then(Value::as_str) else {
        return ToolOutcome::err("Error: glob requires pattern");
    };
    let raw_root = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let root = match resolve(ctx, raw_root) {
        Ok(path) => path,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };
    let anchored = pattern.contains('/') || pattern.contains('\\');
    let matcher = match Glob::new(pattern) {
        Ok(glob) => glob.compile_matcher(),
        Err(error) => return ToolOutcome::err(format!("Error: invalid glob: {error}")),
    };
    let mut files: Vec<(PathBuf, u128)> = Vec::new();
    for entry in walk(&root, false) {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let rel = relative_display(&ctx.workspace, entry.path());
        let matched = if anchored {
            matcher.is_match(&rel)
        } else {
            matcher.is_match(entry.file_name().to_string_lossy().as_ref())
        };
        if !matched {
            continue;
        }
        let modified = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis())
            .unwrap_or(0);
        files.push((entry.path().to_path_buf(), modified));
    }
    files.sort_by_key(|left| std::cmp::Reverse(left.1));
    let total = files.len();
    let shown: Vec<String> = files
        .iter()
        .take(GLOB_LIMIT)
        .map(|(path, _)| relative_display(&ctx.workspace, path))
        .collect();
    if total == 0 {
        return ToolOutcome::ok(format!("No files found matching: {pattern}"));
    }
    let mut output = shown.join("\n");
    if total > GLOB_LIMIT {
        let all: Vec<String> = files
            .iter()
            .map(|(path, _)| relative_display(&ctx.workspace, path))
            .collect();
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

pub async fn grep(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let Some(pattern) = args.get("pattern").and_then(Value::as_str) else {
        return ToolOutcome::err("Error: grep requires pattern");
    };
    let raw_root = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let root = match resolve(ctx, raw_root) {
        Ok(path) => path,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };
    let include = args.get("include").and_then(Value::as_str);
    let include = match include.map(Glob::new).transpose() {
        Ok(glob) => glob.map(|glob| glob.compile_matcher()),
        Err(error) => return ToolOutcome::err(format!("Error: invalid include glob: {error}")),
    };
    let regex = match RegexBuilder::new(pattern).build() {
        Ok(regex) => regex,
        Err(error) => return ToolOutcome::err(format!("Error: Invalid regex pattern: {error}")),
    };
    let mut hits: BTreeMap<String, Vec<(usize, String)>> = BTreeMap::new();
    let mut total = 0usize;
    'outer: for entry in walk(&root, true) {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let rel = relative_display(&ctx.workspace, entry.path());
        if let Some(include) = &include
            && !include.is_match(&rel)
            && !include.is_match(entry.file_name().to_string_lossy().as_ref())
        {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        for (index, line) in content.lines().enumerate() {
            if regex.is_match(line) {
                total += 1;
                hits.entry(rel.clone())
                    .or_default()
                    .push((index + 1, line.trim_end().to_string()));
                if total >= GREP_LIMIT {
                    break 'outer;
                }
            }
        }
    }
    if total == 0 {
        return ToolOutcome::ok(format!("No matches found for pattern: {pattern}"));
    }
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
    ToolOutcome::ok(output)
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
    async fn write_requires_read_for_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "before").unwrap();
        let ctx = context(dir.path());
        let denied = write(
            &ctx,
            &serde_json::json!({"file_path": "a.txt", "content": "after"}),
        )
        .await;
        assert!(denied.is_error);
        assert!(denied.content.contains("read a.txt before overwriting"));
        let _ = read(&ctx, &serde_json::json!({"file_path": "a.txt"})).await;
        let ok = write(
            &ctx,
            &serde_json::json!({"file_path": "a.txt", "content": "after"}),
        )
        .await;
        assert!(!ok.is_error, "{}", ok.content);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "after"
        );
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
}
