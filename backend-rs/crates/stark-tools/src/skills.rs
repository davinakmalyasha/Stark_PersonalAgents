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

/// Skills are instructions from the workspace. Reject symlinked entries in
/// every root: a cloned repo must not be able to point `SKILL.md` at an
/// arbitrary host file.
fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_dir())
        .unwrap_or(false)
}

fn is_real_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false)
}

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
    let metadata = tokio::fs::symlink_metadata(path).await.ok()?;
    if !metadata.file_type().is_file() {
        return None;
    }
    if let Some(parent) = path.parent() {
        let metadata = tokio::fs::symlink_metadata(parent).await.ok()?;
        if !metadata.file_type().is_dir() {
            return None;
        }
    }
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

/// Discovered skill names, sync, for agent-definition validation.
pub fn skill_names(workspace: &Path, harness_root: &Path) -> Vec<String> {
    let mut roots = vec![
        workspace.join(".agents").join("skills"),
        harness_root.join(".agents").join("skills"),
        harness_root.join(".deep_agents").join("skills"),
    ];
    roots.sort();
    roots.dedup();
    let mut names = std::collections::BTreeSet::new();
    for root in roots {
        if !is_real_dir(&root) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_real_dir(&path) {
                continue;
            }
            let skill_path = path.join("SKILL.md");
            if !is_real_file(&skill_path) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&skill_path) else {
                continue;
            };
            let fields = parse_frontmatter(&text);
            let name = fields
                .get("name")
                .cloned()
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    path.file_name()
                        .map(|name| name.to_string_lossy().to_string())
                });
            if let Some(name) = name {
                names.insert(name);
            }
        }
    }
    names.into_iter().collect()
}

pub async fn catalog_block_for(workspace: &Path, harness_root: &Path) -> Option<String> {
    catalog_block_for_agent(workspace, harness_root, None).await
}

/// Same catalog, filtered to an agent's configured skill names. `None` means
/// all discovered skills.
pub async fn catalog_block_for_agent(
    workspace: &Path,
    harness_root: &Path,
    allowed: Option<&[String]>,
) -> Option<String> {
    let allowed: Option<std::collections::HashSet<String>> = allowed.map(|names| {
        names
            .iter()
            .map(|name| name.trim().to_ascii_lowercase())
            .filter(|name| !name.is_empty())
            .collect()
    });
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
        let Ok(metadata) = tokio::fs::symlink_metadata(&root).await else {
            continue;
        };
        if !metadata.file_type().is_dir() {
            continue;
        }
        let Ok(mut dir) = tokio::fs::read_dir(&root).await else {
            continue;
        };
        while let Ok(Some(entry)) = dir.next_entry().await {
            let Ok(file_type) = entry.file_type().await else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let path = entry.path();
            let skill_path = path.join("SKILL.md");
            let Some((name, description, _)) = read_skill(&skill_path).await else {
                continue;
            };
            if let Some(allowed) = &allowed
                && !allowed.contains(&name.to_ascii_lowercase())
            {
                continue;
            }
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
    if let Some(allowed) = &ctx.allowed_skills
        && !allowed.contains(&name.to_ascii_lowercase())
    {
        tracing::debug!(skill = %name, "skill is not enabled for this agent");
        return ToolOutcome::err(format!(
            "Error: skill '{name}' is not enabled for this agent"
        ));
    }
    for root in roots(ctx) {
        let Ok(metadata) = tokio::fs::symlink_metadata(&root).await else {
            continue;
        };
        if !metadata.file_type().is_dir() {
            continue;
        }
        let skill_dir = root.join(name);
        let Ok(metadata) = tokio::fs::symlink_metadata(&skill_dir).await else {
            continue;
        };
        if !metadata.file_type().is_dir() {
            continue;
        }
        let path = skill_dir.join("SKILL.md");
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
    async fn per_agent_skill_allowlist_blocks_others() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join(".agents/skills/review");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: review\ndescription: Review code.\n---\n# Review\n",
        )
        .unwrap();
        let ctx = context(dir.path()).with_allowed_skills(Some(vec!["other".to_string()]));
        let out = skill(&ctx, &serde_json::json!({"name": "review"})).await;
        assert!(out.is_error);
        assert!(out.content.contains("not enabled"));
        let ctx = context(dir.path()).with_allowed_skills(Some(vec!["review".to_string()]));
        let out = skill(&ctx, &serde_json::json!({"name": "review"})).await;
        assert!(!out.is_error, "{}", out.content);
    }

    #[tokio::test]
    async fn skill_rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let out = skill(&ctx, &serde_json::json!({"name": "../secret"})).await;
        assert!(out.is_error);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_skill_entries_are_ignored() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(
            outside.path().join("SKILL.md"),
            "---\nname: leaked\ndescription: Leaked.\n---\n# Leaked\n",
        )
        .unwrap();
        let skill_dir = dir.path().join(".agents/skills/leaked");
        std::fs::create_dir_all(&skill_dir).unwrap();
        symlink(outside.path().join("SKILL.md"), skill_dir.join("SKILL.md")).unwrap();
        symlink(outside.path(), dir.path().join(".agents/skills/linked-dir")).unwrap();

        let ctx = context(dir.path());
        assert!(catalog_block_for(dir.path(), dir.path()).await.is_none());
        let out = skill(&ctx, &serde_json::json!({"name": "leaked"})).await;
        assert!(out.is_error, "{}", out.content);
        let out = skill(&ctx, &serde_json::json!({"name": "linked-dir"})).await;
        assert!(out.is_error, "{}", out.content);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_skill_root_is_ignored() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(outside.path().join("review")).unwrap();
        std::fs::write(
            outside.path().join("review/SKILL.md"),
            "---\nname: review\ndescription: Outside.\n---\n# Outside\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join(".agents")).unwrap();
        symlink(outside.path(), dir.path().join(".agents/skills")).unwrap();

        let ctx = context(dir.path());
        assert!(catalog_block_for(dir.path(), dir.path()).await.is_none());
        let out = skill(&ctx, &serde_json::json!({"name": "review"})).await;
        assert!(out.is_error, "{}", out.content);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn junction_skill_root_is_ignored_on_windows() {
        use std::process::Command;

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(outside.path().join("review")).unwrap();
        std::fs::write(
            outside.path().join("review/SKILL.md"),
            "---\nname: review\ndescription: Outside.\n---\n# Outside\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join(".agents")).unwrap();
        let link = dir.path().join(".agents/skills");
        // Junctions do not need Developer Mode, so this covers the common
        // Windows reparse-point bypass even when symlink creation is denied.
        let script = format!(
            "New-Item -ItemType Junction -Path '{}' -Target '{}' | Out-Null",
            link.display(),
            outside.path().display()
        );
        let created = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !created {
            return;
        }

        let ctx = context(dir.path());
        assert!(catalog_block_for(dir.path(), dir.path()).await.is_none());
        let out = skill(&ctx, &serde_json::json!({"name": "review"})).await;
        assert!(out.is_error, "{}", out.content);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn symlinked_skill_files_are_ignored_on_windows() {
        use std::os::windows::fs::{symlink_dir, symlink_file};

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(
            outside.path().join("SKILL.md"),
            "---\nname: leaked\ndescription: Leaked.\n---\n# Leaked\n",
        )
        .unwrap();
        let skill_dir = dir.path().join(".agents/skills/leaked");
        std::fs::create_dir_all(&skill_dir).unwrap();
        // Creating symlinks needs Developer Mode or elevation; skip otherwise.
        if symlink_file(outside.path().join("SKILL.md"), skill_dir.join("SKILL.md")).is_err() {
            return;
        }
        let _ = symlink_dir(outside.path(), dir.path().join(".agents/skills/linked-dir"));

        let ctx = context(dir.path());
        assert!(catalog_block_for(dir.path(), dir.path()).await.is_none());
        let out = skill(&ctx, &serde_json::json!({"name": "leaked"})).await;
        assert!(out.is_error, "{}", out.content);
        let out = skill(&ctx, &serde_json::json!({"name": "linked-dir"})).await;
        assert!(out.is_error, "{}", out.content);

        let root_workspace = tempfile::tempdir().unwrap();
        let root_outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root_outside.path().join("review")).unwrap();
        std::fs::write(
            root_outside.path().join("review/SKILL.md"),
            "---\nname: review\ndescription: Outside.\n---\n# Outside\n",
        )
        .unwrap();
        std::fs::create_dir_all(root_workspace.path().join(".agents")).unwrap();
        if symlink_dir(
            root_outside.path(),
            root_workspace.path().join(".agents/skills"),
        )
        .is_ok()
        {
            let ctx = context(root_workspace.path());
            assert!(
                catalog_block_for(root_workspace.path(), root_workspace.path())
                    .await
                    .is_none()
            );
            let out = skill(&ctx, &serde_json::json!({"name": "review"})).await;
            assert!(out.is_error, "{}", out.content);
        }
    }
}
