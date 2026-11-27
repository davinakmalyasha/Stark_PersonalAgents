//! Bun sidecar client: spawns `codemode-worker` (Bun + psychoscript) and speaks
//! line-delimited JSON-RPC over stdio.
//!
//! The worker owns the confined JS program; this client owns tool dispatch and
//! the OpenCode `fetch`/`path`/`Bun` host boundary. Every `tool_call` from the
//! worker runs through [`crate::dispatch`] with the executing run's
//! [`ToolContext`], so plan mode, read-only access, approvals, MCP sessions,
//! cancellation, and checkpoints behave exactly like direct calls. OpenCode's
//! `fetch`, `path`, and `Bun` extension calls cross back too: the worker
//! performs the request/read it was already authorized to perform, while Rust
//! evaluates every permission rule, plan/read-only guard, and file handle.
//!
//! MIT attribution: protocol shape follows OpenCode's Code Mode runtime (MIT).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::Instant;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex as AsyncMutex, oneshot};

use super::extensions::{self, RunHost};
use crate::{ToolContext, ToolOutcome};

/// A tool the worker may call. `name` is the dotted program path
/// (`read`, `github.get_issue`); the Rust dispatch name is derived with
/// [`super::to_dispatch_name`] (`read`, `mcp__github__get_issue`).
#[derive(Debug, Clone)]
pub struct WorkerToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub freeform: bool,
}

#[derive(Debug, Clone)]
pub struct WorkerExecuteResult {
    /// OpenCode `ExecuteOutput.output`, already formatted by the worker.
    pub text: String,
    /// OpenCode `metadata.error` (`!result.ok`).
    pub error: bool,
    pub tool_calls: Vec<Value>,
    pub files: Vec<Value>,
}

type PendingTx = oneshot::Sender<Result<Value, String>>;

pub(crate) struct WorkerInner {
    stdin: AsyncMutex<tokio::process::ChildStdin>,
    pending: Mutex<HashMap<String, PendingTx>>,
    runs: Mutex<HashMap<String, Arc<RunHost>>>,
    next_id: AtomicU64,
    dead: std::sync::atomic::AtomicBool,
    /// Caps concurrent host dispatches (each occupies a blocking-pool thread;
    /// see the spawn_blocking note at the request site).
    dispatch_sem: Arc<tokio::sync::Semaphore>,
}

impl WorkerInner {
    async fn send(&self, id: &str, method: &str, params: &Value) -> Result<Value, String> {
        if self.dead.load(Ordering::Acquire) {
            return Err("code mode worker is not running".to_string());
        }
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().unwrap();
            pending.insert(id.to_string(), tx);
        }
        let line = json!({"id": id, "method": method, "params": params});
        let mut text = serde_json::to_string(&line).map_err(|e| e.to_string())?;
        text.push('\n');
        {
            let mut stdin = self.stdin.lock().await;
            if let Err(error) = stdin.write_all(text.as_bytes()).await {
                self.pending.lock().unwrap().remove(id);
                return Err(format!("code mode worker write failed: {error}"));
            }
        }
        match rx.await {
            Ok(result) => result,
            Err(_) => Err("code mode worker closed the request".to_string()),
        }
    }

    fn next_id(&self, prefix: &str) -> String {
        let n = self.next_id.fetch_add(1, Ordering::Relaxed);
        format!("{prefix}_{n}")
    }

    fn host(&self, run_id: &str) -> Option<Arc<RunHost>> {
        self.runs.lock().unwrap().get(run_id).cloned()
    }

    fn insert_host(&self, run_id: &str, host: Arc<RunHost>) {
        self.runs.lock().unwrap().insert(run_id.to_string(), host);
    }

    fn remove_host(&self, run_id: &str) {
        self.runs.lock().unwrap().remove(run_id);
    }
}

pub struct WorkerManager {
    inner: AsyncMutex<Option<Arc<WorkerInner>>>,
    harness_root: PathBuf,
    /// Serializes lazy startup so concurrent parallel tool calls share one
    /// worker instead of spawning two and kill-dropping the loser.
    starting: AsyncMutex<()>,
    /// Serializes `initialize`; the worker rebuilds its whole runtime on each
    /// initialize, and a concurrent execute must not see it half-reset.
    initializing: AsyncMutex<()>,
    /// Inventory key of the runtime the worker currently holds.
    initialized: Mutex<Option<String>>,
    // Kept alive so the child is reaped; the reader task owns the stdout half.
    _child: AsyncMutex<Option<Child>>,
}

impl Drop for WorkerManager {
    fn drop(&mut self) {
        if let Ok(mut guard) = self._child.try_lock()
            && let Some(child) = guard.as_mut()
        {
            let _ = child.start_kill();
        }
    }
}

impl WorkerManager {
    fn global() -> &'static Mutex<HashMap<String, Arc<WorkerManager>>> {
        static MANAGERS: std::sync::OnceLock<Mutex<HashMap<String, Arc<WorkerManager>>>> =
            std::sync::OnceLock::new();
        MANAGERS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    /// One manager per harness root (production). The manager owns the worker
    /// child, so it must outlive background scripts.
    pub fn for_root(harness_root: &Path) -> Arc<Self> {
        let key = harness_root.to_string_lossy().to_string();
        let mut managers = Self::global().lock().unwrap();
        if let Some(manager) = managers.get(&key) {
            return manager.clone();
        }
        let manager = Arc::new(Self::new(harness_root));
        managers.insert(key, manager.clone());
        manager
    }

    /// Disposable manager for tests and one-shot calls. Dropping it kills the
    /// worker child, so a test process exits promptly.
    pub fn local(harness_root: &Path) -> Arc<Self> {
        Arc::new(Self::new(harness_root))
    }

    fn new(harness_root: &Path) -> Self {
        Self {
            inner: AsyncMutex::new(None),
            harness_root: harness_root.to_path_buf(),
            starting: AsyncMutex::new(()),
            initializing: AsyncMutex::new(()),
            initialized: Mutex::new(None),
            _child: AsyncMutex::new(None),
        }
    }

    async fn ensure_live(&self) -> Result<Arc<WorkerInner>, String> {
        {
            let guard = self.inner.lock().await;
            if let Some(inner) = guard.as_ref()
                && !inner.dead.load(Ordering::Acquire)
            {
                return Ok(inner.clone());
            }
        }
        // Parallel direct calls in Code Mode can race here; only one may spawn.
        let _starting = self.starting.lock().await;
        {
            let guard = self.inner.lock().await;
            if let Some(inner) = guard.as_ref()
                && !inner.dead.load(Ordering::Acquire)
            {
                return Ok(inner.clone());
            }
        }
        self.spawn().await
    }

    async fn spawn(&self) -> Result<Arc<WorkerInner>, String> {
        *self.initialized.lock().unwrap() = None;
        let worker_dir = discover_worker_dir(&self.harness_root);
        let main = worker_dir.join("src").join("main.ts");
        if !main.exists() {
            return Err(format!(
                "code mode worker missing at {}. Set STARK_CODEMODE_WORKER_DIR.",
                main.display()
            ));
        }
        let bun = discover_bun();
        let mut child: Child = Command::new(&bun)
            .arg("run")
            .arg("--silent")
            .arg("./src/main.ts")
            .current_dir(&worker_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("failed to spawn code mode worker ({bun}): {error}"))?;
        let stdin = child.stdin.take().ok_or("worker stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("worker stdout unavailable")?;
        let stderr = child.stderr.take().ok_or("worker stderr unavailable")?;
        // Bun's runtime log goes to tracing, never to the model.
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let line = line.trim().to_string();
                if !line.is_empty() {
                    tracing::debug!(target: "codemode-worker", "{line}");
                }
            }
        });
        let (ready_tx, ready_rx) = oneshot::channel();
        let inner = Arc::new(WorkerInner {
            stdin: AsyncMutex::new(stdin),
            pending: Mutex::new(HashMap::new()),
            runs: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            dead: std::sync::atomic::AtomicBool::new(false),
            dispatch_sem: Arc::new(tokio::sync::Semaphore::new(16)),
        });
        let reader_inner = inner.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            let mut ready_tx = Some(ready_tx);
            loop {
                let line = match lines.next_line().await {
                    Ok(Some(line)) => line,
                    _ => break,
                };
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }
                let msg: Value = match serde_json::from_str(&line) {
                    Ok(msg) => msg,
                    Err(_) => continue,
                };
                // Worker->host notification.
                if msg.get("method").is_some() && msg.get("id").is_none() {
                    match msg.get("method").and_then(Value::as_str) {
                        Some("ready") => {
                            if let Some(tx) = ready_tx.take() {
                                let _ = tx.send(());
                            }
                        }
                        Some("progress") => {
                            let params = msg.get("params").cloned().unwrap_or(Value::Null);
                            let run_id = params.get("runId").and_then(Value::as_str).unwrap_or("");
                            if let Some(host) = reader_inner.host(run_id) {
                                let mut metadata = json!({
                                    "toolCalls": params
                                        .get("toolCalls")
                                        .cloned()
                                        .unwrap_or_else(|| json!([])),
                                });
                                if let Some(files) = params.get("files").cloned() {
                                    metadata["files"] = files;
                                }
                                ctx_emit(&host.ctx, stark_contract::Delta::tool_progress(metadata));
                            }
                        }
                        _ => {}
                    }
                    continue;
                }
                // Response to a host->worker request.
                if msg.get("id").is_some() && msg.get("method").is_none() {
                    let id = msg
                        .get("id")
                        .and_then(|id| id.as_str())
                        .unwrap_or("")
                        .to_string();
                    let tx = reader_inner.pending.lock().unwrap().remove(&id);
                    if let Some(tx) = tx {
                        if let Some(error) = msg.get("error") {
                            let message = error
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("worker error");
                            let _ = tx.send(Err(message.to_string()));
                        } else {
                            let _ = tx.send(Ok(msg.get("result").cloned().unwrap_or(Value::Null)));
                        }
                    }
                    continue;
                }
                // Worker->host request (`tool_call` or `extension_call`).
                let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
                if method != "tool_call" && method != "extension_call" {
                    continue;
                }
                let id = msg
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let params = msg.get("params").cloned().unwrap_or(Value::Null);
                let responder = reader_inner.clone();
                // Bounded parallel dispatch. NOTE: `tool_call` must stay
                // `spawn_blocking` + `block_on`, not `tokio::spawn`.
                // serve_tool_call awaits dispatch, whose future can reach
                // back into worker startup (lazy first spawn), whose
                // future spawns this very task shape. That Send-proof
                // cycle cannot close over `tokio::spawn`; `block_on`
                // carries no Send bound on the future. The semaphore
                // caps blocking-pool threads well below the pool limit
                // (512), so nested spawn_blocking inside tools cannot
                // starve.
                let permit = match responder.dispatch_sem.clone().acquire_owned().await {
                    Ok(permit) => permit,
                    Err(_) => continue,
                };
                let method = method.to_string();
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    let result = tokio::runtime::Handle::current().block_on(async {
                        if method == "tool_call" {
                            Self::serve_tool_call(&responder, &params).await
                        } else {
                            Self::serve_extension_call(&responder, &params).await
                        }
                    });
                    let reply = json!({"id": id, "result": result});
                    let mut text = serde_json::to_string(&reply).unwrap_or_default();
                    text.push('\n');
                    tokio::runtime::Handle::current().block_on(async {
                        let mut stdin = responder.stdin.lock().await;
                        let _ = stdin.write_all(text.as_bytes()).await;
                    });
                });
            }
            reader_inner.dead.store(true, Ordering::Release);
            // Fail every outstanding request so execute() returns instead of hanging.
            let pending: Vec<PendingTx> = {
                let mut table = reader_inner.pending.lock().unwrap();
                table.drain().map(|(_, tx)| tx).collect()
            };
            for tx in pending {
                let _ = tx.send(Err("code mode worker exited".to_string()));
            }
        });
        // Wait for ready (Bun startup + Effect import); fail fast otherwise.
        match tokio::time::timeout(std::time::Duration::from_secs(60), ready_rx).await {
            Ok(Ok(())) => {}
            _ => {
                inner.dead.store(true, Ordering::Release);
                return Err("code mode worker did not become ready".to_string());
            }
        }
        {
            let mut guard = self.inner.lock().await;
            *guard = Some(inner.clone());
        }
        {
            let mut child_guard = self._child.lock().await;
            *child_guard = Some(child);
        }
        Ok(inner)
    }

    async fn serve_tool_call(inner: &WorkerInner, params: &Value) -> Value {
        let run_id = params.get("runId").and_then(Value::as_str).unwrap_or("");
        let _key = params.get("key").and_then(Value::as_str).unwrap_or("");
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let input = params.get("input").cloned().unwrap_or(json!({}));
        let Some(host) = inner.host(run_id) else {
            return json!({"ok": false, "output": format!("unknown code mode run: {run_id}")});
        };
        let ctx = &host.ctx;
        let dispatch_name = super::to_dispatch_name(name);
        let args = match &input {
            Value::Object(_) => input.clone(),
            _ => {
                return json!({"ok": false, "output": format!("tool {name} arguments must be a JSON object")});
            }
        };
        ctx_emit(
            ctx,
            stark_contract::Delta::tool_started(&dispatch_name, &args.to_string()),
        );
        let started = Instant::now();
        // Cancel-safe tools race the run token; file mutations run to
        // completion (Stop must never leave a half-written file).
        let cancel_safe = dispatch_name == "bash" || dispatch_name.starts_with("mcp__");
        let outcome: ToolOutcome = if cancel_safe {
            let child = ctx.cancel.child_token();
            let mut child_ctx = ctx.clone();
            child_ctx.cancel = child.clone();
            tokio::select! {
                biased;
                _ = child.cancelled() => crate::cancelled_outcome(&dispatch_name),
                outcome = crate::dispatch(&dispatch_name, &args, &child_ctx) => outcome,
            }
        } else {
            crate::dispatch(&dispatch_name, &args, ctx).await
        };
        let preview: String = outcome.content.chars().take(400).collect();
        ctx_emit(
            ctx,
            stark_contract::Delta::tool_finished(
                &dispatch_name,
                &args.to_string(),
                !outcome.is_error,
                started.elapsed().as_millis() as u64,
                preview,
            ),
        );
        if outcome.is_error {
            json!({
                "ok": false,
                "output": outcome.content,
                "kind": outcome.class.map(|class| format!("{class:?}")).unwrap_or_else(|| "Tool".to_string()),
            })
        } else {
            json!({
                "ok": true,
                "output": outcome.content,
                "metadata": outcome.metadata.unwrap_or(Value::Null),
            })
        }
    }

    async fn serve_extension_call(inner: &WorkerInner, params: &Value) -> Value {
        let run_id = params.get("runId").and_then(Value::as_str).unwrap_or("");
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let args = params
            .get("args")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let Some(host) = inner.host(run_id) else {
            return json!({
                "ok": false,
                "error": {"message": format!("unknown code mode run: {run_id}")},
            });
        };
        match extensions::handle(&host, name, &args).await {
            Ok(value) => json!({"ok": true, "value": value}),
            Err(error) => json!({
                "ok": false,
                "error": {
                    "message": error.message,
                    "code": error.code,
                },
            }),
        }
    }

    pub async fn initialize(&self, tools: &[WorkerToolDef]) -> Result<Vec<Value>, String> {
        let _initializing = self.initializing.lock().await;
        let defs = tool_defs_json(tools);
        let key = serde_json::to_string(&json!({"tools": defs})).unwrap_or_default();
        let inner = self.ensure_live().await?;
        if !inner.dead.load(Ordering::Acquire)
            && self
                .initialized
                .lock()
                .unwrap()
                .as_deref()
                .is_some_and(|known| known == key.as_str())
        {
            return Ok(Vec::new());
        }
        let request_id = inner.next_id("r");
        let body = json!({ "tools": defs });
        let result = inner.send(&request_id, "initialize", &body).await?;
        *self.initialized.lock().unwrap() = Some(key);
        Ok(result
            .get("catalog")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    pub(crate) async fn prepare(&self) -> Result<Arc<WorkerInner>, String> {
        self.ensure_live().await
    }

    /// Execute on an already-live worker. Unlike [`Self::initialize`], this
    /// future does not lazily spawn the worker, so it is `Send` and can be
    /// spawned normally.
    pub(crate) async fn execute_on(
        inner: Arc<WorkerInner>,
        run_id: &str,
        ctx: &ToolContext,
        code: &str,
        full: bool,
        tools: &[WorkerToolDef],
    ) -> Result<WorkerExecuteResult, String> {
        inner.insert_host(run_id, RunHost::new(ctx.clone()));
        let id = inner.next_id("r");
        // The worker is shared by every session in this harness. Carrying the
        // inventory on the call is what keeps two concurrent agents with
        // different tool sets from executing against each other's runtime.
        let params = json!({
            "code": code,
            "runId": run_id,
            "cwd": ctx.current_dir().to_string_lossy(),
            "full": full,
            "tools": tool_defs_json(tools),
        });
        let response = tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => {
                let _ = inner.send(&inner.next_id("r"), "cancel", &json!({"runId": run_id})).await;
                inner.remove_host(run_id);
                return Err("CANCELLED".to_string());
            }
            response = inner.send(&id, "execute", &params) => response,
        };
        inner.remove_host(run_id);
        let result = response?;
        parse_execute_result(&result)
    }

    pub async fn cancel(&self, run_id: &str) -> bool {
        let Ok(inner) = self.ensure_live().await else {
            return false;
        };
        let cancelled = inner
            .send(&inner.next_id("r"), "cancel", &json!({"runId": run_id}))
            .await
            .ok()
            .and_then(|value| value.get("cancelled").and_then(Value::as_bool))
            .unwrap_or(false);
        self.forget(run_id).await;
        cancelled
    }

    /// Drop a run's host context (cancelled or orphaned runs).
    pub async fn forget(&self, run_id: &str) {
        let guard = self.inner.lock().await;
        if let Some(inner) = guard.as_ref() {
            inner.remove_host(run_id);
        }
    }
}

/// The wire shape of one inventory entry, shared by `initialize` and every
/// `execute` so the worker builds the same runtime key both ways.
fn tool_defs_json(tools: &[WorkerToolDef]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "input_schema": tool.input_schema,
                "freeform": tool.freeform,
            })
        })
        .collect()
}

fn ctx_emit(ctx: &ToolContext, delta: stark_contract::Delta) {
    if let Some(callback) = &ctx.code_delta {
        callback(delta);
    }
}

fn parse_execute_result(value: &Value) -> Result<WorkerExecuteResult, String> {
    Ok(WorkerExecuteResult {
        text: value
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        error: value.get("error").and_then(Value::as_bool).unwrap_or(false),
        tool_calls: value
            .get("toolCalls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        files: value
            .get("files")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    })
}

fn discover_worker_dir(harness_root: &Path) -> PathBuf {
    if let Ok(dir) = std::env::var("STARK_CODEMODE_WORKER_DIR")
        && !dir.trim().is_empty()
    {
        return PathBuf::from(dir);
    }
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        candidates.push(parent.join("codemode-worker"));
        if let Some(parent) = parent.parent() {
            candidates.push(parent.join("codemode-worker"));
        }
    }
    candidates.push(harness_root.join("backend-rs").join("codemode-worker"));
    // Rust test binaries run from `target/debug/deps`.
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../codemode-worker"));
    candidates.push(PathBuf::from("backend-rs/codemode-worker"));
    candidates.push(PathBuf::from("codemode-worker"));
    candidates
        .into_iter()
        .find(|path| path.join("src").join("main.ts").is_file())
        .unwrap_or_else(|| PathBuf::from("backend-rs/codemode-worker"))
}

fn discover_bun() -> String {
    static BUN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    BUN.get_or_init(|| {
        if let Ok(bin) = std::env::var("STARK_BUN_BIN")
            && !bin.trim().is_empty()
        {
            return bin;
        }
        if let Some(hit) = probe_bun() {
            return hit;
        }
        "bun".to_string()
    })
    .clone()
}

/// Bun is usually installed outside PATH (npm global, ~/.bun). Probe the
/// well-known locations before falling back to PATH lookup.
fn probe_bun() -> Option<String> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let npm = PathBuf::from(appdata).join("npm");
        candidates.push(
            npm.join("node_modules")
                .join("bun")
                .join("bin")
                .join("bun.exe"),
        );
        candidates.push(npm.join("bun.cmd"));
    }
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let home = PathBuf::from(home);
        candidates.push(home.join(".bun").join("bin").join("bun.exe"));
        candidates.push(home.join(".bun").join("bin").join("bun"));
    }
    candidates.push(PathBuf::from("/usr/local/bin/bun"));
    candidates.push(PathBuf::from("/opt/homebrew/bin/bun"));
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .map(|path| path.to_string_lossy().into_owned())
}
