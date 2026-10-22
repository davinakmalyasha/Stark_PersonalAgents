use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use portable_pty::{Child as PtyChild, CommandBuilder, MasterPty, PtySize, native_pty_system};
use regex::Regex;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child as TokioChild, ChildStdin, Command};
use tokio::sync::Mutex as AsyncMutex;

use crate::head_tail::HeadTailBuffer;

const POLL_INTERVAL: Duration = Duration::from_millis(50);
const MAX_PROCESSES: usize = 64;

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

#[derive(Debug, Clone)]
pub struct SpawnedProcess {
    pub id: i32,
    pub name: String,
    pub pid: u32,
}

struct PipesIo {
    child: Mutex<Option<TokioChild>>,
    stdin: AsyncMutex<Option<ChildStdin>>,
}

struct PtyIo {
    child: Mutex<Option<Box<dyn PtyChild + Send>>>,
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    _master: Mutex<Option<Box<dyn MasterPty + Send>>>,
}

enum ProcessIo {
    Pipes(Box<PipesIo>),
    Pty(Box<PtyIo>),
}

struct ManagedProcess {
    id: i32,
    name: String,
    command: String,
    owner: String,
    pid: u32,
    io: Box<ProcessIo>,
    output: Arc<Mutex<HeadTailBuffer>>,
    status: Arc<Mutex<ProcessStatus>>,
    started: Instant,
}

impl ManagedProcess {
    fn try_wait(&self) -> Result<Option<Option<i32>>, String> {
        match self.io.as_ref() {
            ProcessIo::Pipes(process) => {
                let mut child = process.child.lock().unwrap();
                match child.as_mut() {
                    Some(child) => child
                        .try_wait()
                        .map(|status| status.map(|status| status.code()))
                        .map_err(|error| format!("failed to poll process: {error}")),
                    None => Ok(Some(None)),
                }
            }
            ProcessIo::Pty(process) => {
                let mut child = process.child.lock().unwrap();
                match child.as_mut() {
                    Some(child) => child
                        .try_wait()
                        .map(|status| status.map(|status| Some(status.exit_code() as i32)))
                        .map_err(|error| format!("failed to poll process: {error}")),
                    None => Ok(Some(None)),
                }
            }
        }
    }

    fn refresh_status(&self) {
        if matches!(*self.status.lock().unwrap(), ProcessStatus::Killed) {
            return;
        }
        if let Ok(Some(code)) = self.try_wait() {
            *self.status.lock().unwrap() = ProcessStatus::Exited(code);
        }
    }

    fn output_text(&self) -> String {
        let output = self.output.lock().unwrap();
        String::from_utf8_lossy(&output.to_bytes_with_omission_marker()).into_owned()
    }

    fn write_pty(&self, input: &str) -> Result<(), String> {
        let ProcessIo::Pty(process) = self.io.as_ref() else {
            return Err("process is not a PTY".to_string());
        };
        let mut writer = process.writer.lock().unwrap();
        let Some(writer) = writer.as_mut() else {
            return Err("process PTY is closed".to_string());
        };
        writer
            .write_all(input.as_bytes())
            .and_then(|_| writer.flush())
            .map_err(|error| format!("failed to write PTY input: {error}"))
    }

    fn kill(&self) -> Result<(), String> {
        match self.io.as_ref() {
            ProcessIo::Pipes(process) => {
                let mut child = process.child.lock().unwrap();
                if let Some(child) = child.as_mut()
                    && !matches!(*self.status.lock().unwrap(), ProcessStatus::Exited(_))
                {
                    child
                        .start_kill()
                        .map_err(|error| format!("failed to kill process: {error}"))?;
                }
            }
            ProcessIo::Pty(process) => {
                let mut child = process.child.lock().unwrap();
                if let Some(child) = child.as_mut()
                    && !matches!(*self.status.lock().unwrap(), ProcessStatus::Exited(_))
                {
                    child
                        .kill()
                        .map_err(|error| format!("failed to kill process: {error}"))?;
                }
            }
        }
        let mut status = self.status.lock().unwrap();
        if !matches!(*status, ProcessStatus::Exited(_)) {
            *status = ProcessStatus::Killed;
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct ProcessManager {
    processes: Arc<DashMap<String, Arc<ManagedProcess>>>,
    by_id: Arc<DashMap<i32, Arc<ManagedProcess>>>,
    next_id: Arc<AtomicI32>,
}

impl Default for ProcessManager {
    fn default() -> Self {
        Self {
            processes: Arc::new(DashMap::new()),
            by_id: Arc::new(DashMap::new()),
            next_id: Arc::new(AtomicI32::new(1)),
        }
    }
}

impl ProcessManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn spawn_background(
        &self,
        command: &str,
        workdir: &Path,
        owner: &str,
        tty: bool,
    ) -> Result<SpawnedProcess, String> {
        if self.processes.contains_key(command) {
            return Err(format!(
                "background process already exists for this command: {command}"
            ));
        }
        self.spawn_inner(command, workdir, owner, Some(command.to_string()), tty)
    }

    pub fn spawn_session(
        &self,
        command: &str,
        workdir: &Path,
        owner: &str,
        tty: bool,
    ) -> Result<SpawnedProcess, String> {
        self.spawn_inner(command, workdir, owner, None, tty)
    }

    fn allocate_id(&self) -> i32 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    fn spawn_inner(
        &self,
        command: &str,
        workdir: &Path,
        owner: &str,
        name: Option<String>,
        tty: bool,
    ) -> Result<SpawnedProcess, String> {
        if self.by_id.len() >= MAX_PROCESSES {
            return Err(format!(
                "unified exec process limit reached ({MAX_PROCESSES})"
            ));
        }

        let id = self.allocate_id();
        let name = name.unwrap_or_else(|| format!("exec-{id}"));
        let output = Arc::new(Mutex::new(HeadTailBuffer::default()));
        let (io, pid) = if tty {
            self.spawn_pty(command, workdir, output.clone())?
        } else {
            self.spawn_pipes(command, workdir, output.clone())?
        };
        let io = Box::new(io);
        let managed = Arc::new(ManagedProcess {
            id,
            name: name.clone(),
            command: command.to_string(),
            owner: owner.to_string(),
            pid,
            io,
            output,
            status: Arc::new(Mutex::new(ProcessStatus::Running)),
            started: Instant::now(),
        });
        self.processes.insert(name.clone(), managed.clone());
        self.by_id.insert(id, managed.clone());
        monitor(managed);

        Ok(SpawnedProcess { id, name, pid })
    }

    fn spawn_pipes(
        &self,
        command: &str,
        workdir: &Path,
        output: Arc<Mutex<HeadTailBuffer>>,
    ) -> Result<(ProcessIo, u32), String> {
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
            .map_err(|error| format!("failed to start process: {error}"))?;
        let pid = child.id().unwrap_or(0);
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stdin = child.stdin.take();

        if let Some(stream) = stdout {
            pump_async(stream, output.clone());
        }
        if let Some(stream) = stderr {
            pump_async(stream, output);
        }

        Ok((
            ProcessIo::Pipes(Box::new(PipesIo {
                child: Mutex::new(Some(child)),
                stdin: AsyncMutex::new(stdin),
            })),
            pid,
        ))
    }

    fn spawn_pty(
        &self,
        command: &str,
        workdir: &Path,
        output: Arc<Mutex<HeadTailBuffer>>,
    ) -> Result<(ProcessIo, u32), String> {
        let pair = native_pty_system()
            .openpty(PtySize::default())
            .map_err(|error| format!("failed to allocate PTY: {error}"))?;
        let mut builder = CommandBuilder::new(find_bash());
        builder.args(["--noprofile", "--norc", "-lc", command]);
        builder.cwd(workdir);
        builder.env("TERM", "dumb");

        let child = pair
            .slave
            .spawn_command(builder)
            .map_err(|error| format!("failed to start PTY process: {error}"))?;
        let pid = child.process_id().unwrap_or(0);
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| format!("failed to read PTY output: {error}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| format!("failed to write PTY input: {error}"))?;
        let master = pair.master;
        drop(pair.slave);
        pump_blocking(reader, output);

        Ok((
            ProcessIo::Pty(Box::new(PtyIo {
                child: Mutex::new(Some(child)),
                writer: Mutex::new(Some(writer)),
                _master: Mutex::new(Some(master)),
            })),
            pid,
        ))
    }

    fn get(&self, target: &str) -> Result<Arc<ManagedProcess>, String> {
        if let Ok(id) = target.parse::<i32>()
            && let Some(process) = self.by_id.get(&id)
        {
            return Ok(process.value().clone());
        }
        self.processes
            .get(target)
            .map(|process| process.value().clone())
            .ok_or_else(|| format!("unknown process: {target}"))
    }

    pub fn exists(&self, target: &str) -> bool {
        self.get(target).is_ok()
    }

    pub fn list(&self) -> String {
        if self.by_id.is_empty() {
            return "No background processes.".to_string();
        }
        let mut rows: Vec<String> = self
            .by_id
            .iter()
            .map(|entry| {
                let process = entry.value();
                process.refresh_status();
                let status = process.status.lock().unwrap().label();
                format!(
                    "- #{} {} (PID {}, {}, {}s) — {}",
                    process.id,
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

    pub fn logs(&self, target: &str, tail_lines: usize) -> Result<String, String> {
        let process = self.get(target)?;
        process.refresh_status();
        let output = process.output_text();
        if output.is_empty() {
            return Ok("(no output)".to_string());
        }
        let lines: Vec<&str> = output.lines().collect();
        let start = lines.len().saturating_sub(tail_lines.max(1));
        Ok(lines[start..].join("\n"))
    }

    pub fn kill(&self, target: &str) -> Result<(), String> {
        let process = self.get(target)?;
        process.refresh_status();
        process.kill()
    }

    pub fn remove(&self, target: &str) -> bool {
        let Ok(process) = self.get(target) else {
            return false;
        };
        self.by_id.remove(&process.id);
        self.processes.remove(&process.name);
        true
    }

    pub fn kill_all(&self) -> usize {
        let mut count = 0;
        for entry in self.by_id.iter() {
            let process = entry.value();
            if !matches!(*process.status.lock().unwrap(), ProcessStatus::Exited(_))
                && process.kill().is_ok()
            {
                count += 1;
            }
        }
        count
    }

    pub fn cleanup_owner(&self, owner: &str) -> usize {
        let ids: Vec<i32> = self
            .by_id
            .iter()
            .filter(|entry| entry.value().owner == owner)
            .map(|entry| *entry.key())
            .collect();
        let mut count = 0;
        for id in ids {
            if self.kill(&id.to_string()).is_ok() {
                count += 1;
            }
        }
        count
    }

    pub async fn wait_status(
        &self,
        target: &str,
        timeout_ms: u64,
    ) -> Result<(ProcessStatus, String), String> {
        let process = self.get(target)?;
        let started = Instant::now();
        loop {
            process.refresh_status();
            let status = process.status.lock().unwrap().clone();
            if !matches!(status, ProcessStatus::Running) {
                tokio::time::sleep(Duration::from_millis(75)).await;
                return Ok((status, process.output_text()));
            }
            if started.elapsed() >= Duration::from_millis(timeout_ms) {
                return Ok((ProcessStatus::Running, process.output_text()));
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    pub async fn wait(&self, target: &str, timeout_ms: u64) -> Result<String, String> {
        let (status, _) = self.wait_status(target, timeout_ms).await?;
        if matches!(status, ProcessStatus::Running) {
            return Err(format!(
                "Timed out waiting for '{target}' after {timeout_ms}ms"
            ));
        }
        Ok(format!("Process '{target}' {}", status.label()))
    }

    pub async fn wait_ready(
        &self,
        target: &str,
        pattern: &str,
        timeout_ms: u64,
    ) -> Result<String, String> {
        let regex =
            Regex::new(pattern).map_err(|error| format!("invalid expect regex: {error}"))?;
        let process = self.get(target)?;
        let started = Instant::now();
        loop {
            if regex.is_match(&process.output_text()) {
                return Ok(format!("[READY] '{target}' matched /{pattern}/"));
            }
            process.refresh_status();
            let status = process.status.lock().unwrap().clone();
            if !matches!(status, ProcessStatus::Running) {
                return Err(format!(
                    "Process '{target}' {} before matching /{pattern}/",
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

    pub async fn send(
        &self,
        target: &str,
        input: &str,
        yield_ms: u64,
    ) -> Result<(ProcessStatus, String), String> {
        let process = self.get(target)?;
        if !input.is_empty() {
            match process.io.as_ref() {
                ProcessIo::Pipes(process) => {
                    let mut stdin = process.stdin.lock().await;
                    let Some(stdin) = stdin.as_mut() else {
                        return Err(format!("process '{target}' has no stdin"));
                    };
                    stdin
                        .write_all(input.as_bytes())
                        .await
                        .map_err(|error| format!("failed to write stdin: {error}"))?;
                    stdin
                        .flush()
                        .await
                        .map_err(|error| format!("failed to flush stdin: {error}"))?;
                }
                ProcessIo::Pty(_) => {
                    let process = process.clone();
                    let input = input.to_string();
                    tokio::task::spawn_blocking(move || process.write_pty(&input))
                        .await
                        .map_err(|error| format!("failed to join PTY write: {error}"))??;
                }
            }
        }

        if yield_ms > 0 {
            tokio::time::sleep(Duration::from_millis(yield_ms)).await;
        }
        process.refresh_status();
        let status = process.status.lock().unwrap().clone();
        Ok((status, process.output_text()))
    }
}

fn pump_async<R>(mut reader: R, output: Arc<Mutex<HeadTailBuffer>>)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buffer = [0u8; 4096];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(count) => output.lock().unwrap().push_chunk(&buffer[..count]),
            }
        }
    });
}

fn pump_blocking<R>(mut reader: R, output: Arc<Mutex<HeadTailBuffer>>)
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => output.lock().unwrap().push_chunk(&buffer[..count]),
            }
        }
    });
}

fn monitor(process: Arc<ManagedProcess>) {
    tokio::spawn(async move {
        loop {
            process.refresh_status();
            if !matches!(*process.status.lock().unwrap(), ProcessStatus::Running) {
                break;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    });
}

fn find_bash() -> PathBuf {
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
