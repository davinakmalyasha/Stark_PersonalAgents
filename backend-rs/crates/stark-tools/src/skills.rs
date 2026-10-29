//! DeepSeek Harness-compatible skill catalog and loader.
//!
//! Skills are directories under `.agents/skills/<name>/SKILL.md`. The catalog
//! is injected into the system prompt; the full file is loaded only when the
//! model calls the `skill` tool.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::{ToolContext, ToolOutcome};

const MAX_SKILL_CHARS: usize = 8192;

fn roots(ctx: &ToolContext) -> Vec<PathBuf> {
    let mut roots = vec![
        ctx.workspace.join(".agents").join("skills"),
        ctx.harness_root.join(".agents").join("skills"),
        ctx.harness_root.join(".deep_agents").join("skills"),
    ];
    roots.sort();
    roots.dedup();
    roots
}

fn unquote(value: &str) -> String {
    value
        .trim()
        .trim_matches(|character| character == '"' || character == '\'')
        .to_string()
}

fn parse_frontmatter(text: &str) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    let trimmed = text.trim_start();
    if !trimmed.starts_with("---") {
        return fields;
    }
    let mut lines = trimmed.lines();
    lines.next();
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        fields.insert(key.trim().to_ascii_lowercase(), unquote(value));
    }
    fields
}

fn fallback_description(text: &str) -> String {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("---") {
            continue;
        }
        return line.chars().take(160).collect();
    }
    String::new()
}

async fn read_skill(path: &Path) -> Option<(String, String, String)> {
    let text = tokio::fs::read_to_string(path).await.ok()?;
    let fields = parse_frontmatter(&text);
    let name = fields
        .get("name")
        .cloned()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            path.parent()
                .and_then(|parent| parent.file_name())
                .map(|name| name.to_string_lossy().to_string())
        })?;
    let description = fields
        .get("description")
        .cloned()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback_description(&text));
    Some((name, description, text))
}

pub async fn catalog_block_for(workspace: &Path, harness_root: &Path) -> Option<String> {
    let mut roots = vec![
        workspace.join(".agents").join("skills"),
        harness_root.join(".agents").join("skills"),
        harness_root.join(".deep_agents").join("skills"),
    ];
    roots.sort();
    roots.dedup();
    let mut entries = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for root in roots {
        let Ok(mut dir) = tokio::fs::read_dir(&root).await else {
            continue;
        };
        while let Ok(Some(entry)) = dir.next_entry().await {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let skill_path = path.join("SKILL.md");
            if !skill_path.is_file() {
                continue;
            }
            let Some((name, description, _)) = read_skill(&skill_path).await else {
                continue;
            };
            if !seen.insert(name.to_ascii_lowercase()) {
                continue;
            }
            entries.push(serde_json::json!({
                "name": name,
                "description": description,
            }));
        }
    }
    if entries.is_empty() {
        return None;
    }
    entries.sort_by(|left, right| {
        left["name"]
            .as_str()
            .unwrap_or("")
            .cmp(right["name"].as_str().unwrap_or(""))
    });
    let mut text = String::from(
        "[SKILLS]\nLoad a skill with the skill tool before acting on a matching task.\n",
    );
    for entry in entries {
        text.push_str(&format!(
            "- {}: {}\n",
            entry["name"].as_str().unwrap_or(""),
            entry["description"].as_str().unwrap_or("")
        ));
    }
    Some(text)
}

pub async fn skill(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let Some(name) = args.get("name").and_then(Value::as_str).map(str::trim) else {
        return ToolOutcome::err("Error: skill requires name");
    };
    if name.is_empty()
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return ToolOutcome::err("Error: invalid skill name");
    }
    for root in roots(ctx) {
        let path = root.join(name).join("SKILL.md");
        if !path.is_file() {
            continue;
        }
        let Some((_, _, text)) = read_skill(&path).await else {
            continue;
        };
        let text: String = text.chars().take(MAX_SKILL_CHARS).collect();
        return ToolOutcome::ok(format!("[SKILL] {name}\n\n{text}"));
    }
    ToolOutcome::err(format!("Error: unknown skill '{name}'"))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::QuestionBroker;

    fn context(dir: &std::path::Path) -> ToolContext {
        ToolContext::new(dir, "v2-skill", Arc::new(QuestionBroker::new()))
    }

    #[tokio::test]
    async fn catalog_and_loader_read_skill_md() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join(".agents/skills/review");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: review\ndescription: Review code carefully.\n---\n\n# Review\n\nDo it.\n",
        )
        .unwrap();
        let ctx = context(dir.path());
        let catalog = catalog_block_for(dir.path(), dir.path()).await.unwrap();
        assert!(
            catalog.contains("review: Review code carefully."),
            "{catalog}"
        );
        let loaded = skill(&ctx, &serde_json::json!({"name": "review"})).await;
        assert!(!loaded.is_error, "{}", loaded.content);
        assert!(loaded.content.contains("# Review"), "{}", loaded.content);
    }

    #[tokio::test]
    async fn skill_rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let out = skill(&ctx, &serde_json::json!({"name": "../secret"})).await;
        assert!(out.is_error);
    }
}
