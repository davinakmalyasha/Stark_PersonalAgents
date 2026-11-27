//! Host side of the OpenCode `fetch`, `path`, and `Bun` Code Mode extensions.
//!
//! The Bun sidecar runs the confined program and the extension code; every
//! operation that touches the network or the filesystem comes back here, so
//! Stark's permission gates, plan/read-only guards, external-directory
//! approval, and file mutation path stay authoritative.
//!
//! Ports `packages/core/src/codemode/{fetch,bun,path}.ts` (MIT, OpenCode
//! authors). The worker keeps the byte-identical surface; this module owns the
//! substitution points for Stark's `FileAccess`/`FileWrite`/`Permission`
//! services.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use base64::Engine;
use serde_json::{Value, json};

use crate::{ToolContext, ToolOutcome};

#[derive(Debug, Clone)]
pub struct HostError {
    /// Node error code (`ENOENT`, `EISDIR`, `ENOTDIR`) when the program should
    /// be able to branch on it.
    pub code: Option<String>,
    pub message: String,
}

impl HostError {
    pub fn message(message: impl Into<String>) -> Self {
        Self {
            code: None,
            message: message.into(),
        }
    }

    pub fn coded(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: Some(code.to_string()),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileHandle {
    pub absolute: PathBuf,
    pub resource: String,
}

/// Per-execution host state: the run's tool context and opaque file handles.
/// The worker can only name handles this process created; it cannot forge a
/// path that skipped a permission check.
pub struct RunHost {
    pub ctx: ToolContext,
    handles: Mutex<HashMap<String, FileHandle>>,
    next_handle: AtomicU64,
}

impl RunHost {
    pub fn new(ctx: ToolContext) -> Arc<Self> {
        Arc::new(Self {
            ctx,
            handles: Mutex::new(HashMap::new()),
            next_handle: AtomicU64::new(1),
        })
    }

    fn insert(&self, absolute: PathBuf, resource: String) -> String {
        let id = format!("h{}", self.next_handle.fetch_add(1, Ordering::Relaxed));
        self.handles
            .lock()
            .unwrap()
            .insert(id.clone(), FileHandle { absolute, resource });
        id
    }

    fn handle(&self, id: &str) -> Result<FileHandle, HostError> {
        self.handles
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| HostError::message(format!("unknown Code Mode file handle: {id}")))
    }
}

fn blocked_error(outcome: ToolOutcome) -> HostError {
    HostError::message(outcome.content)
}

fn write_denial(ctx: &ToolContext) -> Option<HostError> {
    if ctx.mode == "plan" {
        return Some(HostError::message(
            "[BLOCKED] Plan mode is read-only. Type /build to leave plan mode. (Code Mode Bun writes are blocked.)",
        ));
    }
    if ctx.access == "read" {
        return Some(HostError::message(
            "[BLOCKED] Read-only agent cannot write files. Report findings or a proposed patch instead.",
        ));
    }
    None
}

/// Lexical path resolution plus containment. Paths outside the workspace need
/// the same `external_directory` approval as the shell tool.
async fn resolve(host: &RunHost, raw: &str) -> Result<(PathBuf, String, bool), HostError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(HostError::message("path is empty"));
    }
    let ctx = &host.ctx;
    let absolute = crate::paths::resolve_lexical(&ctx.current_dir(), raw);
    let workspace = ctx
        .workspace
        .canonicalize()
        .unwrap_or_else(|_| crate::paths::normalize(&ctx.workspace));
    let canonical = crate::paths::canonicalize_best_effort(&absolute);
    let inside = crate::paths::path_starts_with(&canonical, &workspace);
    if !inside {
        let label = absolute.to_string_lossy().to_string();
        if let Some(blocked) =
            crate::permissions::gate(ctx, "external_directory", &[label.clone()], &label).await
        {
            return Err(blocked_error(blocked));
        }
    }
    let resource = if inside {
        crate::paths::relative_display(&ctx.workspace, &absolute)
    } else {
        absolute.to_string_lossy().replace('\\', "/")
    };
    Ok((absolute, resource, !inside))
}

async fn gate_read(host: &RunHost, resource: &str, label: &str) -> Result<(), HostError> {
    if let Some(blocked) =
        crate::permissions::gate(&host.ctx, "read", &[resource.to_string()], label).await
    {
        return Err(blocked_error(blocked));
    }
    Ok(())
}

async fn gate_edit(host: &RunHost, resource: &str, label: &str) -> Result<(), HostError> {
    if let Some(denied) = write_denial(&host.ctx) {
        return Err(denied);
    }
    if let Some(blocked) =
        crate::permissions::gate(&host.ctx, "edit", &[resource.to_string()], label).await
    {
        return Err(blocked_error(blocked));
    }
    Ok(())
}

/// OpenCode `FileMutation.withLock`: serializes writes to the same path even
/// when a program starts them with `Promise.allSettled`.
fn path_lock(path: &Path) -> Arc<tokio::sync::Mutex<()>> {
    use std::sync::OnceLock;
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    let locks = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let key = crate::paths::canonicalize_best_effort(path);
    let mut map = locks.lock().unwrap();
    map.entry(key)
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

fn io_error(path: &Path, error: std::io::Error) -> HostError {
    match error.kind() {
        std::io::ErrorKind::NotFound => HostError::coded(
            "ENOENT",
            format!(
                "ENOENT: no such file or directory, open '{}'",
                path.display()
            ),
        ),
        std::io::ErrorKind::PermissionDenied => HostError::coded(
            "EACCES",
            format!("EACCES: permission denied, open '{}'", path.display()),
        ),
        _ => HostError::message(format!("{}: {error}", path.display())),
    }
}

fn last_modified(info: &std::fs::Metadata) -> u64 {
    info.modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or((2u64 << 52) - 1)
}

fn file_type(info: &std::fs::Metadata) -> &'static str {
    if info.is_file() {
        "file"
    } else if info.is_dir() {
        "directory"
    } else if info.file_type().is_symlink() {
        "symlink"
    } else {
        "other"
    }
}

fn text_of(bytes: &[u8]) -> Option<String> {
    std::str::from_utf8(bytes).ok().map(str::to_string)
}

fn diff_for(resource: &str, before: &str, after: &str, status: &str) -> Value {
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
        .header(&format!("a/{resource}"), &format!("b/{resource}"))
        .to_string();
    json!({
        "file": resource,
        "status": status,
        "additions": additions,
        "deletions": deletions,
        "patch": patch,
    })
}

async fn file_open(host: &RunHost, args: &[Value]) -> Result<Value, HostError> {
    let raw = args
        .first()
        .and_then(Value::as_str)
        .ok_or_else(|| HostError::message("Bun.file expects a path"))?;
    let (absolute, resource, _external) = resolve(host, raw).await?;
    gate_read(host, &resource, raw).await?;
    let info = match tokio::fs::metadata(&absolute).await {
        Ok(info) => Some(info),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(io_error(&absolute, error)),
    };
    let handle = host.insert(absolute.clone(), resource);
    Ok(json!({
        "handle": handle,
        "name": raw,
        "size": info.as_ref().map(|info| info.len()).unwrap_or(0),
        "lastModified": info.as_ref().map(last_modified).unwrap_or((2u64 << 52) - 1),
        "exists": info.is_some(),
    }))
}

async fn file_target(host: &RunHost, args: &[Value]) -> Result<Value, HostError> {
    let raw = args
        .first()
        .and_then(Value::as_str)
        .ok_or_else(|| HostError::message("Bun.write expects a path"))?;
    let (absolute, resource, _external) = resolve(host, raw).await?;
    let handle = host.insert(absolute.clone(), resource);
    Ok(json!({"handle": handle, "name": raw}))
}

async fn file_read(host: &RunHost, args: &[Value]) -> Result<Value, HostError> {
    let id = args
        .first()
        .and_then(Value::as_str)
        .ok_or_else(|| HostError::message("file_read expects a handle"))?;
    let handle = host.handle(id)?;
    let offset = args.get(1).and_then(Value::as_u64);
    let length = args.get(2).and_then(Value::as_u64);
    let bytes = if offset.is_none() && length.is_none() {
        tokio::fs::read(&handle.absolute)
            .await
            .map_err(|error| io_error(&handle.absolute, error))?
    } else {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        let mut file = tokio::fs::File::open(&handle.absolute)
            .await
            .map_err(|error| io_error(&handle.absolute, error))?;
        if let Some(offset) = offset {
            file.seek(std::io::SeekFrom::Start(offset))
                .await
                .map_err(|error| io_error(&handle.absolute, error))?;
        }
        let mut bytes = Vec::new();
        match length {
            Some(length) => {
                file.take(length)
                    .read_to_end(&mut bytes)
                    .await
                    .map_err(|error| io_error(&handle.absolute, error))?;
            }
            None => {
                file.read_to_end(&mut bytes)
                    .await
                    .map_err(|error| io_error(&handle.absolute, error))?;
            }
        }
        bytes
    };
    Ok(json!({
        "data_base64": base64::engine::general_purpose::STANDARD.encode(bytes),
    }))
}

async fn file_exists(host: &RunHost, args: &[Value]) -> Result<Value, HostError> {
    let id = args
        .first()
        .and_then(Value::as_str)
        .ok_or_else(|| HostError::message("file_exists expects a handle"))?;
    let handle = host.handle(id)?;
    Ok(json!({
        "exists": tokio::fs::metadata(&handle.absolute)
            .await
            .map(|info| info.is_file())
            .unwrap_or(false),
    }))
}

async fn file_stat(host: &RunHost, args: &[Value]) -> Result<Value, HostError> {
    let id = args
        .first()
        .and_then(Value::as_str)
        .ok_or_else(|| HostError::message("file_stat expects a handle"))?;
    let handle = host.handle(id)?;
    let info = tokio::fs::metadata(&handle.absolute)
        .await
        .map_err(|error| io_error(&handle.absolute, error))?;
    Ok(json!({
        "size": info.len(),
        "mtimeMs": last_modified(&info),
        "type": file_type(&info),
    }))
}

async fn file_write(host: &RunHost, args: &[Value]) -> Result<Value, HostError> {
    let id = args
        .first()
        .and_then(Value::as_str)
        .ok_or_else(|| HostError::message("file_write expects a handle"))?;
    let handle = host.handle(id)?;
    let data = args
        .get(1)
        .and_then(Value::as_str)
        .ok_or_else(|| HostError::message("file_write expects base64 data"))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|error| HostError::message(format!("invalid base64 file data: {error}")))?;
    let create_path = args.get(2).and_then(Value::as_bool).unwrap_or(true);
    let append = args.get(3).and_then(Value::as_bool).unwrap_or(false);
    let want_diff = args.get(4).and_then(Value::as_bool).unwrap_or(true);
    let lock = path_lock(&handle.absolute);
    let _guard = lock.lock().await;

    gate_edit(host, &handle.resource, &handle.resource).await?;
    let existing = match tokio::fs::metadata(&handle.absolute).await {
        Ok(info) => Some(info),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(io_error(&handle.absolute, error)),
    };
    if existing.as_ref().is_some_and(|info| info.is_dir()) {
        return Err(HostError::coded(
            "EISDIR",
            format!(
                "EISDIR: illegal operation on a directory, open '{}'",
                handle.absolute.display()
            ),
        ));
    }
    if let Some(parent) = handle.absolute.parent() {
        if create_path {
            if tokio::fs::metadata(parent).await.is_err() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|error| io_error(parent, error))?;
            }
        } else if tokio::fs::metadata(parent).await.is_err() {
            return Err(HostError::coded(
                "ENOENT",
                format!(
                    "ENOENT: no such file or directory, open '{}'",
                    handle.absolute.display()
                ),
            ));
        }
    }
    let before = if want_diff && !append {
        match tokio::fs::read(&handle.absolute).await {
            Ok(bytes) => text_of(&bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(String::new()),
            Err(error) => return Err(io_error(&handle.absolute, error)),
        }
    } else {
        None
    };
    if append {
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&handle.absolute)
            .await
            .map_err(|error| io_error(&handle.absolute, error))?;
        file.write_all(&bytes)
            .await
            .map_err(|error| io_error(&handle.absolute, error))?;
    } else {
        tokio::fs::write(&handle.absolute, &bytes)
            .await
            .map_err(|error| io_error(&handle.absolute, error))?;
    }
    let status = if existing.is_some() {
        "modified"
    } else {
        "added"
    };
    let diff = if append {
        let additions = text_of(&bytes)
            .map(|text| {
                if text.is_empty() {
                    0
                } else {
                    text.split('\n').count() - usize::from(text.ends_with('\n'))
                }
            })
            .unwrap_or(0);
        json!({"file": handle.resource, "status": status, "additions": additions, "deletions": 0})
    } else if want_diff {
        match (before, text_of(&bytes)) {
            (Some(before), Some(after)) => {
                if status == "modified" && before == after {
                    Value::Null
                } else {
                    diff_for(&handle.resource, &before, &after, status)
                }
            }
            _ => json!({"file": handle.resource, "status": status}),
        }
    } else {
        json!({"file": handle.resource, "status": status})
    };
    Ok(json!({
        "bytes": bytes.len(),
        "diff": diff,
    }))
}

async fn file_unlink(host: &RunHost, args: &[Value]) -> Result<Value, HostError> {
    let id = args
        .first()
        .and_then(Value::as_str)
        .ok_or_else(|| HostError::message("file_unlink expects a handle"))?;
    let handle = host.handle(id)?;
    let lock = path_lock(&handle.absolute);
    let _guard = lock.lock().await;
    gate_edit(host, &handle.resource, &handle.resource).await?;
    let info = tokio::fs::metadata(&handle.absolute)
        .await
        .map_err(|error| io_error(&handle.absolute, error))?;
    if info.is_dir() {
        return Err(HostError::coded(
            "EISDIR",
            format!(
                "EISDIR: illegal operation on a directory, unlink '{}'",
                handle.absolute.display()
            ),
        ));
    }
    tokio::fs::remove_file(&handle.absolute)
        .await
        .map_err(|error| io_error(&handle.absolute, error))?;
    Ok(json!({
        "diff": {"file": handle.resource, "status": "deleted"},
    }))
}

async fn glob_prepare(host: &RunHost, args: &[Value]) -> Result<Value, HostError> {
    let raw = args
        .first()
        .and_then(Value::as_str)
        .ok_or_else(|| HostError::message("glob_prepare expects a directory"))?;
    let (absolute, resource, _) = resolve(host, raw).await?;
    gate_read(host, &resource, raw).await?;
    Ok(json!({"root": absolute.to_string_lossy()}))
}

async fn fetch_check(host: &RunHost, args: &[Value]) -> Result<Value, HostError> {
    let url = args
        .first()
        .and_then(Value::as_str)
        .ok_or_else(|| HostError::message("fetch_check expects a URL"))?;
    let method = args
        .get(1)
        .and_then(Value::as_str)
        .unwrap_or("GET")
        .to_string();
    let redirect_only = args.get(2).and_then(Value::as_bool).unwrap_or(false);
    let label = format!("{method} {url}");
    if redirect_only {
        if crate::permissions::effect(&host.ctx, "webfetch", &[url.to_string()]).await
            == stark_policy::Effect::Deny
        {
            return Err(HostError::message(format!(
                "[BLOCKED] webfetch denied by permissions.json ({url})."
            )));
        }
    } else if let Some(blocked) =
        crate::permissions::gate(&host.ctx, "webfetch", &[url.to_string()], &label).await
    {
        return Err(blocked_error(blocked));
    }
    Ok(json!({"ok": true}))
}

pub async fn handle(host: &RunHost, name: &str, args: &[Value]) -> Result<Value, HostError> {
    match name {
        "file_open" => file_open(host, args).await,
        "file_target" => file_target(host, args).await,
        "file_read" => file_read(host, args).await,
        "file_exists" => file_exists(host, args).await,
        "file_stat" => file_stat(host, args).await,
        "file_write" => file_write(host, args).await,
        "file_unlink" => file_unlink(host, args).await,
        "glob_prepare" => glob_prepare(host, args).await,
        "fetch_check" => fetch_check(host, args).await,
        other => Err(HostError::message(format!(
            "unknown Code Mode host function: {other}"
        ))),
    }
}
