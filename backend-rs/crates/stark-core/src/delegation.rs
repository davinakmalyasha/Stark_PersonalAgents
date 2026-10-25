use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use chrono::Utc;
use serde_json::Value;
use stark_store::{ChatStore, JobRecord};
use stark_tools::ToolOutcome;
use tokio_util::sync::CancellationToken;

use crate::{JobDoneHook, RunContext, agent_for_scope, child_context, read_report, run_agent};

const MAX_JOBS: usize = 8;
const DEFAULT_TIMEOUT_SECS: u64 = 3_600;
const MAX_TIMEOUT_SECS: u64 = 86_400;
const MAX_REPORT_CHARS: usize = 12_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JobState {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl JobState {
    fn label(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

struct Job {
    id: String,
    parent_chat_id: String,
    agent: String,
    title: String,
    task: String,
    outcome: String,
    acceptance: String,
    chat_id: String,
    created_at: i64,
    status: Mutex<JobState>,
    report: Mutex<String>,
    error: Mutex<Option<String>>,
    steering: Arc<Mutex<VecDeque<String>>>,
    cancel: CancellationToken,
    on_done: Option<JobDoneHook>,
    notified: AtomicBool,
}

static JOBS: OnceLock<Mutex<HashMap<String, Arc<Job>>>> = OnceLock::new();

fn jobs() -> &'static Mutex<HashMap<String, Arc<Job>>> {
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn set_state(job: &Job, state: JobState) {
    *job.status.lock().unwrap() = state;
}

fn job_record(job: &Job) -> JobRecord {
    let status = *job.status.lock().unwrap();
    JobRecord {
        id: job.id.clone(),
        parent_chat_id: job.parent_chat_id.clone(),
        agent: job.agent.clone(),
        chat_id: job.chat_id.clone(),
        title: job.title.clone(),
        status: status.label().to_string(),
        task: job.task.clone(),
        outcome: job.outcome.clone(),
        acceptance: job.acceptance.clone(),
        report: job.report.lock().unwrap().clone(),
        error: job.error.lock().unwrap().clone(),
        created_at: job.created_at,
        finished_at: matches!(
            status,
            JobState::Done | JobState::Failed | JobState::Cancelled
        )
        .then(|| Utc::now().timestamp_millis()),
    }
}

async fn persist_job(store: &ChatStore, job: &Job) {
    if let Err(error) = store.jobs().upsert(&job_record(job)).await {
        tracing::warn!("failed to persist delegation job {}: {error}", job.id);
    }
}

#[derive(Clone)]
struct JobItem {
    agent: crate::AgentSpec,
    task: String,
    outcome: String,
    acceptance: String,
    title: String,
    chat_id: Option<String>,
    timeout_secs: u64,
}

pub(crate) async fn delegate(ctx: &RunContext, args: &Value) -> ToolOutcome {
    if ctx.depth >= 2 {
        return ToolOutcome::err("Error: delegation nesting limit reached");
    }
    let items = match parse_items(args) {
        Ok(items) => items,
        Err(error) => return ToolOutcome::err(error),
    };

    let mut prepared = Vec::with_capacity(items.len());
    for item in items {
        let (chat_id, title) = match &item.chat_id {
            Some(chat_id) => (chat_id.clone(), item.title.clone()),
            None => {
                match ctx
                    .store
                    .create_child_chat(
                        &ctx.workspace_id,
                        ctx.workspace.to_string_lossy().as_ref(),
                        &item.title,
                        &ctx.model,
                        &ctx.chat_id,
                    )
                    .await
                {
                    Ok(chat) => (
                        chat["id"].as_str().unwrap_or("").to_string(),
                        chat["title"].as_str().unwrap_or("").to_string(),
                    ),
                    Err(error) => {
                        return ToolOutcome::err(format!(
                            "Error: could not create job chat: {error}"
                        ));
                    }
                }
            }
        };
        let id = format!("del-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let job = Arc::new(Job {
            id: id.clone(),
            parent_chat_id: ctx.chat_id.clone(),
            agent: item.agent.display.to_string(),
            title,
            task: item.task.clone(),
            outcome: item.outcome.clone(),
            acceptance: item.acceptance.clone(),
            chat_id: chat_id.clone(),
            created_at: Utc::now().timestamp_millis(),
            status: Mutex::new(JobState::Queued),
            report: Mutex::new(String::new()),
            error: Mutex::new(None),
            steering: Arc::new(Mutex::new(VecDeque::new())),
            cancel: CancellationToken::new(),
            on_done: ctx.on_job_done.clone(),
            notified: AtomicBool::new(false),
        });
        jobs().lock().unwrap().insert(id, job.clone());
        persist_job(&ctx.store, &job).await;
        prepared.push((item, chat_id, job));
    }

    let job_refs: Vec<Arc<Job>> = prepared.iter().map(|(_, _, job)| job.clone()).collect();
    for (item, chat_id, job) in prepared {
        let cancel = job.cancel.clone();
        let (child, latest) = child_context(
            ctx,
            &chat_id,
            item.agent.display,
            job.steering.clone(),
            cancel.clone(),
        );
        let timeout_secs = item.timeout_secs;
        let prompt = job_prompt(&item);
        let store = ctx.store.clone();
        tokio::spawn(async move {
            set_state(&job, JobState::Running);
            persist_job(&store, &job).await;
            let run = if timeout_secs == 0 {
                Ok(run_agent(child, prompt).await)
            } else {
                tokio::time::timeout(
                    Duration::from_secs(timeout_secs.min(MAX_TIMEOUT_SECS)),
                    run_agent(child, prompt),
                )
                .await
            };
            match run {
                Ok(_) if cancel.is_cancelled() => {
                    set_state(&job, JobState::Cancelled);
                    *job.error.lock().unwrap() = Some("cancelled".to_string());
                    persist_job(&store, &job).await;
                }
                Ok(Ok(())) => {
                    *job.report.lock().unwrap() = read_report(&latest);
                    set_state(&job, JobState::Done);
                    persist_job(&store, &job).await;
                    notify_done(&job);
                }
                Ok(Err(error)) => {
                    *job.error.lock().unwrap() = Some(error.to_string());
                    set_state(&job, JobState::Failed);
                    persist_job(&store, &job).await;
                    notify_done(&job);
                }
                Err(_) => {
                    cancel.cancel();
                    *job.error.lock().unwrap() =
                        Some(format!("timed out after {timeout_secs} seconds"));
                    set_state(&job, JobState::Failed);
                    persist_job(&store, &job).await;
                    notify_done(&job);
                }
            }
        });
    }

    let lines: Vec<String> = job_refs
        .iter()
        .map(|job| {
            format!(
                "- {} · {} · {} — {} (chat: {})",
                job.id,
                job.agent,
                job.title,
                job.status.lock().unwrap().label(),
                job.chat_id
            )
        })
        .collect();
    ToolOutcome::ok(format!(
        "[DELEGATION STARTED]
{}
Use delegation_status with wait_seconds to collect results; job_message/job_cancel manage a running job.",
        lines.join("
")
    ))
}

pub(crate) async fn job_message(ctx: &RunContext, args: &Value) -> ToolOutcome {
    let Some(id) = args
        .get("job_id")
        .and_then(Value::as_str)
        .or_else(|| args.get("id").and_then(Value::as_str))
    else {
        return ToolOutcome::err("Error: job_message requires job_id");
    };
    let Some(message) = args.get("message").and_then(Value::as_str) else {
        return ToolOutcome::err("Error: job_message requires message");
    };
    let job = jobs().lock().unwrap().get(id).cloned();
    let Some(job) = job.filter(|job| job.parent_chat_id == ctx.chat_id) else {
        return ToolOutcome::err(format!("Error: unknown delegation '{id}'"));
    };
    if matches!(
        *job.status.lock().unwrap(),
        JobState::Queued | JobState::Running
    ) {
        job.steering.lock().unwrap().push_back(message.to_string());
        ToolOutcome::ok(format!("[OK] queued message for {id}"))
    } else {
        ToolOutcome::err(format!("Error: delegation '{id}' is already settled"))
    }
}

pub(crate) async fn job_cancel(ctx: &RunContext, args: &Value) -> ToolOutcome {
    let Some(id) = args
        .get("job_id")
        .and_then(Value::as_str)
        .or_else(|| args.get("id").and_then(Value::as_str))
    else {
        return ToolOutcome::err("Error: job_cancel requires job_id");
    };
    let job = jobs().lock().unwrap().get(id).cloned();
    let Some(job) = job.filter(|job| job.parent_chat_id == ctx.chat_id) else {
        return ToolOutcome::err(format!("Error: unknown delegation '{id}'"));
    };
    if matches!(
        *job.status.lock().unwrap(),
        JobState::Queued | JobState::Running
    ) {
        job.cancel.cancel();
        set_state(&job, JobState::Cancelled);
        let reason = args
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("cancelled by the user");
        *job.error.lock().unwrap() = Some(reason.to_string());
        persist_job(&ctx.store, &job).await;
        ToolOutcome::ok(format!("[OK] cancelled {id}"))
    } else {
        ToolOutcome::err(format!("Error: delegation '{id}' is already settled"))
    }
}

pub(crate) async fn delegation_status(ctx: &RunContext, args: &Value) -> ToolOutcome {
    let id = args.get("id").and_then(Value::as_str).unwrap_or("").trim();
    let wait_seconds = args
        .get("wait_seconds")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(300);
    if !id.is_empty() {
        let deadline = std::time::Instant::now() + Duration::from_secs(wait_seconds);
        loop {
            let job = jobs().lock().unwrap().get(id).cloned();
            let Some(job) = job else {
                return match ctx.store.jobs().get(id).await {
                    Ok(Some(record)) if record.parent_chat_id == ctx.chat_id => {
                        ToolOutcome::ok(record_detail(&record))
                    }
                    _ => ToolOutcome::err(format!("Error: unknown delegation '{id}'")),
                };
            };
            let state = *job.status.lock().unwrap();
            if !matches!(state, JobState::Queued | JobState::Running)
                || std::time::Instant::now() >= deadline
            {
                if matches!(
                    state,
                    JobState::Done | JobState::Failed | JobState::Cancelled
                ) {
                    job.notified.store(true, Ordering::SeqCst);
                }
                return ToolOutcome::ok(job_detail(&job));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    let status_filter = args
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let mut list: Vec<Arc<Job>> = jobs()
        .lock()
        .unwrap()
        .values()
        .filter(|job| job.parent_chat_id == ctx.chat_id)
        .filter(|job| {
            status_filter.is_empty() || job.status.lock().unwrap().label() == status_filter
        })
        .cloned()
        .collect();
    list.sort_by_key(|job| job.created_at);
    if list.is_empty() {
        let records = match ctx
            .store
            .jobs()
            .list(
                &ctx.chat_id,
                (!status_filter.is_empty()).then_some(status_filter.as_str()),
            )
            .await
        {
            Ok(records) => records,
            Err(error) => return ToolOutcome::err(format!("Error: {error}")),
        };
        if records.is_empty() {
            return ToolOutcome::ok("No delegations.".to_string());
        }
        let lines: Vec<String> = records.iter().map(record_line).collect();
        return ToolOutcome::ok(format!(
            "[DELEGATIONS]
{}",
            lines.join(
                "
"
            )
        ));
    }
    let lines: Vec<String> = list
        .iter()
        .map(|job| {
            format!(
                "- {} · {} · {} — {} (chat: {})",
                job.id,
                job.agent,
                job.title,
                job.status.lock().unwrap().label(),
                job.chat_id
            )
        })
        .collect();
    ToolOutcome::ok(format!("[DELEGATIONS]\n{}", lines.join("\n")))
}

fn parse_items(args: &Value) -> Result<Vec<JobItem>, String> {
    let Some(items) = args.get("items").and_then(Value::as_array) else {
        return Err("Error: delegate requires a non-empty items array".to_string());
    };
    if items.is_empty() {
        return Err("Error: delegate requires a non-empty items array".to_string());
    }
    if items.len() > MAX_JOBS {
        return Err(format!(
            "Error: at most {MAX_JOBS} jobs can be delegated at once"
        ));
    }

    let mut parsed = Vec::with_capacity(items.len());
    let mut chat_ids = std::collections::HashSet::new();
    for (index, raw) in items.iter().enumerate() {
        let object = raw
            .as_object()
            .ok_or_else(|| format!("Error: items[{index}] must be an object"))?;
        let agent_name = object
            .get("agent")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let agent = agent_for_scope(agent_name)
            .filter(|agent| matches!(agent.key, "developer" | "general" | "house"))
            .ok_or_else(|| {
                format!("Error: items[{index}].agent must be Developer, General, or House")
            })?;
        let task = object
            .get("task")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if task.is_empty() {
            return Err(format!("Error: items[{index}].task is required"));
        }
        let title = object
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let title = if title.is_empty() {
            task.chars().take(60).collect()
        } else {
            title
        };
        let chat_id = object
            .get("chat_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|chat_id| !chat_id.is_empty())
            .map(str::to_string);
        if let Some(chat_id) = &chat_id
            && !chat_ids.insert(chat_id.clone())
        {
            return Err(format!(
                "Error: items[{index}].chat_id is used more than once"
            ));
        }
        let timeout_secs = object
            .get("timeout")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_TIMEOUT_SECS);
        parsed.push(JobItem {
            agent,
            task,
            outcome: object
                .get("outcome")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
            acceptance: object
                .get("acceptance")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
            title,
            chat_id,
            timeout_secs,
        });
    }
    Ok(parsed)
}

fn job_prompt(item: &JobItem) -> String {
    let mut prompt = item.task.clone();
    if !item.outcome.is_empty() {
        prompt.push_str("\n\nExpected outcome:\n");
        prompt.push_str(&item.outcome);
    }
    if !item.acceptance.is_empty() {
        prompt.push_str("\n\nAcceptance criteria:\n");
        prompt.push_str(&item.acceptance);
    }
    prompt
}

fn notify_done(job: &Job) {
    if job.notified.swap(true, Ordering::SeqCst) {
        return;
    }
    let Some(hook) = &job.on_done else {
        return;
    };
    let state = *job.status.lock().unwrap();
    if !matches!(state, JobState::Done | JobState::Failed) {
        return;
    }
    let body = match state {
        JobState::Done => job.report.lock().unwrap().clone(),
        JobState::Failed => job
            .error
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| "(failed)".to_string()),
        _ => return,
    };
    hook(format!(
        "[DELEGATION RESULT] {} · {} · {} — {}
chat: {}
{}",
        job.id,
        job.agent,
        job.title,
        state.label(),
        job.chat_id,
        truncate(&body, MAX_REPORT_CHARS)
    ));
}

fn record_line(record: &JobRecord) -> String {
    format!(
        "- {} · {} · {} — {} (chat: {})",
        record.id, record.agent, record.title, record.status, record.chat_id
    )
}

fn record_detail(record: &JobRecord) -> String {
    let mut lines = vec![
        format!("[JOB {}]", record.id),
        format!("agent={}", record.agent),
        format!("status={}", record.status),
        format!("chat={}", record.chat_id),
        format!("title={}", record.title),
        format!("task={}", record.task),
    ];
    if !record.outcome.is_empty() {
        lines.push(format!("outcome={}", record.outcome));
    }
    if !record.acceptance.is_empty() {
        lines.push(format!("acceptance={}", record.acceptance));
    }
    if let Some(error) = &record.error {
        lines.push(format!("error={error}"));
    }
    if !record.report.is_empty() {
        lines.push(format!(
            "report:
{}",
            truncate(&record.report, MAX_REPORT_CHARS)
        ));
    }
    lines.join(
        "
",
    )
}

fn job_detail(job: &Job) -> String {
    let state = *job.status.lock().unwrap();
    let mut lines = vec![
        format!("[JOB {}]", job.id),
        format!("agent={}", job.agent),
        format!("status={}", state.label()),
        format!("chat={}", job.chat_id),
        format!("title={}", job.title),
        format!("task={}", job.task),
    ];
    if !job.outcome.is_empty() {
        lines.push(format!("outcome={}", job.outcome));
    }
    if !job.acceptance.is_empty() {
        lines.push(format!("acceptance={}", job.acceptance));
    }
    if let Some(error) = job.error.lock().unwrap().as_ref() {
        lines.push(format!("error={error}"));
    }
    let report = job.report.lock().unwrap();
    if !report.is_empty() {
        lines.push(format!("report:\n{}", truncate(&report, MAX_REPORT_CHARS)));
    }
    lines.join("\n")
}

fn truncate(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    let mut text: String = value.chars().take(limit).collect();
    text.push_str("\n[truncated]");
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn item(agent: &str, task: &str) -> Value {
        json!({"agent": agent, "task": task})
    }

    #[test]
    fn validates_items() {
        assert!(parse_items(&json!({"items": []})).is_err());
        assert!(parse_items(&json!({"items": [item("Orchestrator", "x")]})).is_err());
        assert!(parse_items(&json!({"items": [item("Developer", "")]})).is_err());
        let parsed = parse_items(&json!({
            "items": [item("developer", "Build it"), item("General", "Research it")]
        }))
        .unwrap();
        assert_eq!(parsed[0].agent.key, "developer");
        assert_eq!(parsed[1].agent.key, "general");
    }

    #[test]
    fn prompt_includes_outcome_and_acceptance() {
        let parsed = parse_items(&json!({
            "items": [{
                "agent": "Developer",
                "task": "Build it",
                "outcome": "A working build",
                "acceptance": "Tests pass"
            }]
        }))
        .unwrap();
        let prompt = job_prompt(&parsed[0]);
        assert!(prompt.contains("Expected outcome:\nA working build"));
        assert!(prompt.contains("Acceptance criteria:\nTests pass"));
    }

    #[tokio::test]
    async fn delegates_two_jobs_in_parallel_and_reports_status() {
        use std::sync::Arc;

        use axum::body::Body;
        use axum::http::header;
        use axum::response::Response;
        use axum::routing::post;
        use axum::{Json, Router};
        use tokio::sync::broadcast;

        use stark_providers::{Provider, ProviderConfig};

        async fn mock_provider(Json(_body): Json<Value>) -> Response {
            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"Job done\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n"
            );
            Response::builder()
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from(body))
                .unwrap()
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().route("/chat/completions", post(mock_provider));
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let dir = tempfile::tempdir().unwrap();
        let store = stark_store::ChatStore::open(dir.path()).await.unwrap();
        let provider = Provider::new(ProviderConfig {
            mode: "deepseek".to_string(),
            base_url: format!("http://{address}"),
            api_key: "test".to_string(),
            model: "mock".to_string(),
        })
        .unwrap();
        let (tx, _rx) = broadcast::channel(64);
        let notifications = Arc::new(Mutex::new(Vec::new()));
        let notifications_for_hook = notifications.clone();
        let ctx = RunContext {
            chat_id: "parent-delegate".to_string(),
            workspace_id: "_general".to_string(),
            workspace: dir.path().to_path_buf(),
            role: Some("Orchestrator".to_string()),
            scope: "orchestrator".to_string(),
            depth: 0,
            model: "mock".to_string(),
            temperature: 0.0,
            provider,
            store: store.clone(),
            questions: Arc::new(stark_tools::QuestionBroker::new()),
            tx,
            latest: Arc::new(Mutex::new(None)),
            steering: Arc::new(Mutex::new(VecDeque::new())),
            follow_up: Arc::new(Mutex::new(VecDeque::new())),
            checkpoint: None,
            cancel: CancellationToken::new(),
            on_job_done: Some(Arc::new(move |message| {
                notifications_for_hook.lock().unwrap().push(message);
            })),
        };

        let result = delegate(
            &ctx,
            &json!({
                "items": [
                    {"agent": "Developer", "task": "one", "title": "One"},
                    {"agent": "General", "task": "two", "title": "Two", "timeout": 10}
                ]
            }),
        )
        .await;
        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("DELEGATION STARTED"));
        assert!(result.content.contains("One"));
        assert!(result.content.contains("Two"));

        for _ in 0..100 {
            if !notifications.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            notifications
                .lock()
                .unwrap()
                .iter()
                .any(|message| message.contains("Job done"))
        );

        let ids: Vec<String> = jobs()
            .lock()
            .unwrap()
            .values()
            .filter(|job| job.parent_chat_id == ctx.chat_id)
            .map(|job| job.id.clone())
            .collect();
        assert_eq!(ids.len(), 2);
        let detail = delegation_status(&ctx, &json!({"id": ids[0], "wait_seconds": 10})).await;
        assert!(!detail.is_error, "{}", detail.content);
        assert!(detail.content.contains("status=done"));
        assert!(detail.content.contains("Job done"));

        let chats = store.list_chats("_general").await;
        assert_eq!(chats.len(), 2);
        let board = delegation_status(&ctx, &json!({})).await;
        assert!(!board.is_error, "{}", board.content);
        assert!(board.content.contains("One"));
        assert!(board.content.contains("Two"));
    }
}
