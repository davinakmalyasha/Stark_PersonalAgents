//! Code Mode background jobs: OpenCode v2 `packages/core/src/codemode/job.ts`,
//! adapted to Stark's follow-up mechanism.
//!
//! With `full_codemode`, foreground `execute` blocks up to `timeout` (default
//! 2 minutes). If the script is still running, the call returns immediately
//! with `Script cm_... is still running; its result will arrive in this
//! session.` and `metadata: { jobID }`, while the script keeps running. On
//! completion the result is delivered as a synthetic
//! `<background-result kind="codemode" ...>` message through the run's
//! background hook.
//!
//! OpenCode does **not** resume Code Mode programs after a server restart: a
//! running job becomes `Interrupted by server restart.` and the session turn
//! resumes. We persist enough to deliver that notice exactly once.
//!
//! MIT attribution: job shape and notice text © OpenCode authors (MIT).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

/// Foreground block before a script moves to the background.
/// OpenCode `BLOCK_TIMEOUT = "2 minutes"`.
pub const BLOCK_TIMEOUT_MS: u64 = 2 * 60 * 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobStatus {
    Running,
    Completed,
    Error,
    Cancelled,
}

impl JobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone)]
pub struct JobInfo {
    pub id: String,
    pub chat_id: String,
    pub status: JobStatus,
    pub output: Option<String>,
    pub error: Option<String>,
    pub started_at_ms: u64,
    pub completed_at_ms: Option<u64>,
}

/// A restart notice owed to a chat. OpenCode's `SessionRestart` delivers these
/// once, then acknowledges the durable job row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobNotice {
    pub job_id: String,
    pub chat_id: String,
    pub message: String,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// OpenCode `CodeModeJob.notice`: state mapping is byte-identical, including
/// `"Interrupted by server restart."` for jobs that were running.
pub fn notice(id: &str, status: &JobStatus, output: Option<&str>, error: Option<&str>) -> Value {
    let state = match status {
        JobStatus::Running => "cancelled",
        JobStatus::Completed => "completed",
        JobStatus::Error => "error",
        JobStatus::Cancelled => "cancelled",
    };
    let body = match status {
        JobStatus::Running => "Interrupted by server restart.".to_string(),
        _ => output.or(error).unwrap_or(state).to_string(),
    };
    json!({
        "description": "Script",
        "text": background_result(&format!("kind=\"codemode\" id=\"{id}\" state=\"{state}\""), &body),
        "metadata": {"source": "codemode", "jobID": id, "state": state},
    })
}

/// OpenCode `BackgroundResult.render`: `<background-result ...>\nbody\n</background-result>`.
pub fn background_result(attributes: &str, body: &str) -> String {
    format!("<background-result {attributes}>\n{body}\n</background-result>")
}

/// The follow-up message delivered when a background script finishes.
pub fn completion_message(info: &JobInfo) -> String {
    notice(
        &info.id,
        &info.status,
        info.output.as_deref(),
        info.error.as_deref(),
    )
    .get("text")
    .and_then(Value::as_str)
    .unwrap_or("")
    .to_string()
}

pub fn still_running_text(id: &str) -> String {
    format!("Script {id} is still running; its result will arrive in this session.")
}

fn snapshot_dir(harness_root: &Path) -> PathBuf {
    harness_root.join(".deep_agents").join("codemode")
}

fn job_file(harness_root: &Path, id: &str) -> PathBuf {
    snapshot_dir(harness_root).join(format!("{id}.json"))
}

#[allow(clippy::too_many_arguments)]
fn write_job(
    path: &Path,
    status: &JobStatus,
    chat_id: &str,
    output: Option<&str>,
    error: Option<&str>,
    started_at_ms: u64,
    completed_at_ms: Option<u64>,
    delivered: bool,
) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(
        path,
        serde_json::to_string_pretty(&json!({
            "status": status.as_str(),
            "chat_id": chat_id,
            "output": output,
            "error": error,
            "started_at_ms": started_at_ms,
            "completed_at_ms": completed_at_ms,
            "notice_delivered": delivered,
        }))
        .unwrap_or_default(),
    );
}

#[derive(Debug, Clone)]
struct StoredJob {
    info: JobInfo,
    delivered: bool,
}

pub struct JobManager {
    harness_root: PathBuf,
    jobs: Mutex<HashMap<String, StoredJob>>,
}

impl JobManager {
    fn global() -> &'static Mutex<HashMap<String, Arc<JobManager>>> {
        use std::sync::OnceLock as StdOnceLock;
        static MANAGERS: StdOnceLock<Mutex<HashMap<String, Arc<JobManager>>>> = StdOnceLock::new();
        MANAGERS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub fn for_root(harness_root: &Path) -> Arc<Self> {
        let key = harness_root.to_string_lossy().to_string();
        let mut managers = Self::global().lock().unwrap();
        if let Some(manager) = managers.get(&key) {
            return manager.clone();
        }
        let manager = Arc::new(Self {
            harness_root: harness_root.to_path_buf(),
            jobs: Mutex::new(HashMap::new()),
        });
        managers.insert(key, manager.clone());
        manager
    }

    pub fn start(&self, id: &str, chat_id: &str) {
        let now = now_ms();
        let mut jobs = self.jobs.lock().unwrap();
        jobs.insert(
            id.to_string(),
            StoredJob {
                info: JobInfo {
                    id: id.to_string(),
                    chat_id: chat_id.to_string(),
                    status: JobStatus::Running,
                    output: None,
                    error: None,
                    started_at_ms: now,
                    completed_at_ms: None,
                },
                delivered: false,
            },
        );
        drop(jobs);
        self.persist(id);
    }

    pub fn finish(
        &self,
        id: &str,
        status: JobStatus,
        output: Option<String>,
        error: Option<String>,
    ) {
        {
            let mut jobs = self.jobs.lock().unwrap();
            let Some(stored) = jobs.get_mut(id) else {
                return;
            };
            stored.info.status = status;
            stored.info.output = output;
            stored.info.error = error;
            stored.info.completed_at_ms = Some(now_ms());
        }
        self.persist(id);
    }

    pub fn harness_root(&self) -> &Path {
        &self.harness_root
    }

    pub fn get(&self, id: &str) -> Option<JobInfo> {
        self.jobs
            .lock()
            .unwrap()
            .get(id)
            .map(|stored| stored.info.clone())
    }

    /// Marks the durable row's completion notice as delivered, then drops the
    /// row. OpenCode deletes a delivered background job instead of keeping its
    /// output in memory and on disk forever (`Job.completeBackground`).
    pub fn mark_delivered(&self, id: &str) {
        {
            let mut jobs = self.jobs.lock().unwrap();
            if jobs.remove(id).is_none() {
                return;
            }
        }
        let _ = std::fs::remove_file(job_file(&self.harness_root, id));
    }

    fn persist(&self, id: &str) {
        let stored = self.jobs.lock().unwrap().get(id).cloned();
        let Some(stored) = stored else { return };
        write_job(
            &job_file(&self.harness_root, id),
            &stored.info.status,
            &stored.info.chat_id,
            stored.info.output.as_deref(),
            stored.info.error.as_deref(),
            stored.info.started_at_ms,
            stored.info.completed_at_ms,
            stored.delivered,
        );
    }
}

/// Deletes a delivered notice row. OpenCode removes the durable background
/// row only after the synthetic message is recorded (`completeBackground`).
pub fn acknowledge_notice(harness_root: &Path, id: &str) {
    let _ = std::fs::remove_file(job_file(harness_root, id));
}

/// Scans persisted Code Mode jobs at server boot. Running jobs become
/// `Interrupted by server restart.` (OpenCode never resumes the program);
/// finished jobs whose notice never reached the session are returned too.
/// A row stays on disk until the server acknowledges delivery, so a crash
/// between recovery and delivery produces the notice again rather than losing
/// it.
pub fn recover_notices(harness_root: &Path) -> Vec<JobNotice> {
    let dir = snapshot_dir(harness_root);
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect();
    paths.sort();
    let mut notices = Vec::new();
    for path in paths {
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(stored) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            continue;
        }
        let chat_id = stored
            .get("chat_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if chat_id.is_empty() {
            continue;
        }
        let delivered = stored
            .get("notice_delivered")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if delivered {
            // No row should carry this bit now, but a pre-ack build may have
            // left one. It has already been delivered, so remove it.
            let _ = std::fs::remove_file(&path);
            continue;
        }
        let status = match stored.get("status").and_then(Value::as_str) {
            Some("running") => JobStatus::Running,
            Some("completed") => JobStatus::Completed,
            Some("error") => JobStatus::Error,
            Some("cancelled") => JobStatus::Cancelled,
            _ => JobStatus::Cancelled,
        };
        let output = stored.get("output").and_then(Value::as_str);
        let error = stored.get("error").and_then(Value::as_str);
        let info = JobInfo {
            id: id.clone(),
            chat_id: chat_id.clone(),
            status: status.clone(),
            output: output.map(str::to_string),
            error: error.map(str::to_string),
            started_at_ms: stored
                .get("started_at_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            completed_at_ms: stored.get("completed_at_ms").and_then(Value::as_u64),
        };
        notices.push(JobNotice {
            job_id: id,
            chat_id,
            message: completion_message(&info),
        });
    }
    notices
}

/// OpenCode `Identifier.create(false)`: 12 lowercase hex characters encoding
/// `Date.now() * 0x1000 + counter`, then 14 base62 random characters. The
/// random tail differs per call, but the id shape is the same.
pub fn new_identifier() -> String {
    use std::sync::Mutex;
    static CLOCK: Mutex<(u64, u64)> = Mutex::new((0, 0));
    let timestamp = now_ms();
    let counter = {
        let mut clock = CLOCK.lock().unwrap_or_else(|poison| poison.into_inner());
        if clock.0 != timestamp {
            clock.0 = timestamp;
            clock.1 = 0;
        }
        clock.1 += 1;
        clock.1
    };
    let value = timestamp.saturating_mul(0x1000).saturating_add(counter);
    let mut id = String::with_capacity(26);
    for shift in [40u64, 32, 24, 16, 8, 0] {
        id.push_str(&format!("{:02x}", (value >> shift) & 0xff));
    }
    const CHARS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let bytes = uuid::Uuid::new_v4();
    for byte in bytes.as_bytes().iter().take(14) {
        id.push(CHARS[(*byte as usize) % CHARS.len()] as char);
    }
    id
}

/// OpenCode `CodeModeJob` id: `cm_` plus an ascending identifier.
pub fn new_job_id() -> String {
    format!("cm_{}", new_identifier())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_running_script_reports_interruption_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = job_file(dir.path(), "cm_test");
        write_job(
            &path,
            &JobStatus::Running,
            "chat-1",
            None,
            None,
            1,
            None,
            false,
        );
        let notices = recover_notices(dir.path());
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].chat_id, "chat-1");
        assert_eq!(
            notices[0].message,
            "<background-result kind=\"codemode\" id=\"cm_test\" state=\"cancelled\">\nInterrupted by server restart.\n</background-result>"
        );
        // Recovery does not acknowledge by itself: a crash before delivery
        // must produce the notice again.
        assert_eq!(recover_notices(dir.path()).len(), 1);
        acknowledge_notice(dir.path(), "cm_test");
        assert!(recover_notices(dir.path()).is_empty());
    }

    #[test]
    fn delivered_job_row_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let manager = JobManager::for_root(dir.path());
        manager.start("cm_delivered", "chat-1");
        manager.finish(
            "cm_delivered",
            JobStatus::Completed,
            Some("done".to_string()),
            None,
        );
        let path = job_file(dir.path(), "cm_delivered");
        assert!(path.is_file());
        manager.mark_delivered("cm_delivered");
        assert!(manager.get("cm_delivered").is_none());
        assert!(!path.exists());
    }

    #[test]
    fn restart_completed_script_reports_its_result_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = job_file(dir.path(), "cm_done");
        write_job(
            &path,
            &JobStatus::Completed,
            "chat-2",
            Some("done"),
            None,
            1,
            Some(2),
            false,
        );
        let notices = recover_notices(dir.path());
        assert_eq!(notices.len(), 1);
        assert!(notices[0].message.contains("done"));
        acknowledge_notice(dir.path(), "cm_done");
        assert!(recover_notices(dir.path()).is_empty());
    }
}
