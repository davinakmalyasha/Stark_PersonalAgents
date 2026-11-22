//! Stable Stark Rune launcher/supervisor.
//!
//! Layout:
//! ```text
//! StarkRune.exe
//! current.json           { version, previous }
//! app/<version>/         stark-rune.exe, stark-server.exe, onnxruntime.dll
//! models/                shared voice models
//! updates/               staged downloads
//! ```
//! The launcher starts the server and UI from the current version, owns the
//! server with a kill-on-close job object, applies `pending.json` only after
//! the UI exits, and rolls back when the new UI fails immediately.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

const DEFAULT_PORT: u16 = 8124;
const STARTUP_GRACE: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Current {
    version: String,
    #[serde(default)]
    previous: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Pending {
    version: String,
    #[serde(default)]
    reason: String,
}

fn main() {
    if std::env::args().any(|arg| arg == "--version") {
        println!("stark-launcher {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let root = install_root();
    let _ = std::fs::create_dir_all(root.join("runtime"));
    #[cfg(windows)]
    let _guard = match single_instance() {
        Some(guard) => guard,
        None => {
            log(&root, "another launcher is already running");
            fatal_dialog("Stark Rune is already running.");
            return;
        }
    };
    if let Err(error) = supervise(&root) {
        log(&root, &format!("fatal: {error}"));
        fatal_dialog(&format!(
            "Stark Rune could not start.

{error}"
        ));
        std::process::exit(1);
    }
}

fn install_root() -> PathBuf {
    if let Ok(raw) = std::env::var("STARK_INSTALL_ROOT") {
        let path = PathBuf::from(raw.trim());
        if !path.as_os_str().is_empty() {
            return path;
        }
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn supervise(root: &Path) -> Result<(), String> {
    // A pending update left by a previous run is applied before any UI starts.
    // The pending marker is consumed here so a normal exit cannot later write
    // the running version as its own rollback target.
    let mut startup_update = false;
    if root.join("pending.json").is_file() {
        match apply_pending(root, None) {
            Ok(version) => {
                log(root, &format!("applied pending update {version}"));
                startup_update = true;
            }
            Err(error) => log(root, &format!("pending update rejected: {error}")),
        }
        let _ = std::fs::remove_file(root.join("pending.json"));
    }
    loop {
        let current = read_current(root)?;
        cleanup_versions(root, &current);
        let version_dir = root.join("app").join(&current.version);
        if let Err(error) = validate_version(&version_dir, &current.version) {
            if let Some(previous) = &current.previous {
                let previous_dir = root.join("app").join(previous);
                if validate_version(&previous_dir, previous).is_ok() {
                    log(root, &format!("{error}; recovering {previous}"));
                    write_current(root, previous, None)?;
                    continue;
                }
            }
            return Err(error);
        }
        log(root, &format!("starting {}", current.version));

        let mut server = start_server(root, &version_dir, &current.version)?;
        let mut ui = start_ui(root, &version_dir)?;
        let started = Instant::now();
        let status = ui.wait().map_err(|error| format!("wait for UI: {error}"))?;
        stop_server(&mut server);
        let elapsed = started.elapsed();

        if !root.join("pending.json").is_file() {
            log(
                root,
                &format!("UI exited ({status:?}) after {:.1}s", elapsed.as_secs_f32()),
            );
            if startup_update && !status.success() && elapsed < STARTUP_GRACE {
                if let Some(previous) = current.previous.clone() {
                    log(
                        root,
                        &format!(
                            "new build {} failed ({status:?}); rolling back to {previous}",
                            current.version
                        ),
                    );
                    write_current(root, &previous, None)?;
                    write_blocked(root, &current.version, &format!("UI exited with {status}"));
                    startup_update = false;
                    continue;
                }
            }
            return if status.success() {
                Ok(())
            } else {
                Err(format!("UI exited with {status}"))
            };
        }

        if !status.success() && elapsed < STARTUP_GRACE {
            // The new build failed before it could settle: put the previous
            // version back and launch that instead.
            if let Some(previous) = current.previous.clone() {
                log(
                    root,
                    &format!(
                        "new build {} failed ({status:?}); rolling back to {previous}",
                        current.version
                    ),
                );
                write_current(root, &previous, None)?;
                let _ = std::fs::remove_file(root.join("pending.json"));
                write_blocked(root, &current.version, &format!("UI exited with {status}"));
                continue;
            }
        }

        match apply_pending(root, Some(&current)) {
            Ok(version) => {
                log(root, &format!("updated {version}"));
                let _ = std::fs::remove_file(root.join("pending.json"));
                startup_update = true;
                continue;
            }
            Err(error) => {
                log(root, &format!("update failed: {error}"));
                let _ = std::fs::remove_file(root.join("pending.json"));
                write_blocked(root, &current.version, &error);
                continue;
            }
        }
    }
}

/// Keep the running version and the one rollback target. Everything else is a
/// superseded payload and safe to remove now that no UI is running.
fn cleanup_versions(root: &Path, current: &Current) {
    let Ok(entries) = std::fs::read_dir(root.join("app")) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == current.version || current.previous.as_deref() == Some(name.as_str()) {
            continue;
        }
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

fn read_current(root: &Path) -> Result<Current, String> {
    let text = std::fs::read_to_string(root.join("current.json"))
        .map_err(|error| format!("read current.json: {error}"))?;
    let current: Current =
        serde_json::from_str(&text).map_err(|error| format!("parse current.json: {error}"))?;
    if current.version.trim().is_empty() {
        return Err("current.json has no version".to_string());
    }
    Ok(current)
}

fn write_current(root: &Path, version: &str, previous: Option<&str>) -> Result<(), String> {
    let path = root.join("current.json");
    let temp = root.join("current.json.tmp");
    let value = Current {
        version: version.to_string(),
        previous: previous.map(ToString::to_string),
    };
    std::fs::write(
        &temp,
        serde_json::to_vec_pretty(&value).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("write current.json: {error}"))?;
    std::fs::rename(&temp, &path).map_err(|error| format!("replace current.json: {error}"))
}

fn apply_pending(root: &Path, previous: Option<&Current>) -> Result<String, String> {
    let text = std::fs::read_to_string(root.join("pending.json"))
        .map_err(|error| format!("read pending.json: {error}"))?;
    let pending: Pending =
        serde_json::from_str(&text).map_err(|error| format!("parse pending.json: {error}"))?;
    if pending.version.trim().is_empty() {
        return Err("pending.json has no version".to_string());
    }
    let dir = root.join("app").join(&pending.version);
    validate_version(&dir, &pending.version)?;
    let previous = previous
        .map(|current| current.version.clone())
        .or_else(|| read_current(root).ok().map(|current| current.version));
    write_current(root, &pending.version, previous.as_deref())?;
    Ok(pending.version)
}

fn validate_version(dir: &Path, version: &str) -> Result<(), String> {
    let app = dir.join("stark-rune.exe");
    let server = dir.join("stark-server.exe");
    if !app.is_file() {
        return Err(format!("app/{version}/stark-rune.exe is missing"));
    }
    if !server.is_file() {
        return Err(format!("app/{version}/stark-server.exe is missing"));
    }
    if let Ok(stamp) = std::fs::read_to_string(dir.join("VERSION.txt")) {
        if !stamp.contains(version) {
            return Err(format!(
                "app/{version}/VERSION.txt does not name this version"
            ));
        }
    }
    Ok(())
}

fn start_ui(root: &Path, version_dir: &Path) -> Result<Child, String> {
    let exe = version_dir.join("stark-rune.exe");
    let data = data_root(root);
    let mut command = Command::new(&exe);
    command
        .current_dir(version_dir)
        .env("STARK_INSTALL_ROOT", root)
        .env("STARK_PORT", DEFAULT_PORT.to_string())
        .env("STARK_API_PORT", DEFAULT_PORT.to_string())
        .env("STARK_HARNESS_ROOT", &data)
        .env("STARK_LAUNCHER_PID", std::process::id().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let models = root.join("models");
    if models.is_dir() {
        command.env("STARK_MODELS_DIR", &models);
    }
    hidden(&mut command);
    command
        .spawn()
        .map_err(|error| format!("start {}: {error}", exe.display()))
}

struct Server {
    child: Child,
    #[cfg(windows)]
    job: Option<windows_sys::Win32::Foundation::HANDLE>,
}

fn start_server(root: &Path, version_dir: &Path, version: &str) -> Result<Option<Server>, String> {
    let exe = version_dir.join("stark-server.exe");
    if !exe.is_file() {
        log(
            root,
            "no stark-server.exe in this version; UI will run alone",
        );
        return Ok(None);
    }
    if let Some(health) = api_health(DEFAULT_PORT) {
        if health_matches(&health, version) {
            log(
                root,
                &format!("port {DEFAULT_PORT} already runs {version}; reusing it"),
            );
            return Ok(None);
        }
        let running = health
            .get("app_version")
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .unwrap_or("an unknown build");
        return Err(format!(
            "port {DEFAULT_PORT} is held by {running}, not {version}. Close the development backend and start Stark Rune again."
        ));
    }
    let data = data_root(root);
    std::fs::create_dir_all(&data)
        .map_err(|error| format!("create data root {}: {error}", data.display()))?;
    let mut command = Command::new(&exe);
    command
        .current_dir(version_dir)
        .env("STARK_PORT", DEFAULT_PORT.to_string())
        .env("STARK_APP_VERSION", version)
        .env("STARK_HARNESS_ROOT", &data)
        .env("HARNESS_ROOT", &data)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let models = root.join("models");
    if models.is_dir() {
        command.env("STARK_MODELS_DIR", &models);
    }
    hidden(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| format!("start {}: {error}", exe.display()))?;
    #[cfg(windows)]
    let job = job_for(&child);
    wait_healthy(root, DEFAULT_PORT, Duration::from_secs(15));
    if let Ok(Some(status)) = child.try_wait() {
        return Err(format!(
            "the bundled backend exited during startup ({status})"
        ));
    }
    Ok(Some(Server {
        child,
        #[cfg(windows)]
        job,
    }))
}

fn stop_server(server: &mut Option<Server>) {
    if let Some(mut server) = server.take() {
        let _ = server.child.kill();
        let _ = server.child.wait();
        #[cfg(windows)]
        if let Some(job) = server.job.take() {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(job);
            }
        }
    }
}

fn data_root(root: &Path) -> PathBuf {
    if let Ok(raw) = std::env::var("STARK_HARNESS_ROOT") {
        let path = PathBuf::from(raw.trim());
        if !path.as_os_str().is_empty() {
            return path;
        }
    }
    #[cfg(windows)]
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        return PathBuf::from(local).join("StarkRune").join("data");
    }
    root.join("data")
}

fn api_health(port: u16) -> Option<serde_json::Value> {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    let address = format!("127.0.0.1:{port}");
    let mut stream =
        TcpStream::connect_timeout(&address.parse().ok()?, Duration::from_millis(500)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(700)));
    stream
        .write_all(b"GET /api/health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .ok()?;
    let mut buffer = Vec::new();
    stream.read_to_end(&mut buffer).ok()?;
    let text = String::from_utf8_lossy(&buffer);
    let (head, body) = text.split_once("\r\n\r\n")?;
    if !head.starts_with("HTTP/1.1 200") && !head.starts_with("HTTP/1.0 200") {
        return None;
    }
    serde_json::from_str(body.trim()).ok()
}

/// A healthy port belongs to this install only when it reports the same app
/// version. This stops an installed build from silently reusing a development
/// backend with different data and code.
fn health_matches(health: &serde_json::Value, version: &str) -> bool {
    health.get("ok").and_then(|value| value.as_bool()) == Some(true)
        && health.get("app_version").and_then(|value| value.as_str()) == Some(version)
}

fn wait_healthy(root: &Path, port: u16, timeout: Duration) {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if api_health(port).is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    log(root, "backend did not become healthy before the UI started");
}

fn hidden(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    let _ = command;
}

fn write_blocked(root: &Path, version: &str, error: &str) {
    let path = root.join("blocked.json");
    let value = serde_json::json!({ "version": version, "error": error });
    let _ = std::fs::write(path, serde_json::to_vec_pretty(&value).unwrap_or_default());
}

fn fatal_dialog(message: &str) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
        let title: Vec<u16> = "Stark Rune ".encode_utf16().collect();
        let text: Vec<u16> = format!("{message} ").encode_utf16().collect();
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }
    #[cfg(not(windows))]
    eprintln!("{message}");
}

fn log(root: &Path, message: &str) {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("runtime").join("launcher.log"))
    {
        let _ = writeln!(file, "{millis} {message}");
    }
}

#[cfg(windows)]
fn job_for(child: &Child) -> Option<windows_sys::Win32::Foundation::HANDLE> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return None;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) == 0
        {
            return Some(job);
        }
        let _ = AssignProcessToJobObject(job, child.as_raw_handle() as _);
        Some(job)
    }
}

#[cfg(windows)]
struct InstanceGuard(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for InstanceGuard {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        unsafe {
            if !self.0.is_null() {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(windows)]
fn single_instance() -> Option<InstanceGuard> {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name: Vec<u16> = "Local\\StarkRuneLauncher\0".encode_utf16().collect();
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
        if handle.is_null() || GetLastError() == ERROR_ALREADY_EXISTS {
            if !handle.is_null() {
                windows_sys::Win32::Foundation::CloseHandle(handle);
            }
            None
        } else {
            Some(InstanceGuard(handle))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "stark-launcher-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn version(root: &Path, version: &str) {
        let dir = root.join("app").join(version);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("stark-rune.exe"), b"x").unwrap();
        std::fs::write(dir.join("stark-server.exe"), b"x").unwrap();
    }

    #[test]
    fn pending_update_switches_current_and_records_previous() {
        let root = root("apply");
        version(&root, "1.0.0");
        version(&root, "2.0.0");
        write_current(&root, "1.0.0", None).unwrap();
        std::fs::write(
            root.join("pending.json"),
            br#"{"version":"2.0.0","reason":"staged"}"#,
        )
        .unwrap();
        let current = read_current(&root).unwrap();
        assert_eq!(apply_pending(&root, Some(&current)).unwrap(), "2.0.0");
        let current = read_current(&root).unwrap();
        assert_eq!(current.version, "2.0.0");
        assert_eq!(current.previous.as_deref(), Some("1.0.0"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn pending_missing_binaries_is_rejected_without_touching_current() {
        let root = root("reject");
        version(&root, "1.0.0");
        write_current(&root, "1.0.0", None).unwrap();
        std::fs::write(root.join("pending.json"), br#"{"version":"9.9.9"}"#).unwrap();
        assert!(apply_pending(&root, None).is_err());
        assert_eq!(read_current(&root).unwrap().version, "1.0.0");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn health_must_match_the_running_app_version() {
        let health = serde_json::json!({"ok": true, "app_version": "2.7.1-master.10"});
        assert!(health_matches(&health, "2.7.1-master.10"));
        assert!(!health_matches(&health, "2.7.0"));
        assert!(!health_matches(
            &serde_json::json!({"ok": false, "app_version": "2.7.1-master.10"}),
            "2.7.1-master.10"
        ));
        assert!(!health_matches(
            &serde_json::json!({"ok": true}),
            "2.7.1-master.10"
        ));
    }

    #[test]
    fn current_round_trips_through_atomic_temp() {
        let root = root("current");
        write_current(&root, "2.6.4", Some("2.6.3")).unwrap();
        assert!(!root.join("current.json.tmp").exists());
        let current = read_current(&root).unwrap();
        assert_eq!(current.version, "2.6.4");
        assert_eq!(current.previous.as_deref(), Some("2.6.3"));
        let _ = std::fs::remove_dir_all(root);
    }
}
