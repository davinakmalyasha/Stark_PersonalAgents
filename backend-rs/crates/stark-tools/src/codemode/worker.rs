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
use std::time::{Duration, Instant};

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

/// How long the host waits for a cancelled run to acknowledge before killing
/// the worker. A synchronous program blocks Bun's event loop, so the cancel
/// RPC can never be read; the child is the only thing the host can stop.
const CANCEL_GRACE: Duration = Duration::from_secs(2);

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

    fn fail_pending(&self, message: &str) {
        let pending: Vec<PendingTx> = {
            let mut table = self.pending.lock().unwrap();
            table.drain().map(|(_, tx)| tx).collect()
        };
        for tx in pending {
            let _ = tx.send(Err(message.to_string()));
        }
    }
}

pub struct WorkerManager {
    inner: AsyncMutex<Option<Arc<WorkerInner>>>,
    harness_root: PathBuf,
    /// Milliseconds since the Unix epoch; drives idle pruning.
    last_used: AtomicU64,
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

    /// One manager per harness root. Kept for callers that do not have a chat
    /// identity (tests, one-shot tooling).
    pub fn for_root(harness_root: &Path) -> Arc<Self> {
        Self::for_chat(harness_root, "__root__")
    }

    /// One worker per chat, so a synchronous program in one conversation
    /// cannot block or kill another conversation's worker. Idle managers are
    /// reaped when a new chat asks for one; the process cap falls back to the
    /// least-recently-used manager rather than failing a run.
    pub fn for_chat(harness_root: &Path, chat_id: &str) -> Arc<Self> {
        let root = harness_root.to_string_lossy().to_string();
        let key = format!("{root}\0{}", chat_id.trim());
        let mut managers = Self::global().lock().unwrap();
        prune_workers(&mut managers, &root);
        if let Some(manager) = managers.get(&key) {
            manager.touch();
            return manager.clone();
        }
        if managers.len() >= worker_limit()
            && let Some((_, lru)) = managers
                .iter()
                .min_by_key(|(_, manager)| manager.last_used.load(Ordering::Relaxed))
        {
            tracing::warn!(
                chat_id = %chat_id,
                "Code Mode worker cap reached; sharing the least-recently-used worker"
            );
            lru.touch();
            return lru.clone();
        }
        let manager = Arc::new(Self::new(harness_root));
        managers.insert(key, manager.clone());
        manager
    }

    /// Reap idle chat workers. Called periodically by the server so an idle
    /// conversation does not keep a Bun process alive forever.
    pub fn sweep_idle() {
        let mut managers = Self::global().lock().unwrap();
        prune_workers(&mut managers, "");
    }

    fn touch(&self) {
        self.last_used.store(now_millis(), Ordering::Relaxed);
    }

    fn idle(&self) -> bool {
        let Ok(inner) = self.inner.try_lock() else {
            return false;
        };
        inner
            .as_ref()
            .map(|inner| {
                inner
                    .runs
                    .lock()
                    .map(|runs| runs.is_empty())
                    .unwrap_or(false)
                    && inner
                        .pending
                        .lock()
                        .map(|pending| pending.is_empty())
                        .unwrap_or(false)
            })
            .unwrap_or(true)
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
            last_used: AtomicU64::new(now_millis()),
            starting: AsyncMutex::new(()),
            initializing: AsyncMutex::new(()),
            initialized: Mutex::new(None),
            _child: AsyncMutex::new(None),
        }
    }

    /// Stop this manager's worker without removing it from the chat map.
    /// Tests use this so a global manager cannot keep the runtime alive.
    pub async fn shutdown(&self) {
        let inner = { self.inner.lock().await.clone() };
        if let Some(inner) = inner {
            self.kill(&inner, "manager shutdown").await;
            return;
        }
        let child = { self._child.lock().await.take() };
        if let Some(mut child) = child {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
        }
    }

    /// Last-resort shutdown for a worker that will not answer `cancel`.
    /// Concurrent runs fail with `reason`; the next call respawns lazily.
    /// `target` is checked so a worker spawned after the timeout is never
    /// killed by mistake.
    pub(crate) async fn kill(&self, target: &Arc<WorkerInner>, reason: &str) {
        // Serialize with lazy startup so the child we take is the target's.
        let _starting = self.starting.lock().await;
        let inner = {
            let mut guard = self.inner.lock().await;
            match guard.as_ref() {
                Some(inner) if Arc::ptr_eq(inner, target) => guard.take(),
                _ => None,
            }
        };
        let Some(inner) = inner else {
            return;
        };
        inner.dead.store(true, Ordering::Release);
        inner.fail_pending(reason);
        inner.runs.lock().unwrap().clear();
        *self.initialized.lock().unwrap() = None;
        let child = { self._child.lock().await.take() };
        drop(_starting);
        if let Some(mut child) = child {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
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
        let launch = discover_launch(&self.harness_root)?;
        tracing::debug!(
            runtime = launch.kind(),
            worker = %launch.description(),
            "spawning code mode worker"
        );
        let mut command = launch.command(&self.harness_root);
        let mut child: Child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| {
                format!(
                    "failed to spawn code mode worker ({}): {error}",
                    launch.description()
                )
            })?;
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
            stark_contract::Delta::tool_started(&dispatch_name, args.to_string()),
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
                args.to_string(),
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
        &self,
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
                let cancel_id = inner.next_id("r");
                let cancel = tokio::time::timeout(
                    CANCEL_GRACE,
                    inner.send(&cancel_id, "cancel", &json!({"runId": run_id})),
                )
                .await;
                match cancel {
                    Ok(Ok(_)) => {}
                    Ok(Err(error)) => {
                        tracing::warn!(
                            run_id = %run_id,
                            error = %error,
                            "code mode worker cancel failed; killing it"
                        );
                        ctx.audit_event(
                            "worker_kill",
                            run_id,
                            json!({"reason": "cancel_failed", "error": error}),
                        );
                        self.kill(&inner, "code mode worker cancel failed").await;
                    }
                    Err(_) => {
                        inner.pending.lock().unwrap().remove(&cancel_id);
                        tracing::warn!(
                            run_id = %run_id,
                            "code mode worker did not acknowledge cancel; killing it"
                        );
                        ctx.audit_event(
                            "worker_kill",
                            run_id,
                            json!({"reason": "cancel_timeout"}),
                        );
                        self.kill(&inner, "code mode worker did not acknowledge cancel")
                            .await;
                    }
                }
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
        // Cancel never spawns a worker: a run can only exist on a live child.
        let inner = {
            let guard = self.inner.lock().await;
            match guard.as_ref() {
                Some(inner) if !inner.dead.load(Ordering::Acquire) => inner.clone(),
                _ => return false,
            }
        };
        let cancel_id = inner.next_id("r");
        let response = tokio::time::timeout(
            CANCEL_GRACE,
            inner.send(&cancel_id, "cancel", &json!({"runId": run_id})),
        )
        .await;
        let cancelled = match response {
            Ok(Ok(value)) => value
                .get("cancelled")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            Ok(Err(error)) => {
                tracing::warn!(
                    run_id = %run_id,
                    error = %error,
                    "code mode worker cancel failed; killing it"
                );
                self.kill(&inner, "code mode worker cancel failed").await;
                false
            }
            Err(_) => {
                inner.pending.lock().unwrap().remove(&cancel_id);
                tracing::warn!(
                    run_id = %run_id,
                    "code mode worker did not acknowledge cancel; killing it"
                );
                self.kill(&inner, "code mode worker did not acknowledge cancel")
                    .await;
                false
            }
        };
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

enum WorkerLaunch {
    Binary(PathBuf),
    Source { bun: String, dir: PathBuf },
}

impl WorkerLaunch {
    fn kind(&self) -> &'static str {
        match self {
            Self::Binary(_) => "binary",
            Self::Source { .. } => "source",
        }
    }

    fn description(&self) -> String {
        match self {
            Self::Binary(path) => path.display().to_string(),
            Self::Source { bun, dir } => format!("{} {}", bun, dir.join("src/main.ts").display()),
        }
    }

    fn command(&self, harness_root: &Path) -> Command {
        match self {
            Self::Binary(path) => {
                let mut command = Command::new(path);
                command.current_dir(harness_root);
                command
            }
            Self::Source { bun, dir } => {
                let mut command = Command::new(bun);
                command
                    .arg("run")
                    .arg("--silent")
                    .arg("./src/main.ts")
                    .current_dir(dir);
                command
            }
        }
    }
}

/// Runtime availability for `/api/health`; discovery only, never spawns.
pub fn runtime_status(harness_root: &Path) -> Value {
    match discover_launch(harness_root) {
        Ok(launch) => json!({
            "available": true,
            "kind": launch.kind(),
        }),
        Err(error) => json!({
            "available": false,
            "kind": "missing",
            "error": error,
        }),
    }
}

fn worker_binary_name() -> &'static str {
    if cfg!(windows) {
        "codemode-worker.exe"
    } else {
        "codemode-worker"
    }
}

fn binary_candidates(harness_root: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(raw) = std::env::var("STARK_CODEMODE_WORKER_BIN") {
        let path = PathBuf::from(raw.trim());
        if path.is_dir() {
            candidates.push(path.join(worker_binary_name()));
        } else if !path.as_os_str().is_empty() {
            candidates.push(path);
        }
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        candidates.push(parent.join(worker_binary_name()));
        candidates.push(parent.join("resources").join(worker_binary_name()));
        if let Some(parent) = parent.parent() {
            candidates.push(parent.join("Resources").join(worker_binary_name()));
            candidates.push(parent.join(worker_binary_name()));
        }
    }
    for root in [
        harness_root.join(".deep_agents").join("bin"),
        harness_root
            .join("backend-rs")
            .join("codemode-worker")
            .join("dist"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../codemode-worker")
            .join("dist"),
        PathBuf::from("backend-rs/codemode-worker/dist"),
    ] {
        candidates.push(root.join(worker_binary_name()));
    }
    candidates
}

fn discover_launch(harness_root: &Path) -> Result<WorkerLaunch, String> {
    if let Some(binary) = binary_candidates(harness_root)
        .into_iter()
        .find(|path| path.is_file())
    {
        return Ok(WorkerLaunch::Binary(binary));
    }
    let worker_dir = discover_worker_dir(harness_root);
    let main = worker_dir.join("src").join("main.ts");
    if main.is_file() {
        return Ok(WorkerLaunch::Source {
            bun: discover_bun(),
            dir: worker_dir,
        });
    }
    Err(format!(
        "code mode worker missing. Set STARK_CODEMODE_WORKER_BIN to the packaged binary, or STARK_CODEMODE_WORKER_DIR to the source tree (checked {}).",
        harness_root.display()
    ))
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn worker_limit() -> usize {
    std::env::var("STARK_CODEMODE_MAX_WORKERS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(8)
}

fn worker_idle_ms() -> u64 {
    std::env::var("STARK_CODEMODE_WORKER_IDLE_SECS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(1_800)
        .saturating_mul(1_000)
}

fn prune_workers(managers: &mut HashMap<String, Arc<WorkerManager>>, _root: &str) {
    let now = now_millis();
    let idle_ms = worker_idle_ms();
    managers.retain(|_, manager| {
        now.saturating_sub(manager.last_used.load(Ordering::Relaxed)) < idle_ms || !manager.idle()
    });
    let limit = worker_limit();
    while managers.len() >= limit {
        let oldest = managers
            .iter()
            .filter(|(_, manager)| manager.idle())
            .min_by_key(|(_, manager)| manager.last_used.load(Ordering::Relaxed))
            .map(|(key, _)| key.clone());
        let Some(oldest) = oldest else {
            break;
        };
        managers.remove(&oldest);
    }
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
