use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use regex::Regex;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Mutex as AsyncMutex;

const MAX_OUTPUT: usize = 1_000_000;
const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone)]
pub enum ProcessStatus {
    Running,
    Exited(Option<i32>),
    Killed,
}

impl ProcessStatus {
    fn label(&self) -> String {
        match self {
            Self::Running => "running".to_string(),
            Self::Exited(Some(code)) => format!("exited({code})"),
            Self::Exited(None) => "exited(unknown)".to_string(),
            Self::Killed => "killed".to_string(),
        }
    }
}

struct ManagedProcess {
    name: String,
    command: String,
    owner: String,
    pid: u32,
    child: Mutex<Option<Child>>,
    stdin: AsyncMutex<Option<ChildStdin>>,
    output: Arc<Mutex<String>>,
    status: Arc<Mutex<ProcessStatus>>,
    started: Instant,
}

#[derive(Clone, Default)]
pub struct ProcessManager {
    processes: Arc<DashMap<String, Arc<ManagedProcess>>>,
}

impl ProcessManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn spawn(
        &self,
        command: &str,
        workdir: &Path,
        owner: &str,
    ) -> Result<(String, u32), String> {
        if self.processes.contains_key(command) {
            return Err(format!(
                "background process already exists for this command: {command}"
            ));
        }

        let mut process = Command::new(find_bash());
        process
            .arg("-lc")
            .arg(command)
            .current_dir(workdir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = process
            .spawn()
            .map_err(|error| format!("failed to start background process: {error}"))?;
        let pid = child.id().unwrap_or(0);
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stdin = child.stdin.take();

        let output = Arc::new(Mutex::new(String::new()));
        let status = Arc::new(Mutex::new(ProcessStatus::Running));
        let managed = Arc::new(ManagedProcess {
            name: command.to_string(),
            command: command.to_string(),
            owner: owner.to_string(),
            pid,
            child: Mutex::new(Some(child)),
            stdin: AsyncMutex::new(stdin),
            output: output.clone(),
            status: status.clone(),
            started: Instant::now(),
        });
        self.processes.insert(command.to_string(), managed.clone());

        if let Some(stream) = stdout {
            pump(stream, output.clone());
        }
        if let Some(stream) = stderr {
            pump(stream, output);
        }
        monitor(managed.clone());

        Ok((command.to_string(), pid))
    }

    pub fn exists(&self, name: &str) -> bool {
        self.processes.contains_key(name)
    }

    pub fn list(&self) -> String {
        if self.processes.is_empty() {
            return "No background processes.".to_string();
        }
        let mut rows: Vec<String> = self
            .processes
            .iter()
            .map(|entry| {
                let process = entry.value();
                let status = process.status.lock().unwrap().label();
                format!(
                    "- {} (PID {}, {}, {}s) — {}",
                    process.name,
                    process.pid,
                    status,
                    process.started.elapsed().as_secs(),
                    process.command
                )
            })
            .collect();
        rows.sort();
        rows.join("\n")
    }

    pub fn logs(&self, name: &str, tail_lines: usize) -> Result<String, String> {
        let process = self
            .processes
            .get(name)
            .ok_or_else(|| format!("unknown background process: {name}"))?;
        let output = process.output.lock().unwrap();
        if output.is_empty() {
            return Ok("(no output)".to_string());
        }
        let lines: Vec<&str> = output.lines().collect();
        let start = lines.len().saturating_sub(tail_lines.max(1));
        Ok(lines[start..].join("\n"))
    }

    pub fn kill(&self, name: &str) -> Result<(), String> {
        let process = self
            .processes
            .get(name)
            .ok_or_else(|| format!("unknown background process: {name}"))?;
        if let Some(child) = process.child.lock().unwrap().as_mut() {
            child
                .start_kill()
                .map_err(|error| format!("failed to kill {name}: {error}"))?;
        }
        *process.status.lock().unwrap() = ProcessStatus::Killed;
        Ok(())
    }

    pub fn kill_all(&self) -> usize {
        let mut count = 0;
        for entry in self.processes.iter() {
            if let Some(child) = entry.value().child.lock().unwrap().as_mut() {
                let _ = child.start_kill();
                *entry.value().status.lock().unwrap() = ProcessStatus::Killed;
                count += 1;
            }
        }
        count
    }

    pub fn cleanup_owner(&self, owner: &str) -> usize {
        let names: Vec<String> = self
            .processes
            .iter()
            .filter(|entry| entry.value().owner == owner)
            .map(|entry| entry.key().clone())
            .collect();
        let mut count = 0;
        for name in names {
            if self.kill(&name).is_ok() {
                count += 1;
            }
        }
        count
    }

    pub async fn wait(&self, name: &str, timeout_ms: u64) -> Result<String, String> {
        let process = self
            .processes
            .get(name)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| format!("unknown background process: {name}"))?;
        let started = Instant::now();
        loop {
            let status = process.status.lock().unwrap().clone();
            if !matches!(status, ProcessStatus::Running) {
                return Ok(format!("Process '{name}' {}", status.label()));
            }
            if started.elapsed() >= Duration::from_millis(timeout_ms) {
                return Err(format!(
                    "Timed out waiting for '{name}' after {timeout_ms}ms"
                ));
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    pub async fn wait_ready(
        &self,
        name: &str,
        pattern: &str,
        timeout_ms: u64,
    ) -> Result<String, String> {
        let regex =
            Regex::new(pattern).map_err(|error| format!("invalid expect regex: {error}"))?;
        let process = self
            .processes
            .get(name)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| format!("unknown background process: {name}"))?;
        let started = Instant::now();
        loop {
            let matched = regex.is_match(&process.output.lock().unwrap());
            if matched {
                return Ok(format!("[READY] '{name}' matched /{pattern}/"));
            }
            let status = process.status.lock().unwrap().clone();
            if !matches!(status, ProcessStatus::Running) {
                return Err(format!(
                    "Process '{name}' {} before matching /{pattern}/",
                    status.label()
                ));
            }
            if started.elapsed() >= Duration::from_millis(timeout_ms) {
                return Err(format!(
                    "Timed out waiting for /{pattern}/ after {timeout_ms}ms"
                ));
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    pub async fn send(&self, name: &str, input: &str) -> Result<(), String> {
        let process = self
            .processes
            .get(name)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| format!("unknown background process: {name}"))?;
        let mut stdin = process.stdin.lock().await;
        let Some(stdin) = stdin.as_mut() else {
            return Err(format!("background process '{name}' has no stdin"));
        };
        stdin
            .write_all(input.as_bytes())
            .await
            .map_err(|error| format!("failed to write stdin: {error}"))?;
        if !input.ends_with('\n') {
            stdin
                .write_all(b"\n")
                .await
                .map_err(|error| format!("failed to write stdin: {error}"))?;
        }
        stdin
            .flush()
            .await
            .map_err(|error| format!("failed to flush stdin: {error}"))
    }
}

fn pump<R>(mut reader: R, output: Arc<Mutex<String>>)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buffer = [0u8; 4096];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    let mut output = output.lock().unwrap();
                    output.push_str(&String::from_utf8_lossy(&buffer[..count]));
                    if output.len() > MAX_OUTPUT {
                        output.clear();
                        output.push_str("[output truncated]\n");
                    }
                }
            }
        }
    });
}

fn monitor(process: Arc<ManagedProcess>) {
    tokio::spawn(async move {
        loop {
            let exit = {
                let mut child = process.child.lock().unwrap();
                match child.as_mut() {
                    Some(child) => match child.try_wait() {
                        Ok(Some(status)) => Some(status.code()),
                        Ok(None) => None,
                        Err(_) => Some(None),
                    },
                    None => Some(None),
                }
            };
            if let Some(code) = exit {
                let mut status = process.status.lock().unwrap();
                if !matches!(*status, ProcessStatus::Killed) {
                    *status = ProcessStatus::Exited(code);
                }
                break;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    });
}

fn find_bash() -> std::path::PathBuf {
    if let Ok(path) = std::env::var("STARK_BASH")
        && Path::new(&path).is_file()
    {
        return path.into();
    }
    for path in [
        r"C:\Program Files\Git\bin\bash.exe",
        r"C:\Program Files\Git\usr\bin\bash.exe",
        r"C:\Program Files (x86)\Git\bin\bash.exe",
    ] {
        if Path::new(path).is_file() {
            return path.into();
        }
    }
    "bash".into()
}
