use serde_json::Value;

use crate::paths::resolve_workspace_path;
use crate::{ToolContext, ToolOutcome};

pub async fn edit_file(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    if args.get("diff").and_then(Value::as_str).is_some() {
        return ToolOutcome::err(
            "Error: edit_file diff mode (Mode C) is not implemented in the Rust Phase 1 backend",
        );
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
        let lines = new_string.lines().count();
        return ToolOutcome::ok(format!(
            "[OK] Wrote {raw_path} ({lines} lines, {} chars)",
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
