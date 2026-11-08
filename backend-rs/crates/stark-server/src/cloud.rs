//! Cloud handoffs: move one chat (or a whole workspace) to the VPS.
//!
//! A handoff is a scoped package, not a full backup: the chat row + messages,
//! its rollout trace, the portable workspace record (`id`/`rel`/`repo`, never
//! an absolute path), and the workspace memory index. It never contains
//! `.env`, job queues, logs, checkpoints, or WAL files.
//!
//! Flow: `POST /api/cloud/send` builds the package, points a `cloud/<slug>`
//! branch at `HEAD` when the workspace is a git repo (safe: no checkout, so
//! local work is untouched), and records a handoff. When `STARK_CLOUD_URL` +
//! `STARK_CLOUD_TOKEN` are set the package is pushed to the VPS
//! `POST /api/cloud/receive` (authed by the same pairing token); otherwise it
//! stays `staged` with the package path so the operator can carry it over.
//! The VPS imports by id, so canvas watches and delegations keep working.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zip::write::SimpleFileOptions;

pub const HANDOFF_FORMAT: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudHandoff {
    pub id: String,
    pub chat_id: Option<String>,
    pub workspace_id: String,
    pub workspace_rel: Option<String>,
    pub repo: Option<String>,
    pub branch: String,
    pub task: Option<String>,
    pub status: String,
    pub origin: String,
    pub package: String,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

fn cloud_dir(harness_root: &Path) -> PathBuf {
    harness_root.join(".deep_agents").join("cloud")
}

fn handoffs_path(harness_root: &Path) -> PathBuf {
    cloud_dir(harness_root).join("handoffs.json")
}

pub fn load_handoffs(harness_root: &Path) -> Vec<CloudHandoff> {
    std::fs::read_to_string(handoffs_path(harness_root))
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<CloudHandoff>>(&text).ok())
        .unwrap_or_default()
}

fn save_handoffs(harness_root: &Path, handoffs: &[CloudHandoff]) -> Result<(), String> {
    let dir = cloud_dir(harness_root);
    std::fs::create_dir_all(&dir).map_err(|error| format!("cannot create cloud dir: {error}"))?;
    let text = serde_json::to_string_pretty(handoffs)
        .map_err(|error| format!("cannot encode handoffs: {error}"))?;
    let path = handoffs_path(harness_root);
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text).map_err(|error| format!("cannot write handoffs: {error}"))?;
    std::fs::rename(&temp, &path).map_err(|error| format!("cannot write handoffs: {error}"))?;
    Ok(())
}

fn upsert_handoff(harness_root: &Path, handoff: CloudHandoff) -> Result<(), String> {
    let mut handoffs = load_handoffs(harness_root);
    if let Some(existing) = handoffs.iter_mut().find(|item| item.id == handoff.id) {
        *existing = handoff;
    } else {
        handoffs.push(handoff);
    }
    save_handoffs(harness_root, &handoffs)
}

/// URL-safe slug for branch names and labels.
pub fn slugify(title: &str) -> String {
    let mut slug = String::new();
    for chunk in title
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|chunk| !chunk.is_empty())
    {
        if !slug.is_empty() {
            slug.push('-');
        }
        slug.push_str(chunk);
    }
    if slug.is_empty() {
        slug.push_str("chat");
    }
    slug.chars().take(40).collect()
}

pub fn cloud_branch(title: &str, chat_id: Option<&str>) -> String {
    let slug = slugify(title);
    let short = chat_id
        .map(|id| {
            id.trim_start_matches("chat-")
                .trim_start_matches("del-")
                .chars()
                .take(6)
                .collect::<String>()
        })
        .filter(|short| !short.is_empty())
        .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string()[..6].to_string());
    format!("cloud/{slug}-{short}")
}

fn git(workspace: &Path, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(args)
        .output()
        .map_err(|error| format!("git failed: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("git {} failed", args.join(" "))
        } else {
            stderr
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Points `branch` at `HEAD` when the workspace is a git repo. Never checks
/// anything out, so local work is untouched. Non-repos are fine: the handoff
/// simply carries no branch.
pub fn ensure_cloud_branch(workspace_path: &Path, branch: &str) -> Result<bool, String> {
    if !workspace_path.is_dir() {
        return Ok(false);
    }
    if git(workspace_path, &["rev-parse", "--show-toplevel"]).is_err() {
        return Ok(false);
    }
    if git(workspace_path, &["rev-parse", "--verify", branch]).is_ok() {
        return Ok(true);
    }
    git(workspace_path, &["branch", branch, "HEAD"])?;
    Ok(true)
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn add_bytes(
    zip: &mut zip::ZipWriter<std::fs::File>,
    name: &str,
    bytes: &[u8],
    options: SimpleFileOptions,
) -> Result<(), String> {
    zip.start_file(name, options)
        .map_err(|error| format!("cannot add {name}: {error}"))?;
    zip.write_all(bytes)
        .map_err(|error| format!("cannot write {name}: {error}"))?;
    Ok(())
}

pub struct SendRequest {
    pub chat_id: Option<String>,
    pub workspace_id: String,
    pub task: Option<String>,
    pub branch: Option<String>,
}

/// Builds the handoff package and records it. Pushes to the VPS when
/// `STARK_CLOUD_URL` + `STARK_CLOUD_TOKEN` are configured; otherwise the
/// handoff stays `staged` with its package path.
pub async fn build_handoff(
    harness_root: &Path,
    store: &stark_store::ChatStore,
    request: SendRequest,
) -> Result<CloudHandoff, String> {
    let registry = stark_store::WorkspaceRegistry::new(harness_root);
    let record = registry
        .get(&request.workspace_id)
        .ok_or_else(|| format!("unknown workspace '{}'", request.workspace_id))?;
    let chat = match request.chat_id.as_deref() {
        Some(chat_id) => Some(
            store
                .get_chat(chat_id)
                .await
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("unknown chat '{chat_id}'"))?,
        ),
        None => None,
    };
    let title = chat
        .as_ref()
        .and_then(|chat| chat.get("title").and_then(Value::as_str))
        .or(request.task.as_deref())
        .unwrap_or(&record.name);
    let branch = request
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| cloud_branch(title, request.chat_id.as_deref()));
    let workspace_path = PathBuf::from(&record.path);
    let _ = ensure_cloud_branch(&workspace_path, &branch);

    let id = format!(
        "handoff-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    );
    let out_dir = cloud_dir(harness_root).join("out");
    std::fs::create_dir_all(&out_dir)
        .map_err(|error| format!("cannot create cloud out dir: {error}"))?;
    let package_path = out_dir.join(format!("{id}.zip"));
    let package_name = package_path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| format!("{id}.zip"));

    let manifest = json!({
        "format": HANDOFF_FORMAT,
        "kind": "chat-handoff",
        "id": id,
        "chat_id": request.chat_id,
        "workspace": {
            "id": record.id,
            "name": record.name,
            "root": record.root,
            "rel": record.rel,
            "repo": record.repo,
            "branch": branch,
        },
        "task": request.task,
        "origin": crate::server_origin(),
        "created_at": now_iso(),
        "excluded": [".env", "jobs.db", "checkpoints", "logs", "snapshots", "voice models", "*.db-wal"],
    });
    let file = std::fs::File::create(&package_path)
        .map_err(|error| format!("cannot create package: {error}"))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    add_bytes(
        &mut zip,
        "manifest.json",
        serde_json::to_vec_pretty(&manifest)
            .map_err(|error| format!("cannot encode manifest: {error}"))?
            .as_slice(),
        options,
    )?;
    if let Some(chat) = &chat {
        add_bytes(
            &mut zip,
            "chat.json",
            serde_json::to_vec_pretty(chat)
                .map_err(|error| format!("cannot encode chat: {error}"))?
                .as_slice(),
            options,
        )?;
    }
    if let Some(chat_id) = request.chat_id.as_deref()
        && let Some(rollout) = stark_store::rollout::find_rollout(harness_root, chat_id)
        && let Ok(bytes) = std::fs::read(&rollout)
    {
        add_bytes(&mut zip, "rollout.jsonl", &bytes, options)?;
    }
    let memory_index = harness_root
        .join(".deep_agents")
        .join("memory")
        .join(&record.id)
        .join("MEMORY.md");
    if let Ok(bytes) = std::fs::read(&memory_index) {
        add_bytes(&mut zip, "memory.md", &bytes, options)?;
    }
    zip.finish()
        .map_err(|error| format!("cannot finish package: {error}"))?;

    let mut handoff = CloudHandoff {
        id: id.clone(),
        chat_id: request.chat_id.clone(),
        workspace_id: record.id.clone(),
        workspace_rel: record.rel.clone(),
        repo: record.repo.clone(),
        branch,
        task: request.task.clone(),
        status: "staged".to_string(),
        origin: crate::server_origin(),
        package: package_name,
        error: None,
        created_at: now_iso(),
        updated_at: now_iso(),
    };
    // Push when a cloud target is configured; otherwise the package waits
    // for a manual carry (Settings import or direct receive call).
    match push_package(harness_root, &package_path).await {
        Ok(()) => {
            handoff.status = "sent".to_string();
            handoff.updated_at = now_iso();
        }
        Err(error) if error == "cloud-unconfigured" => {}
        Err(error) => {
            handoff.status = "failed".to_string();
            handoff.error = Some(error);
            handoff.updated_at = now_iso();
        }
    }
    upsert_handoff(harness_root, handoff.clone())?;
    Ok(handoff)
}

fn cloud_target() -> Option<(String, String)> {
    let url = std::env::var("STARK_CLOUD_URL")
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())?;
    let token = std::env::var("STARK_CLOUD_TOKEN")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())?;
    Some((url, token))
}

pub fn cloud_configured() -> bool {
    cloud_target().is_some()
}

pub fn cloud_host() -> Option<String> {
    cloud_target().map(|(url, _)| {
        url.replace("http://", "")
            .replace("https://", "")
            .split('/')
            .next()
            .unwrap_or("")
            .to_string()
    })
}

async fn push_package(harness_root: &Path, package: &Path) -> Result<(), String> {
    let Some((url, token)) = cloud_target() else {
        return Err("cloud-unconfigured".to_string());
    };
    let bytes = tokio::fs::read(package)
        .await
        .map_err(|error| format!("cannot read package: {error}"))?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|error| format!("cannot build http client: {error}"))?;
    let response = client
        .post(format!("{url}/api/cloud/receive"))
        .header("x-api-token", token)
        .header("content-type", "application/zip")
        .body(bytes)
        .send()
        .await
        .map_err(|error| format!("cloud push failed: {error}"))?;
    if !response.status().is_success() {
        let detail = response.text().await.unwrap_or_default();
        let _ = harness_root;
        return Err(format!(
            "cloud rejected the handoff: {}",
            detail.chars().take(300).collect::<String>()
        ));
    }
    Ok(())
}

/// Imports a handoff package on the receiving machine. Workspaces keep their
/// id; the path resolves to this machine's projects root, the folder is
/// re-cloned from the stored repo when missing, and chats upsert by id.
pub async fn receive_package(
    harness_root: &Path,
    store: &stark_store::ChatStore,
    bytes: &[u8],
) -> Result<CloudHandoff, String> {
    if bytes.len() > 512 * 1024 * 1024 {
        return Err("package too large (512 MiB max)".to_string());
    }
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|error| format!("invalid package: {error}"))?;
    let manifest: Value = {
        let mut entry = archive
            .by_name("manifest.json")
            .map_err(|_| "package has no manifest".to_string())?;
        let mut text = String::new();
        std::io::Read::read_to_string(&mut entry, &mut text)
            .map_err(|error| format!("cannot read manifest: {error}"))?;
        serde_json::from_str(&text).map_err(|_| "invalid manifest".to_string())?
    };
    if manifest.get("format").and_then(Value::as_u64) != Some(HANDOFF_FORMAT as u64) {
        return Err("unsupported handoff format".to_string());
    }
    if manifest.get("kind").and_then(Value::as_str) != Some("chat-handoff") {
        return Err("not a chat handoff".to_string());
    }
    let workspace_manifest = manifest.get("workspace").cloned().unwrap_or(Value::Null);
    let workspace_id = workspace_manifest
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| "handoff has no workspace id".to_string())?;
    let branch = workspace_manifest
        .get("branch")
        .and_then(Value::as_str)
        .filter(|branch| !branch.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "cloud/import".to_string());

    let registry = stark_store::WorkspaceRegistry::new(harness_root);
    let record = if let Some(existing) = registry.get(workspace_id) {
        existing
    } else {
        let name = workspace_manifest
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Cloud project");
        let rel = workspace_manifest
            .get("rel")
            .and_then(Value::as_str)
            .filter(|rel| !rel.is_empty());
        let root = stark_store::projects_root();
        let path = match rel {
            Some(rel) => rel
                .split('/')
                .fold(root, |path, part| path.join(part))
                .to_string_lossy()
                .to_string(),
            None => root.join(name).to_string_lossy().to_string(),
        };
        let stored = stark_store::WorkspaceRecord {
            id: workspace_id.to_string(),
            name: name.to_string(),
            path: path.clone(),
            root: workspace_manifest
                .get("root")
                .and_then(Value::as_str)
                .map(str::to_string),
            rel: rel.map(str::to_string),
            repo: workspace_manifest
                .get("repo")
                .and_then(Value::as_str)
                .map(str::to_string),
            branch: None,
            added_at: now_iso(),
        };
        registry.upsert_record(stored)?
    };
    // Missing folder + stored repo: shallow re-clone so the chat has a home.
    if !Path::new(&record.path).is_dir()
        && let Some(repo) = record.repo.as_deref().filter(|repo| !repo.is_empty())
    {
        let path = PathBuf::from(&record.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create workspace dir: {error}"))?;
        }
        let output = std::process::Command::new("git")
            .arg("clone")
            .arg("--depth")
            .arg("1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .arg(repo)
            .arg(&path)
            .output()
            .map_err(|error| format!("git clone failed: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "cannot clone {}: {}",
                repo,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    let _ = ensure_cloud_branch(Path::new(&record.path), &branch);

    let mut chat_id: Option<String> = manifest
        .get("chat_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Ok(mut entry) = archive.by_name("chat.json") {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut entry, &mut text)
            .map_err(|error| format!("cannot read chat: {error}"))?;
        let mut chat: Value =
            serde_json::from_str(&text).map_err(|_| "invalid chat snapshot".to_string())?;
        if chat.get("id").and_then(Value::as_str).is_none() {
            chat["id"] = chat_id.clone().map(Value::String).unwrap_or(Value::Null);
        }
        chat_id = chat.get("id").and_then(Value::as_str).map(str::to_string);
        store
            .import_chat_snapshot(&chat, &record.id, &record.path)
            .await
            .map_err(|error| error.to_string())?;
    }
    if let Some(chat_id) = chat_id.as_deref()
        && stark_store::rollout::find_rollout(harness_root, chat_id).is_none()
        && let Ok(mut entry) = archive.by_name("rollout.jsonl")
    {
        let mut bytes = Vec::new();
        if std::io::Read::read_to_end(&mut entry, &mut bytes).is_ok()
            && !bytes.is_empty()
            && !stark_store::rollout::parse_rollout_text(&String::from_utf8_lossy(&bytes))
                .is_empty()
        {
            let target = stark_store::rollout::new_rollout_path(harness_root, chat_id);
            if let Some(parent) = target.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&target, bytes);
        }
    }
    if let Ok(mut entry) = archive.by_name("memory.md") {
        let mut bytes = Vec::new();
        if std::io::Read::read_to_end(&mut entry, &mut bytes).is_ok() && !bytes.is_empty() {
            let target = harness_root
                .join(".deep_agents")
                .join("memory")
                .join(&record.id)
                .join("MEMORY.md");
            if !target.is_file() {
                if let Some(parent) = target.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(&target, bytes);
            }
        }
    }

    let handoff = CloudHandoff {
        id: manifest
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| {
                format!(
                    "handoff-{}",
                    &uuid::Uuid::new_v4().simple().to_string()[..12]
                )
            }),
        chat_id,
        workspace_id: record.id.clone(),
        workspace_rel: record.rel.clone(),
        repo: record.repo.clone(),
        branch,
        task: manifest
            .get("task")
            .and_then(Value::as_str)
            .map(str::to_string),
        status: "received".to_string(),
        origin: manifest
            .get("origin")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| "unknown".to_string()),
        package: String::new(),
        error: None,
        created_at: manifest
            .get("created_at")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(now_iso),
        updated_at: now_iso(),
    };
    upsert_handoff(harness_root, handoff.clone())?;
    Ok(handoff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_and_branch_are_safe() {
        assert_eq!(slugify("Fix login crash!!"), "fix-login-crash");
        assert_eq!(slugify(""), "chat");
        let branch = cloud_branch("Fix login crash", Some("chat-abcdef123456"));
        assert!(branch.starts_with("cloud/fix-login-crash-"));
        assert!(!branch.contains(' '));
    }

    #[test]
    fn non_git_workspace_skips_branch() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            ensure_cloud_branch(dir.path(), "cloud/test-123456").unwrap(),
            false
        );
    }

    #[test]
    fn handoff_store_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let handoff = CloudHandoff {
            id: "handoff-test".to_string(),
            chat_id: Some("chat-abc".to_string()),
            workspace_id: "ws-1".to_string(),
            workspace_rel: Some("demo".to_string()),
            repo: None,
            branch: "cloud/demo-abc".to_string(),
            task: None,
            status: "staged".to_string(),
            origin: "local".to_string(),
            package: "handoff-test.zip".to_string(),
            error: None,
            created_at: now_iso(),
            updated_at: now_iso(),
        };
        upsert_handoff(dir.path(), handoff).unwrap();
        let loaded = load_handoffs(dir.path());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].branch, "cloud/demo-abc");
    }

    #[tokio::test]
    async fn handoff_package_roundtrips_between_machines() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let store_a = stark_store::ChatStore::open(dir_a.path()).await.unwrap();
        let registry_a = stark_store::WorkspaceRegistry::new(dir_a.path());
        let project = dir_a.path().join("demo-proj");
        std::fs::create_dir_all(&project).unwrap();
        let record = registry_a
            .add(&project.to_string_lossy(), "demo-proj")
            .unwrap();
        let created = store_a
            .create_chat(&record.id, &record.path, "Roundtrip", "test-model")
            .await
            .unwrap();
        let chat_id = created["id"].as_str().unwrap().to_string();
        store_a
            .add_message(&chat_id, "user", "hello cloud", None)
            .await
            .unwrap();
        store_a
            .add_message(&chat_id, "agent", "hello local", None)
            .await
            .unwrap();

        let handoff = build_handoff(
            dir_a.path(),
            &store_a,
            SendRequest {
                chat_id: Some(chat_id.clone()),
                workspace_id: record.id.clone(),
                task: Some("take over".to_string()),
                branch: None,
            },
        )
        .await
        .unwrap();
        assert!(handoff.branch.starts_with("cloud/roundtrip-"));
        // No cloud target in tests: the package waits staged.
        assert_eq!(handoff.status, "staged");
        let package = cloud_dir(dir_a.path()).join("out").join(&handoff.package);
        assert!(package.is_file());
        let bytes = std::fs::read(&package).unwrap();

        let store_b = stark_store::ChatStore::open(dir_b.path()).await.unwrap();
        let received = receive_package(dir_b.path(), &store_b, &bytes)
            .await
            .unwrap();
        assert_eq!(received.status, "received");
        assert_eq!(received.workspace_id, record.id);
        let imported = store_b.get_chat(&chat_id).await.unwrap().unwrap();
        let texts: Vec<&str> = imported["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|message| message.get("content").and_then(Value::as_str))
            .collect();
        assert!(texts.contains(&"hello cloud"));
        assert!(texts.contains(&"hello local"));
    }
}
