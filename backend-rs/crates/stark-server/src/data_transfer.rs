//! Portable data export/import.
//!
//! Export writes one zip containing the durable harness state needed to rebuild
//! this instance on another machine: a consistent `chats.db` snapshot, sessions,
//! config, memory, uploads, plans, skills, schedules, groups, and a portable
//! `workspaces.json`. It never includes `.env` (provider keys stay on the
//! machine), the transient job queue, logs, WAL files, or old backups.
//!
//! Import is staged and applied on the next boot: replacing a live SQLite file
//! under the connection pool is unsafe, so the endpoint extracts to a staging
//! directory and writes a marker. `apply_pending_import` performs the swap
//! before the store opens, after backing up the current files.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use zip::write::SimpleFileOptions;

const EXPORT_FORMAT: u32 = 1;

/// Durable harness state that must travel. Paths are relative to the harness
/// root. Everything here is either user-authored or needed to resume work.
const DURABLE: &[&str] = &[
    ".deep_agents/config",
    ".deep_agents/sessions",
    ".deep_agents/memory",
    ".deep_agents/uploads",
    ".deep_agents/skills",
    ".deep_agents/tasks",
    ".deep_agents/groups",
    ".deep_agents/mcp_servers.json",
    ".deep_agents/ui_state.json",
    ".deep_agents/agent_notes.json",
    ".deep_agents/workspaces.json",
    ".deep_agents/AGENTS.md",
    ".deep_agents/AGENTS.override.md",
    ".deep_agents/dfd_clients.json",
    ".deep_agents/house_state.json",
];

/// Deliberately not portable. Listed in the manifest so an operator can audit
/// exactly what a backup leaves behind.
const EXCLUDED: &[&str] = &[
    ".env",
    ".deep_agents/jobs.db",
    ".deep_agents/backups",
    ".deep_agents/exports",
    ".deep_agents/imports",
    ".deep_agents/logs",
    ".deep_agents/snapshots",
    "voice models",
    "*.db-wal",
    "*.db-shm",
];

/// Build the export zip and return its path.
pub async fn export(
    harness_root: &Path,
    store: &stark_store::ChatStore,
) -> Result<PathBuf, String> {
    let exports = harness_root.join(".deep_agents").join("exports");
    tokio::fs::create_dir_all(&exports)
        .await
        .map_err(|error| format!("cannot create export dir: {error}"))?;
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let zip_path = exports.join(format!("stark-backup-{stamp}.zip"));

    // Consistent DB snapshot: never zip a live SQLite file + WAL.
    let snapshot = exports.join(format!(".snapshot-{stamp}.db"));
    store
        .vacuum_into(&snapshot)
        .await
        .map_err(|error| format!("cannot snapshot chats.db: {error}"))?;

    let file =
        std::fs::File::create(&zip_path).map_err(|error| format!("cannot create zip: {error}"))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    // Manifest last: it records the exact entries written, so the archive can
    // be audited without opening every file.
    let mut included: Vec<String> = Vec::new();

    // DB snapshot under its live name inside `.deep_agents/`.
    add_file(
        &mut zip,
        &snapshot,
        ".deep_agents/chats.db",
        options,
        &mut included,
    )?;
    let _ = std::fs::remove_file(&snapshot);

    // Portable registry: stored records keep the `%PROJECTS_ROOT%` token.
    let registry = stark_store::WorkspaceRegistry::new(harness_root);
    let records = registry.stored_records();
    let portable = serde_json::to_string_pretty(&json!({ "workspaces": records }))
        .map_err(|error| format!("cannot encode workspaces: {error}"))?;
    add_bytes(
        &mut zip,
        ".deep_agents/workspaces.json",
        portable.as_bytes(),
        options,
    )?;
    included.push(".deep_agents/workspaces.json".to_string());

    // Schedules use their own SQLite file; snapshot it instead of copying WAL.
    let schedules = harness_root.join(".deep_agents").join("schedules.db");
    if schedules.is_file() {
        let snapshot_path = exports.join(format!(".schedules-{stamp}.db"));
        match stark_store::vacuum_sqlite(&schedules, &snapshot_path).await {
            Ok(()) => {
                add_file(
                    &mut zip,
                    &snapshot_path,
                    ".deep_agents/schedules.db",
                    options,
                    &mut included,
                )?;
                let _ = std::fs::remove_file(&snapshot_path);
            }
            Err(error) => {
                tracing::warn!("schedules snapshot failed; exporting the last checkpoint: {error}");
                add_file(
                    &mut zip,
                    &schedules,
                    ".deep_agents/schedules.db",
                    options,
                    &mut included,
                )?;
            }
        }
    }

    for entry in DURABLE {
        if *entry == ".deep_agents/workspaces.json" {
            continue; // written above in portable form
        }
        let path = harness_root.join(entry);
        if path.is_file() {
            add_file(&mut zip, &path, entry, options, &mut included)?;
        } else if path.is_dir() {
            add_dir(&mut zip, &path, entry, options, &mut included).await?;
        }
    }

    included.sort();
    included.push("manifest.json".to_string());
    let manifest = json!({
        "format": EXPORT_FORMAT,
        "app": "stark",
        "contract_version": stark_tools::contract().contract_version,
        "exported_at": chrono::Utc::now().to_rfc3339(),
        "projects_root": stark_store::projects_root().to_string_lossy(),
        "included": included,
        "excluded": EXCLUDED,
    });
    add_bytes(
        &mut zip,
        "manifest.json",
        serde_json::to_string_pretty(&manifest)
            .unwrap_or_default()
            .as_bytes(),
        options,
    )?;

    zip.finish()
        .map_err(|error| format!("cannot finish zip: {error}"))?;

    // Keep the newest three exports; they can be large.
    prune_exports(&exports, 3);
    Ok(zip_path)
}

fn add_file(
    zip: &mut zip::ZipWriter<std::fs::File>,
    path: &Path,
    name: &str,
    options: SimpleFileOptions,
    included: &mut Vec<String>,
) -> Result<(), String> {
    let bytes =
        std::fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    included.push(name.to_string());
    add_bytes(zip, name, &bytes, options)
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
        .map_err(|error| format!("cannot write {name}: {error}"))
}

async fn add_dir(
    zip: &mut zip::ZipWriter<std::fs::File>,
    root: &Path,
    prefix: &str,
    options: SimpleFileOptions,
    included: &mut Vec<String>,
) -> Result<(), String> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .map_err(|error| format!("cannot list {}: {error}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("cannot read entry: {error}"))?;
            let path = entry.path();
            let name = path
                .strip_prefix(root)
                .map_err(|_| "path escaped export root".to_string())?;
            let name = format!("{prefix}/{}", name.to_string_lossy().replace('\\', "/"));
            let file_type = entry
                .file_type()
                .map_err(|error| format!("cannot stat: {error}"))?;
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() {
                // Skip sqlite sidecars anywhere in the tree.
                let lower = name.to_ascii_lowercase();
                if lower.ends_with(".db-wal") || lower.ends_with(".db-shm") {
                    continue;
                }
                if lower.ends_with(".db") {
                    let snapshot = std::env::temp_dir().join(format!(
                        "stark-export-{}-{}.db",
                        std::process::id(),
                        uuid::Uuid::new_v4().simple()
                    ));
                    match stark_store::vacuum_sqlite(&path, &snapshot).await {
                        Ok(()) => {
                            add_file(zip, &snapshot, &name, options, included)?;
                            let _ = std::fs::remove_file(&snapshot);
                        }
                        Err(error) => {
                            tracing::warn!(
                                "sqlite snapshot failed for {}; copying last checkpoint: {error}",
                                name
                            );
                            add_file(zip, &path, &name, options, included)?;
                        }
                    }
                } else {
                    add_file(zip, &path, &name, options, included)?;
                }
            }
        }
    }
    Ok(())
}

fn prune_exports(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("stark-backup-")
                        && (name.ends_with(".zip") || name.ends_with(".zip.enc"))
                })
        })
        .collect();
    files.sort();
    while files.len() > keep {
        let oldest = files.remove(0);
        let _ = std::fs::remove_file(oldest);
    }
}

/// One scheduled snapshot: export, optional AES-256 encryption, prune.
///
/// Encryption uses `openssl enc` (present on the Ubuntu runner and most
/// servers) with the passphrase read from `STARK_SNAPSHOT_PASSPHRASE` in the
/// environment — the secret is never passed on the command line. If OpenSSL is
/// missing the snapshot stays plaintext and a warning is logged.
pub async fn snapshot_once(
    harness_root: &Path,
    store: &stark_store::ChatStore,
) -> Result<PathBuf, String> {
    let zip = export(harness_root, store).await?;
    let Some(passphrase) = std::env::var("STARK_SNAPSHOT_PASSPHRASE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        return Ok(zip);
    };
    let encrypted = zip.with_extension("zip.enc");
    let _ = std::fs::remove_file(&encrypted);
    let status = std::process::Command::new("openssl")
        .args([
            "enc",
            "-aes-256-cbc",
            "-pbkdf2",
            "-salt",
            "-in",
            &zip.to_string_lossy(),
            "-out",
            &encrypted.to_string_lossy(),
            "-pass",
            "env:STARK_SNAPSHOT_PASSPHRASE",
        ])
        .env("STARK_SNAPSHOT_PASSPHRASE", &passphrase)
        .status();
    match status {
        Ok(status) if status.success() => {
            let _ = std::fs::remove_file(&zip);
            if let Ok(dir) = std::env::var("STARK_SNAPSHOT_DIR") {
                let dest = PathBuf::from(dir.trim());
                if !dest.as_os_str().is_empty() {
                    let _ = std::fs::create_dir_all(&dest);
                    let name = encrypted
                        .file_name()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| PathBuf::from("stark-backup.zip.enc"));
                    if let Err(error) = std::fs::copy(&encrypted, dest.join(name)) {
                        tracing::warn!("snapshot copy to {} failed: {error}", dest.display());
                    }
                }
            }
            Ok(encrypted)
        }
        _ => {
            tracing::warn!(
                "snapshot encryption unavailable (openssl failed); keeping plaintext {}",
                zip.display()
            );
            Ok(zip)
        }
    }
}

/// Extract an uploaded zip into a staging directory and mark it for the next
/// boot. Returns the staged path and the manifest.
pub fn stage_import(
    harness_root: &Path,
    zip_bytes: &[u8],
    to_root: Option<&str>,
) -> Result<Value, String> {
    if zip_bytes.is_empty() {
        return Err("empty upload".to_string());
    }
    let id = uuid::Uuid::new_v4().to_string();
    let staging = harness_root.join(".deep_agents").join("imports").join(&id);
    let data = staging.join("data");
    std::fs::create_dir_all(&data).map_err(|error| format!("cannot create staging: {error}"))?;

    let reader = std::io::Cursor::new(zip_bytes);
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|error| format!("not a valid zip: {error}"))?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("cannot read zip entry: {error}"))?;
        let Some(relative) = entry.enclosed_name() else {
            return Err("zip contains an unsafe path".to_string());
        };
        let target = data.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&target)
                .map_err(|error| format!("cannot create {}: {error}", target.display()))?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
        let mut out = std::fs::File::create(&target)
            .map_err(|error| format!("cannot write {}: {error}", target.display()))?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|error| format!("cannot extract {}: {error}", target.display()))?;
    }

    let manifest_path = data.join("manifest.json");
    if !manifest_path.is_file() {
        return Err("backup is missing manifest.json".to_string());
    }
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(&manifest_path)
            .map_err(|error| format!("cannot read manifest: {error}"))?,
    )
    .map_err(|error| format!("manifest is not valid JSON: {error}"))?;
    if manifest.get("app").and_then(Value::as_str) != Some("stark") {
        return Err("manifest does not identify a Stark backup".to_string());
    }
    if !data.join(".deep_agents").join("chats.db").is_file() {
        return Err("backup is missing .deep_agents/chats.db".to_string());
    }

    let from_root = manifest
        .get("projects_root")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let new_root = to_root
        .map(str::trim)
        .filter(|root| !root.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| stark_store::projects_root().to_string_lossy().to_string());

    let marker = harness_root
        .join(".deep_agents")
        .join("pending_import.json");
    let payload = json!({
        "staging": staging.to_string_lossy(),
        "from_root": from_root,
        "to_root": new_root,
        "created_at": chrono::Utc::now().to_rfc3339(),
    });
    std::fs::write(
        &marker,
        serde_json::to_string_pretty(&payload).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("cannot write import marker: {error}"))?;

    Ok(json!({
        "ok": true,
        "staged": true,
        "from_root": from_root,
        "to_root": new_root,
        "manifest": manifest,
        "restart_required": true,
    }))
}

/// Apply a staged import before the store opens. Returns the root rebase pair
/// so the caller can rewrite chat rows after opening the database.
pub fn apply_pending_import(harness_root: &Path) -> Option<(PathBuf, PathBuf)> {
    let marker = harness_root
        .join(".deep_agents")
        .join("pending_import.json");
    let text = std::fs::read_to_string(&marker).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let staging = PathBuf::from(value.get("staging")?.as_str()?);
    let data = staging.join("data");
    if !data.is_dir() {
        let _ = std::fs::remove_file(&marker);
        return None;
    }
    let from_root = PathBuf::from(value.get("from_root").and_then(Value::as_str).unwrap_or(""));
    let to_root = PathBuf::from(
        value
            .get("to_root")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| {
                std::env::var("STARK_PROJECTS_ROOT")
                    .unwrap_or_else(|_| stark_store::projects_root().to_string_lossy().to_string())
            }),
    );

    // Back up current state before replacing anything.
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let backup = harness_root
        .join(".deep_agents")
        .join("backups")
        .join(format!("pre-import-{stamp}"));
    if std::fs::create_dir_all(&backup).is_ok() {
        for name in ["chats.db", "workspaces.json"] {
            let current = harness_root.join(".deep_agents").join(name);
            if current.is_file() {
                let _ = std::fs::copy(&current, backup.join(name));
            }
        }
        for dir in ["sessions", "config", "memory", "uploads"] {
            let current = harness_root.join(".deep_agents").join(dir);
            if current.is_dir() {
                let _ = copy_dir(&current, &backup.join(dir));
            }
        }
    }

    // Copy staged content over the harness (files replace, directories merge).
    copy_dir(
        &data.join(".deep_agents"),
        &harness_root.join(".deep_agents"),
    )
    .ok()?;

    // Rewrite absolute workspace paths to the new root.
    let registry_path = harness_root.join(".deep_agents").join("workspaces.json");
    if let Ok(raw) = std::fs::read_to_string(&registry_path)
        && let Ok(mut parsed) = serde_json::from_str::<Value>(&raw)
        && let Some(records) = parsed.get_mut("workspaces").and_then(Value::as_array_mut)
    {
        let mut changed = false;
        for record in records.iter_mut() {
            let Ok(mut typed) =
                serde_json::from_value::<stark_store::WorkspaceRecord>(record.clone())
            else {
                continue;
            };
            if typed.path.is_empty() {
                continue;
            }
            let rebased = stark_store::rebase_record(typed.clone(), &from_root, &to_root);
            if rebased.path != typed.path || rebased.root != typed.root {
                typed = rebased;
                if let Ok(value) = serde_json::to_value(&typed) {
                    *record = value;
                    changed = true;
                }
            }
        }
        if changed && let Ok(pretty) = serde_json::to_string_pretty(&parsed) {
            let _ = std::fs::write(&registry_path, pretty);
        }
    }

    // Consume the marker and staging so a crash cannot re-import stale data.
    let _ = std::fs::remove_file(&marker);
    let _ = std::fs::remove_dir_all(&staging);
    Some((from_root, to_root))
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if file_type.is_file() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn export_is_portable_and_never_ships_keys() {
        let dir = tempfile::tempdir().unwrap();
        let harness = dir.path();
        std::fs::write(harness.join(".env"), "OPENCODE_API_KEY=secret").unwrap();
        for (path, body) in [
            (".deep_agents/config/providers.json", "{\"providers\":{}}"),
            (".deep_agents/AGENTS.md", "global rules"),
            (".deep_agents/tasks/task_chat.json", "{\"plan\":[]}"),
            (".deep_agents/skills/review/SKILL.md", "review skill"),
            (".deep_agents/dfd_clients.json", "[]"),
            (".deep_agents/house_state.json", "{}"),
            (".deep_agents/groups/demo/PROJECT.md", "group"),
        ] {
            let target = harness.join(path);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(target, body).unwrap();
        }
        // Valid SQLite stores: one durable, one transient.
        stark_store::ScheduleStore::open(harness).await.unwrap();
        stark_store::JobStore::open(harness).await.unwrap();
        let store = stark_store::ChatStore::open(harness).await.unwrap();
        let zip_path = export(harness, &store).await.expect("export");
        let file = std::fs::File::open(&zip_path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut names = Vec::new();
        for index in 0..archive.len() {
            names.push(archive.by_index(index).unwrap().name().unwrap().to_string());
        }
        for expected in [
            "manifest.json",
            ".deep_agents/chats.db",
            ".deep_agents/workspaces.json",
            ".deep_agents/config/providers.json",
            ".deep_agents/AGENTS.md",
            ".deep_agents/tasks/task_chat.json",
            ".deep_agents/skills/review/SKILL.md",
            ".deep_agents/dfd_clients.json",
            ".deep_agents/house_state.json",
            ".deep_agents/schedules.db",
            ".deep_agents/groups/demo/PROJECT.md",
        ] {
            assert!(
                names.contains(&expected.to_string()),
                "missing {expected}: {names:?}"
            );
        }
        assert!(
            !names.iter().any(|name| name.ends_with(".env")),
            "export must never contain .env: {names:?}"
        );
        assert!(
            !names.iter().any(|name| name.ends_with("jobs.db")),
            "transient jobs must stay local: {names:?}"
        );
        assert!(
            !names
                .iter()
                .any(|name| name.contains("-wal") || name.contains("-shm"))
        );

        let mut manifest_file = archive.by_name("manifest.json").unwrap();
        let mut manifest_text = String::new();
        std::io::Read::read_to_string(&mut manifest_file, &mut manifest_text).unwrap();
        let manifest: Value = serde_json::from_str(&manifest_text).unwrap();
        let included = manifest["included"].as_array().unwrap();
        assert!(
            included
                .iter()
                .any(|entry| entry.as_str() == Some(".deep_agents/schedules.db"))
        );
        let excluded = manifest["excluded"].as_array().unwrap();
        assert!(
            excluded
                .iter()
                .any(|entry| entry.as_str() == Some(".deep_agents/jobs.db"))
        );
        assert!(excluded.iter().any(|entry| entry.as_str() == Some(".env")));
    }

    #[test]
    fn stage_import_rejects_zip_slip() {
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buffer);
            let options = SimpleFileOptions::default();
            zip.start_file("../escape.txt", options).unwrap();
            zip.write_all(b"nope").unwrap();
            zip.finish().unwrap();
        }
        let dir = tempfile::tempdir().unwrap();
        let error = stage_import(dir.path(), buffer.get_ref(), None).unwrap_err();
        assert!(error.contains("unsafe path"), "{error}");
    }

    #[test]
    fn stage_import_requires_stark_manifest() {
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buffer);
            let options = SimpleFileOptions::default();
            zip.start_file("manifest.json", options).unwrap();
            zip.write_all(br#"{"app":"not-stark"}"#).unwrap();
            zip.finish().unwrap();
        }
        let dir = tempfile::tempdir().unwrap();
        let error = stage_import(dir.path(), buffer.get_ref(), None).unwrap_err();
        assert!(error.contains("Stark backup"), "{error}");
    }
}
