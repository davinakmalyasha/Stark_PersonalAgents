use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tauri::{AppHandle, Manager};

use crate::ShellState;

fn root_dir() -> PathBuf {
    if let Ok(root) = std::env::var("STARK_ROOT") {
        let path = PathBuf::from(root);
        if path.join("server").join("api_server.py").exists() {
            return path;
        }
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    for candidate in [cwd.join("..").join(".."), cwd.clone()] {
        if candidate.join("server").join("api_server.py").exists() {
            return candidate;
        }
    }

    PathBuf::from(r"D:\MyProject\LangChain")
}

fn python_command(root: &Path) -> Option<(PathBuf, Vec<String>)> {
    for relative in [
        "venv/Scripts/python.exe",
        ".venv/Scripts/python.exe",
        "venv312/Scripts/python.exe",
    ] {
        let path = root.join(relative);
        if path.exists() {
            return Some((
                path,
                vec!["-B".to_string(), "server/api_server.py".to_string()],
            ));
        }
    }
    let python = if cfg!(windows) {
        "python.exe"
    } else {
        "python3"
    };
    Some((
        PathBuf::from(python),
        vec!["-B".to_string(), "server/api_server.py".to_string()],
    ))
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

    let root = root_dir();
    let configured = std::env::var("STARK_BACKEND_BIN")
        .ok()
        .map(PathBuf::from)
        .filter(|path| path.exists());

    let (program, args, cwd) = if let Some(binary) = configured {
        (binary, Vec::new(), root)
    } else if let Some((python, args)) = python_command(&root) {
        (python, args, root)
    } else {
        log::error!("no Stark backend binary or Python venv found");
        return;
    };

    match hidden_command(&program)
        .args(args)
        .current_dir(cwd)
        .env("STARK_PORT", crate::api_port().to_string())
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
