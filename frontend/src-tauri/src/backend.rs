use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tauri::{AppHandle, Manager};

use crate::ShellState;

fn default_harness_root() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(r"D:\StarkData")
    }
    #[cfg(not(windows))]
    {
        std::env::var("HOME")
            .map(|home| PathBuf::from(home).join(".stark-data"))
            .unwrap_or_else(|_| PathBuf::from(".stark-data"))
    }
}

fn pointer_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|dir| dir.join("harness_root.json"))
}

fn read_pointer(path: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let raw = value
        .get("path")
        .and_then(serde_json::Value::as_str)?
        .trim();
    if raw.is_empty() {
        return None;
    }
    Some(PathBuf::from(raw))
}

fn write_pointer(path: &Path, root: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create settings dir: {error}"))?;
    }
    let text = serde_json::to_string_pretty(&serde_json::json!({
        "path": root.to_string_lossy(),
    }))
    .map_err(|error| format!("cannot encode harness pointer: {error}"))?;
    // Atomic: temp in same dir + rename.
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text).map_err(|error| format!("cannot write pointer: {error}"))?;
    std::fs::rename(&temp, path).map_err(|error| format!("cannot save pointer: {error}"))?;
    Ok(())
}

fn ensure_writable_dir(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path)
        .map_err(|error| format!("cannot create {}: {error}", path.display()))?;
    let probe = path.join(".write_test");
    match std::fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(error) => Err(format!("{} is not writable: {error}", path.display())),
    }
}

/// Resolve the harness root. Fail-closed: an explicit env var that points
/// nowhere, or a missing/unwritable `D:\StarkData`, is an error — never a
/// silent fallback to the repo or AppData. The launch cwd is never consulted.
fn root_dir(app: &AppHandle) -> Result<PathBuf, String> {
    // 1. Explicit override (dev / advanced). Must exist and be writable.
    for key in ["STARK_HARNESS_ROOT", "STARK_ROOT"] {
        if let Ok(raw) = std::env::var(key)
            && !raw.trim().is_empty()
        {
            let path = PathBuf::from(raw.trim());
            ensure_writable_dir(&path).map_err(|error| format!("{key} invalid: {error}"))?;
            return Ok(path);
        }
    }

    // 2. Dev escape hatch only: run with the repo as harness root.
    // Requires an explicit flag so dev never accidentally uses prod data.
    if tauri::is_dev()
        && std::env::var("STARK_DEV_REPO_ROOT").ok().as_deref() == Some("1")
    {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        for candidate in [cwd.join("..").join(".."), cwd.clone()] {
            if candidate.join("backend-rs").join("Cargo.toml").exists() {
                ensure_writable_dir(&candidate)?;
                return Ok(candidate);
            }
        }
        return Err(
            "STARK_DEV_REPO_ROOT=1 but no repo root with backend-rs/Cargo.toml was found."
                .to_string(),
        );
    }

    // 3. Persisted pointer, else the D:\StarkData default.
    if let Some(pointer) = pointer_path(app)
        && let Some(pinned) = read_pointer(&pointer)
    {
        ensure_writable_dir(&pinned)
            .map_err(|error| format!("saved harness root invalid ({}): {error}. Pick another folder in Settings.", pinned.display()))?;
        return Ok(pinned);
    }
    let default = default_harness_root();
    ensure_writable_dir(&default).map_err(|error| {
        format!(
            "default harness root {} unavailable: {error}. Set STARK_HARNESS_ROOT or pick a folder in Settings.",
            default.display()
        )
    })?;
    if let Some(pointer) = pointer_path(app) {
        // Best-effort: a missing pointer must not break startup.
        let _ = write_pointer(&pointer, &default);
    }
    Ok(default)
}

fn packaged_backend(app: &AppHandle) -> Option<PathBuf> {
    let resource_dir = app.path().resource_dir().ok()?;
    let mut bases = vec![resource_dir.clone()];
    if let Some(parent) = resource_dir.parent() {
        bases.push(parent.to_path_buf());
    }
    for base in bases {
        for relative in [
            "binaries/stark-server.exe",
            "stark-server.exe",
            "binaries/stark-server-x86_64-pc-windows-gnu.exe",
            "binaries/stark-server-x86_64-pc-windows-msvc.exe",
        ] {
            let path = base.join(relative);
            if path.exists() {
                return Some(path);
            }
        }
    }
    None
}

#[cfg(windows)]
fn hidden_command(program: &Path) -> Command {
    use std::os::windows::process::CommandExt;
    let mut command = Command::new(program);
    command.creation_flags(0x0800_0000);
    command
}

#[cfg(not(windows))]
fn hidden_command(program: &Path) -> Command {
    Command::new(program)
}

fn show_fatal(app: &AppHandle, title: &str, message: &str) {
    log::error!("{title}: {message}");
    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
    let _ = app
        .dialog()
        .message(message)
        .title(title)
        .kind(MessageDialogKind::Error)
        .blocking_show();
}

/// Current harness locations for Settings. Never includes secrets.
pub fn harness_info(app: &AppHandle) -> serde_json::Value {
    let exe = std::env::current_exe()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (root, error) = match root_dir(app) {
        Ok(root) => (root.to_string_lossy().into_owned(), None),
        Err(error) => (String::new(), Some(error)),
    };
    serde_json::json!({
        "harness_root": root,
        "error": error,
        "install_path": exe,
        "default_root": default_harness_root().to_string_lossy(),
        "dev": tauri::is_dev(),
    })
}

/// Persist a new harness root chosen in Settings. Validates writability
/// before saving; takes effect on next launch (backend restarts with it).
pub fn set_harness_root(app: &AppHandle, raw: &str) -> Result<String, String> {
    let path = PathBuf::from(raw.trim());
    if path.as_os_str().is_empty() {
        return Err("Pick a folder first.".to_string());
    }
    ensure_writable_dir(&path)?;
    let Some(pointer) = pointer_path(app) else {
        return Err("Settings storage is unavailable.".to_string());
    };
    write_pointer(&pointer, &path)?;
    Ok(path.to_string_lossy().into_owned())
}

pub fn ensure_running(app: &AppHandle) {
    if std::env::var("STARK_SKIP_BACKEND").ok().as_deref() == Some("1") {
        return;
    }
    if crate::api_healthy() {
        log::info!("using existing Stark backend on port {}", crate::api_port());
        return;
    }

    let root = match root_dir(app) {
        Ok(root) => root,
        Err(error) => {
            log::error!("harness root unavailable: {error}");
            show_fatal(app, "Harness data unavailable", &error);
            return;
        }
    };
    let configured = std::env::var("STARK_BACKEND_BIN")
        .ok()
        .map(PathBuf::from)
        .filter(|path| path.exists());

    fn rust_dev_binary(root: &Path) -> Option<PathBuf> {        let exe = if cfg!(windows) {
            "stark-server.exe"
        } else {
            "stark-server"
        };
        for candidate in [
            root.join("backend-rs").join("target").join("debug").join(exe),
            root.join("target").join("debug").join(exe),
        ] {
            if candidate.exists() {
                return Some(candidate);
            }
        }
        std::env::var("CARGO_TARGET_DIR")
            .ok()
            .map(PathBuf::from)
            .map(|dir| dir.join("debug").join(exe))
            .filter(|path| path.exists())
    }

    let (program, cwd) = if let Some(binary) = configured {
        (binary, root.clone())
    } else if let Some(binary) = packaged_backend(app) {
        (binary, root.clone())
    } else if let Some(binary) = rust_dev_binary(&root) {
        (binary, root.clone())
    } else {
        log::error!("no Stark backend binary found (set STARK_BACKEND_BIN)");
        return;
    };

    match hidden_command(&program)
        .current_dir(cwd)
        .env("STARK_PORT", crate::api_port().to_string())
        .env("STARK_HARNESS_ROOT", &root)
        .env("HARNESS_ROOT", &root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => {
            log::info!(
                "started Stark backend: {} (pid {})",
                program.display(),
                child.id()
            );
            if let Ok(mut guard) = app.state::<ShellState>().backend.lock() {
                *guard = Some(child);
            }
        }
        Err(error) => log::error!(
            "failed to start Stark backend {}: {error}",
            program.display()
        ),
    }
}

pub fn stop(app: &AppHandle) {
    let state = app.state::<ShellState>();
    let child = state.backend.lock().ok().and_then(|mut guard| guard.take());
    let Some(mut child) = child else {
        return;
    };

    let pid = child.id();
    #[cfg(windows)]
    {
        let pid_text = pid.to_string();
        let _ = hidden_command(Path::new("taskkill"))
            .args(["/PID", pid_text.as_str(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
    log::info!("stopped Stark backend pid {pid}");
}
