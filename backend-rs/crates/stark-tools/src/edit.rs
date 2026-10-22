use serde_json::Value;

use crate::paths::resolve_workspace_path;
use crate::{ToolContext, ToolOutcome};

pub async fn edit_file(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    if let Some(diff) = args
        .get("diff")
        .and_then(Value::as_str)
        .filter(|diff| !diff.trim().is_empty())
    {
        return apply_diff(ctx, diff).await;
    }

    let Some(raw_path) = args.get("file_path").and_then(Value::as_str) else {
        return ToolOutcome::err("Error: edit_file requires file_path");
    };
    let path = match resolve_workspace_path(&ctx.workspace, raw_path) {
        Ok(path) => path,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };

    let old_string = args.get("old_string").and_then(Value::as_str);
    let Some(new_string) = args.get("new_string").and_then(Value::as_str) else {
        return ToolOutcome::err(
            "Error: edit_file requires new_string, or old_string + new_string",
        );
    };

    if old_string.is_none() {
        if let Some(parent) = path.parent()
            && let Err(error) = tokio::fs::create_dir_all(parent).await
        {
            return ToolOutcome::err(format!("Error: {raw_path}: {error}"));
        }
        if let Err(error) = tokio::fs::write(&path, new_string).await {
            return ToolOutcome::err(format!("Error: {raw_path}: {error}"));
        }
        return ToolOutcome::ok(format!(
            "[OK] Wrote {raw_path} ({} lines, {} chars)",
            new_string.lines().count(),
            new_string.chars().count()
        ));
    }

    let old_string = old_string.unwrap_or_default();
    if old_string == new_string {
        return ToolOutcome::err("Error: old_string and new_string are identical.");
    }

    let content = match tokio::fs::read_to_string(&path).await {
        Ok(content) => content,
        Err(error) => return ToolOutcome::err(format!("Error: {raw_path}: {error}")),
    };
    let count = content.matches(old_string).count();
    if count == 0 {
        return ToolOutcome::err(format!(
            "Error: old_string not found in {raw_path}. The text must match exactly. \
             Read the file with read_file before retrying."
        ));
    }
    if count > 1 {
        return ToolOutcome::err(format!(
            "Error: old_string matches {count} times in {raw_path}. \
             Include more surrounding context to make it unique."
        ));
    }

    let updated = content.replacen(old_string, new_string, 1);
    if let Err(error) = tokio::fs::write(&path, updated).await {
        return ToolOutcome::err(format!("Error: {raw_path}: {error}"));
    }
    ToolOutcome::ok(format!("[OK] Edited {raw_path} (1 replacement)"))
}

#[derive(Debug)]
enum PatchOp {
    Add {
        path: String,
        content: String,
    },
    Update {
        path: String,
        hunks: Vec<Hunk>,
        move_to: Option<String>,
    },
    Delete {
        path: String,
    },
}

#[derive(Debug)]
struct Hunk {
    find: Vec<String>,
    replace: Vec<String>,
}

async fn apply_diff(ctx: &ToolContext, diff: &str) -> ToolOutcome {
    let ops = match parse_patch(diff) {
        Ok(ops) => ops,
        Err(error) => return ToolOutcome::err(format!("Error: invalid patch: {error}")),
    };
    if ops.is_empty() {
        return ToolOutcome::err("Error: diff contains no file operations");
    }

    let mut changed_path = String::new();
    let mut changed_lines = 0usize;

    for op in ops {
        let result = match op {
            PatchOp::Add { path, content } => add_file(ctx, &path, &content).await,
            PatchOp::Update {
                path,
                hunks,
                move_to,
            } => update_file(ctx, &path, &hunks, move_to.as_deref()).await,
            PatchOp::Delete { path } => delete_file(ctx, &path).await,
        };

        match result {
            Ok((path, lines)) => {
                if changed_path.is_empty() {
                    changed_path = path;
                }
                changed_lines += lines;
            }
            Err(error) => return ToolOutcome::err(error),
        }
    }

    ToolOutcome::ok(format!(
        "[OK] Applied diff to {changed_path} ({changed_lines} lines)"
    ))
}

async fn add_file(
    ctx: &ToolContext,
    raw_path: &str,
    content: &str,
) -> Result<(String, usize), String> {
    let path = resolve_workspace_path(&ctx.workspace, raw_path)?;
    if path.exists() {
        return Err(format!("Error: cannot add existing file: {raw_path}"));
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| format!("Error: {raw_path}: {error}"))?;
    }
    tokio::fs::write(&path, content)
        .await
        .map_err(|error| format!("Error: {raw_path}: {error}"))?;
    Ok((raw_path.to_string(), content.lines().count()))
}

async fn update_file(
    ctx: &ToolContext,
    raw_path: &str,
    hunks: &[Hunk],
    move_to: Option<&str>,
) -> Result<(String, usize), String> {
    if hunks.is_empty() && move_to.is_none() {
        return Err(format!("Error: patch has no hunks for {raw_path}"));
    }
    let path = resolve_workspace_path(&ctx.workspace, raw_path)?;
    let content = tokio::fs::read_to_string(&path)
        .await
        .map_err(|error| format!("Error: {raw_path}: {error}"))?;
    let mut lines: Vec<String> = content.lines().map(str::to_string).collect();
    let trailing_newline = content.ends_with('\n');

    for hunk in hunks {
        if hunk.find.is_empty() {
            return Err(format!("Error: empty hunk for {raw_path}"));
        }
        let positions: Vec<usize> = lines
            .windows(hunk.find.len())
            .enumerate()
            .filter_map(|(index, window)| (window == hunk.find).then_some(index))
            .collect();
        if positions.is_empty() {
            return Err(format!(
                "Error: patch hunk not found in {raw_path}. Read the file and retry with exact context."
            ));
        }
        if positions.len() > 1 {
            return Err(format!(
                "Error: patch hunk is ambiguous in {raw_path} ({} matches). Add more context.",
                positions.len()
            ));
        }
        let position = positions[0];
        lines.splice(
            position..position + hunk.find.len(),
            hunk.replace.iter().cloned(),
        );
    }

    let mut updated = lines.join("\n");
    if trailing_newline {
        updated.push('\n');
    }

    if let Some(move_to) = move_to {
        let destination = resolve_workspace_path(&ctx.workspace, move_to)?;
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| format!("Error: {move_to}: {error}"))?;
        }
        tokio::fs::write(&destination, &updated)
            .await
            .map_err(|error| format!("Error: {move_to}: {error}"))?;
        tokio::fs::remove_file(&path)
            .await
            .map_err(|error| format!("Error: {raw_path}: {error}"))?;
    } else {
        tokio::fs::write(&path, &updated)
            .await
            .map_err(|error| format!("Error: {raw_path}: {error}"))?;
    }

    Ok((raw_path.to_string(), hunks.len()))
}

async fn delete_file(ctx: &ToolContext, raw_path: &str) -> Result<(String, usize), String> {
    let path = resolve_workspace_path(&ctx.workspace, raw_path)?;
    if !path.exists() {
        return Err(format!("Error: cannot delete missing file: {raw_path}"));
    }
    tokio::fs::remove_file(&path)
        .await
        .map_err(|error| format!("Error: {raw_path}: {error}"))?;
    Ok((raw_path.to_string(), 0))
}

fn parse_patch(diff: &str) -> Result<Vec<PatchOp>, String> {
    let lines: Vec<&str> = diff.lines().collect();
    let mut index = 0;
    let mut ops = Vec::new();

    while index < lines.len() {
        let line = lines[index].trim_end();
        if line.is_empty() || line == "*** Begin Patch" || line == "*** End Patch" {
            index += 1;
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Add File:") {
            let path = path.trim().to_string();
            index += 1;
            let mut body = Vec::new();
            while index < lines.len() && !lines[index].starts_with("***") {
                let value = lines[index];
                body.push(value.strip_prefix('+').unwrap_or(value).to_string());
                index += 1;
            }
            ops.push(PatchOp::Add {
                path,
                content: finish_body(body),
            });
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Update File:") {
            let path = path.trim().to_string();
            index += 1;
            let mut hunks = Vec::new();
            let mut move_to = None;
            let mut current: Option<Hunk> = None;

            while index < lines.len() {
                let value = lines[index];
                if let Some(destination) = value.trim_end().strip_prefix("*** Move to:") {
                    if let Some(hunk) = current.take()
                        && (!hunk.find.is_empty() || !hunk.replace.is_empty())
                    {
                        hunks.push(hunk);
                    }
                    move_to = Some(destination.trim().to_string());
                    index += 1;
                    continue;
                }
                if value.starts_with("*** ") {
                    break;
                }
                index += 1;

                if value.trim_end().is_empty() {
                    continue;
                }
                if value.starts_with("@@") {
                    if let Some(hunk) = current.take()
                        && (!hunk.find.is_empty() || !hunk.replace.is_empty())
                    {
                        hunks.push(hunk);
                    }
                    current = Some(Hunk {
                        find: Vec::new(),
                        replace: Vec::new(),
                    });
                    continue;
                }

                let hunk = current.get_or_insert_with(|| Hunk {
                    find: Vec::new(),
                    replace: Vec::new(),
                });
                let (prefix, text) = value.split_at(1);
                match prefix {
                    " " => {
                        hunk.find.push(text.to_string());
                        hunk.replace.push(text.to_string());
                    }
                    "-" => hunk.find.push(text.to_string()),
                    "+" => hunk.replace.push(text.to_string()),
                    _ => {
                        return Err(format!(
                            "expected ' ', '+', or '-' in update hunk, got: {value}"
                        ));
                    }
                }
            }

            if let Some(hunk) = current
                && (!hunk.find.is_empty() || !hunk.replace.is_empty())
            {
                hunks.push(hunk);
            }
            ops.push(PatchOp::Update {
                path,
                hunks,
                move_to,
            });
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Delete File:") {
            ops.push(PatchOp::Delete {
                path: path.trim().to_string(),
            });
            index += 1;
            while index < lines.len() && !lines[index].starts_with("***") {
                index += 1;
            }
            continue;
        }

        return Err(format!("unknown patch header: {line}"));
    }

    Ok(ops)
}

fn finish_body(lines: Vec<String>) -> String {
    let mut body = lines.join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Arc;

    use crate::QuestionBroker;

    fn context(workspace: &Path) -> ToolContext {
        ToolContext::new(workspace, "test", Arc::new(QuestionBroker::new()))
    }

    #[tokio::test]
    async fn applies_add_update_delete_patch() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let add = edit_file(
            &ctx,
            &serde_json::json!({
                "diff": "*** Begin Patch\n*** Add File: src/demo.txt\n+one\n+two\n*** End Patch"
            }),
        )
        .await;
        assert!(!add.is_error, "{}", add.content);

        let update = edit_file(
            &ctx,
            &serde_json::json!({
                "diff": "*** Begin Patch\n*** Update File: src/demo.txt\n@@\n one\n-two\n+three\n*** End Patch"
            }),
        )
        .await;
        assert!(!update.is_error, "{}", update.content);
        assert_eq!(
            tokio::fs::read_to_string(dir.path().join("src/demo.txt"))
                .await
                .unwrap(),
            "one\nthree\n"
        );

        let delete = edit_file(
            &ctx,
            &serde_json::json!({
                "diff": "*** Begin Patch\n*** Delete File: src/demo.txt\n*** End Patch"
            }),
        )
        .await;
        assert!(!delete.is_error, "{}", delete.content);
        assert!(!dir.path().join("src/demo.txt").exists());
    }

    #[tokio::test]
    async fn patch_rejects_ambiguous_hunks() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        tokio::fs::write(dir.path().join("a.txt"), "same\nsame\n")
            .await
            .unwrap();
        let result = edit_file(
            &ctx,
            &serde_json::json!({
                "diff": "*** Update File: a.txt\n@@\n-same\n+other\n"
            }),
        )
        .await;
        assert!(result.is_error);
        assert!(result.content.contains("ambiguous"));
    }
}
