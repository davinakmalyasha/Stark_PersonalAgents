use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tauri::{AppHandle, Manager};

use crate::ShellState;

fn root_dir(app: &AppHandle) -> PathBuf {
    for key in ["STARK_HARNESS_ROOT", "STARK_ROOT"] {
        if let Ok(root) = std::env::var(key) {
            let path = PathBuf::from(root);
            if path.exists() {
                return path;
            }
        }
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    // The repo root is the directory containing backend-rs/ (dev) or the
    // current directory. No Python-tree probing: the backend is Rust-only.
    for candidate in [cwd.join("..").join(".."), cwd.clone()] {
        if candidate.join("backend-rs").join("Cargo.toml").exists() {
            return candidate;
        }
    }

    let fallback = app.path().app_data_dir().unwrap_or_else(|_| cwd.clone());
    let _ = std::fs::create_dir_all(&fallback);
    fallback
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

pub fn ensure_running(app: &AppHandle) {
    if std::env::var("STARK_SKIP_BACKEND").ok().as_deref() == Some("1") {
        return;
    }
    if crate::api_healthy() {
        log::info!("using existing Stark backend on port {}", crate::api_port());
        return;
    }

    let root = root_dir(app);
    let configured = std::env::var("STARK_BACKEND_BIN")
        .ok()
        .map(PathBuf::from)
        .filter(|path| path.exists());

    fn rust_dev_binary(root: &Path) -> Option<PathBuf> {
        let exe = if cfg!(windows) {
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
