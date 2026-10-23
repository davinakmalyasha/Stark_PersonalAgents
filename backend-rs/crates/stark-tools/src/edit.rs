use serde_json::Value;

use crate::paths::resolve_workspace_path;
use crate::{ToolContext, ToolOutcome};

pub async fn edit_file(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    if let Some(diff) = args
        .get("diff")
        .and_then(Value::as_str)
        .filter(|diff| !diff.trim().is_empty())
    {
        if let Some(checkpoint) = &ctx.checkpoint {
            checkpoint.snapshot_patch(diff);
        }
        return crate::patch::apply_patch(ctx, diff).await;
    }

    let Some(raw_path) = args.get("file_path").and_then(Value::as_str) else {
        return ToolOutcome::err("Error: edit_file requires file_path");
    };
    let path = match resolve_workspace_path(&ctx.workspace, raw_path) {
        Ok(path) => path,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };

    if let Some(checkpoint) = &ctx.checkpoint {
        checkpoint.snapshot_file(&path);
    }

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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::QuestionBroker;

    fn context(workspace: &std::path::Path) -> ToolContext {
        ToolContext::new(workspace, "test", Arc::new(QuestionBroker::new()))
    }

    #[tokio::test]
    async fn edit_records_a_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        tokio::fs::write(&file, "before").await.unwrap();
        let checkpoint = crate::Checkpoint::begin("chat", dir.path());
        checkpoint.set_user_message("u1");
        let ctx = context(dir.path()).with_checkpoint(checkpoint.clone());
        let result = edit_file(
            &ctx,
            &serde_json::json!({"file_path": "a.txt", "new_string": "after"}),
        )
        .await;
        assert!(!result.is_error, "{}", result.content);
        checkpoint.finalize();

        let groups = crate::Checkpoint::list(dir.path(), "chat", 10);
        assert_eq!(groups.len(), 1);
        let report =
            crate::Checkpoint::restore_for_messages(dir.path(), "chat", &["u1".to_string()], false);
        assert!(report.ok(), "{:?}", report);
        assert_eq!(tokio::fs::read_to_string(&file).await.unwrap(), "before");
    }

    #[tokio::test]
    async fn targeted_edit_and_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let write = edit_file(
            &ctx,
            &serde_json::json!({"file_path": "a.txt", "new_string": "one\ntwo\n"}),
        )
        .await;
        assert!(!write.is_error, "{}", write.content);

        let edit = edit_file(
            &ctx,
            &serde_json::json!({
                "file_path": "a.txt",
                "old_string": "two",
                "new_string": "three"
            }),
        )
        .await;
        assert!(!edit.is_error, "{}", edit.content);
        assert_eq!(
            tokio::fs::read_to_string(dir.path().join("a.txt"))
                .await
                .unwrap(),
            "one\nthree\n"
        );
    }
}
