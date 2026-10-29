//! Model-assisted workspace memory consolidation.
//!
//! Deterministic capture happens at the end of every turn. This module is the
//! slower distillation layer: it folds pending stage-1 records into durable
//! entries, a compact index, and topic files. If the model is unavailable or
//! returns junk, the deterministic fallback still produces useful memory.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use stark_providers::{ChatMessage, Provider, ProviderSession};
use stark_store::{MemoryStore, NewEntry};
use tokio_util::sync::CancellationToken;

const MAX_RECORDS: usize = 40;
const MAX_ENTRIES: usize = 24;
const MAX_PROMPT_CHARS: usize = 60_000;
const MODEL_TIMEOUT_SECS: u64 = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsolidateMode {
    Model,
    Deterministic,
}

#[derive(Clone, Debug)]
pub struct ConsolidationOutcome {
    pub summary: String,
    pub entries: usize,
    pub source: String,
    pub model: String,
    pub fallback: bool,
}

#[derive(Debug, Deserialize)]
struct ModelMemory {
    #[serde(default)]
    summary: String,
    #[serde(default)]
    entries: Vec<ModelEntry>,
}

#[derive(Debug, Deserialize)]
struct ModelEntry {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    topic: String,
    #[serde(default)]
    text: String,
}

fn locks() -> &'static Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn workspace_lock(workspace_id: &str) -> Arc<tokio::sync::Mutex<()>> {
    locks()
        .lock()
        .unwrap()
        .entry(workspace_id.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

pub fn provider_for_memory(base: &Provider) -> Provider {
    let model = std::env::var("STARK_MEMORY_MODEL")
        .unwrap_or_default()
        .trim()
        .to_string();
    if model.is_empty() {
        return base.clone();
    }
    base.with_model(&model).unwrap_or_else(|_| base.clone())
}

pub async fn maybe_consolidate(
    provider: &Provider,
    cancel: &CancellationToken,
    store: &MemoryStore,
    workspace_id: &str,
) -> Result<Option<ConsolidationOutcome>, String> {
    if !stark_store::memory_enabled() {
        return Ok(None);
    }
    let pending = store
        .pending_count(workspace_id)
        .await
        .map_err(|error| error.to_string())?;
    if pending < stark_store::memory_consolidate_after() as i64 {
        return Ok(None);
    }
    consolidate_workspace(
        provider,
        cancel,
        store,
        workspace_id,
        ConsolidateMode::Model,
    )
    .await
    .map(Some)
}

pub async fn consolidate_workspace(
    provider: &Provider,
    cancel: &CancellationToken,
    store: &MemoryStore,
    workspace_id: &str,
    mode: ConsolidateMode,
) -> Result<ConsolidationOutcome, String> {
    let lock = workspace_lock(workspace_id);
    let _guard = lock.lock().await;

    if mode == ConsolidateMode::Deterministic {
        let summary = store
            .consolidate(workspace_id, MAX_RECORDS)
            .await
            .map_err(|error| error.to_string())?;
        let status = store
            .status(workspace_id)
            .await
            .map_err(|error| error.to_string())?;
        return Ok(ConsolidationOutcome {
            summary,
            entries: status["entryCount"].as_u64().unwrap_or(0) as usize,
            source: "deterministic".to_string(),
            model: String::new(),
            fallback: false,
        });
    }

    let records = store
        .pending_stage1(workspace_id, MAX_RECORDS)
        .await
        .map_err(|error| error.to_string())?;
    if records.is_empty() {
        return Ok(ConsolidationOutcome {
            summary: store
                .summary(workspace_id, stark_store::memory_max_chars())
                .await
                .unwrap_or_default(),
            entries: 0,
            source: "none".to_string(),
            model: String::new(),
            fallback: false,
        });
    }

    let existing = store
        .summary(workspace_id, stark_store::memory_max_chars())
        .await
        .unwrap_or_default();
    let prompt = build_prompt(workspace_id, &existing, &records);
    let model_provider = provider_for_memory(provider);
    let session_key = format!("memory-{}", safe_session(workspace_id));
    let session = ProviderSession {
        session_id: &session_key,
        affinity: &session_key,
    };
    let messages = vec![
        ChatMessage::system(
            "You maintain durable workspace memory for an autonomous coding agent. \
             You preserve decisions, conventions, commands, pitfalls, preferences, file \
             locations, and unresolved work across sessions. You never call tools. \
             Return JSON only.",
        ),
        ChatMessage::user(prompt),
    ];

    let model = model_provider.config().model.clone();
    let result = tokio::select! {
        _ = cancel.cancelled() => Err("memory consolidation cancelled".to_string()),
        result = tokio::time::timeout(
            Duration::from_secs(MODEL_TIMEOUT_SECS),
            model_provider.complete_text_for_session(&messages, 0.0, session),
        ) => match result {
            Ok(Ok(completion)) => Ok(completion),
            Ok(Err(error)) => Err(error.to_string()),
            Err(_) => Err("memory consolidation timed out".to_string()),
        },
    };

    let completion = match result {
        Ok(completion) => completion,
        Err(error) => {
            tracing::warn!(
                "memory consolidation model failed: {error}; using deterministic fallback"
            );
            let summary = store
                .consolidate(workspace_id, MAX_RECORDS)
                .await
                .map_err(|error| error.to_string())?;
            let status = store
                .status(workspace_id)
                .await
                .map_err(|error| error.to_string())?;
            return Ok(ConsolidationOutcome {
                summary,
                entries: status["entryCount"].as_u64().unwrap_or(0) as usize,
                source: "deterministic".to_string(),
                model: String::new(),
                fallback: true,
            });
        }
    };

    let parsed = match parse_model_output(&completion.text) {
        Ok(parsed) => parsed,
        Err(error) => {
            tracing::warn!("memory consolidation returned invalid output: {error}; using fallback");
            let summary = store
                .consolidate(workspace_id, MAX_RECORDS)
                .await
                .map_err(|error| error.to_string())?;
            let status = store
                .status(workspace_id)
                .await
                .map_err(|error| error.to_string())?;
            return Ok(ConsolidationOutcome {
                summary,
                entries: status["entryCount"].as_u64().unwrap_or(0) as usize,
                source: "deterministic".to_string(),
                model: String::new(),
                fallback: true,
            });
        }
    };

    let summary = if parsed.summary.trim().is_empty() {
        "## Objective\nConsolidated workspace memory.".to_string()
    } else {
        parsed.summary.trim().to_string()
    };
    let consumed: Vec<String> = records.iter().map(|record| record.id.clone()).collect();
    let entries: Vec<NewEntry> = parsed
        .entries
        .into_iter()
        .filter(|entry| !entry.text.trim().is_empty())
        .take(MAX_ENTRIES)
        .map(|entry| NewEntry {
            chat_id: String::new(),
            agent_id: None,
            kind: entry.kind,
            topic: entry.topic,
            text: entry.text,
            source: "model".to_string(),
            model: model.clone(),
            pinned: false,
        })
        .collect();
    if entries.is_empty() {
        tracing::warn!("memory consolidation returned no entries; using fallback");
        let summary = store
            .consolidate(workspace_id, MAX_RECORDS)
            .await
            .map_err(|error| error.to_string())?;
        let status = store
            .status(workspace_id)
            .await
            .map_err(|error| error.to_string())?;
        return Ok(ConsolidationOutcome {
            summary,
            entries: status["entryCount"].as_u64().unwrap_or(0) as usize,
            source: "deterministic".to_string(),
            model: String::new(),
            fallback: true,
        });
    }
    let usage = serde_json::to_value(&completion.usage).unwrap_or_else(|_| json!({}));
    store
        .apply_consolidation(
            workspace_id,
            &summary,
            &entries,
            &consumed,
            "model",
            &model,
            usage,
        )
        .await
        .map_err(|error| error.to_string())?;

    Ok(ConsolidationOutcome {
        summary,
        entries: entries.len(),
        source: "model".to_string(),
        model,
        fallback: false,
    })
}

fn build_prompt(
    workspace_id: &str,
    existing: &str,
    records: &[stark_store::Stage1Record],
) -> String {
    let mut text = format!(
        "Workspace: {workspace_id}\n\nExisting consolidated memory:\n{}\n\nNew completed turns:\n",
        if existing.trim().is_empty() {
            "(none)"
        } else {
            existing
        }
    );
    let mut used = text.chars().count();
    for (index, record) in records.iter().enumerate() {
        let entry = format!(
            "\n[{}] chat={} at={}\nrequest: {}\noutcome:\n{}\n",
            index + 1,
            record.chat_id,
            record.created_at,
            record.request.chars().take(800).collect::<String>(),
            record.raw_memory.chars().take(6_000).collect::<String>()
        );
        let size = entry.chars().count();
        if used + size > MAX_PROMPT_CHARS {
            text.push_str("\n[remaining records omitted to stay within the memory budget]\n");
            break;
        }
        text.push_str(&entry);
        used += size;
    }
    text.push_str(&format!(
        r#"

Return exactly one JSON object:
{{"summary":"markdown summary","entries":[{{"kind":"decision|fact|command|pitfall|preference|task","topic":"decisions|facts|commands|pitfalls|preferences|tasks","text":"one self-contained sentence"}}]}}

Rules:
- Keep only durable, reusable knowledge: architecture, conventions, commands, decisions, pitfalls, user preferences, file locations, and unresolved work.
- Do not store secrets, credentials, tokens, transient status, greetings, or facts obvious from the current code.
- One self-contained sentence per entry, at most 500 characters.
- At most {MAX_ENTRIES} entries. Merge duplicates.
- The summary must be Markdown with the headings: ## Objective, ## Decisions, ## Facts, ## Commands, ## Pitfalls, ## Open threads.
- Keep the summary under {} characters.
"#,
        stark_store::memory_max_chars()
    ));
    text
}

fn parse_model_output(text: &str) -> Result<ModelMemory, String> {
    let start = text.find('{').ok_or_else(|| "no JSON object".to_string())?;
    let end = text
        .rfind('}')
        .ok_or_else(|| "no JSON object".to_string())?;
    if end <= start {
        return Err("empty JSON object".to_string());
    }
    let mut parsed: ModelMemory = serde_json::from_str(&text[start..=end])
        .map_err(|error| format!("invalid JSON: {error}"))?;
    parsed.summary = parsed.summary.trim().to_string();
    let mut seen = HashSet::new();
    parsed.entries.retain(|entry| {
        let text = entry.text.trim().to_lowercase();
        !text.is_empty() && seen.insert(text)
    });
    if parsed.summary.is_empty() && parsed.entries.is_empty() {
        return Err("empty memory".to_string());
    }
    Ok(parsed)
}

fn safe_session(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        .take(40)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fenced_json_and_dedupes_entries() {
        let text = r###"```json
{"summary":"## Objective\nBuild it","entries":[
 {"kind":"decision","topic":"decisions","text":"Use SQLite"},
 {"kind":"decision","topic":"decisions","text":"use sqlite"},
 {"kind":"pitfall","topic":"pitfalls","text":"Refresh before hydration"}
]}
```"###;
        let parsed = parse_model_output(text).unwrap();
        assert_eq!(parsed.entries.len(), 2);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_model_output("not json").is_err());
        assert!(parse_model_output("{}").is_err());
    }
}

#[cfg(test)]
mod model_tests {
    use axum::body::Body;
    use axum::http::header;
    use axum::response::Response;
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::Value;
    use stark_providers::ProviderConfig;
    use stark_store::MemoryStore;
    use tokio_util::sync::CancellationToken;

    use super::*;

    async fn mock_provider(headers: axum::http::HeaderMap, Json(_body): Json<Value>) -> Response {
        assert_eq!(
            headers
                .get("x-opencode-session")
                .and_then(|value| value.to_str().ok()),
            Some("memory-_general")
        );
        let payload = json!({
            "summary": "## Objective\nKeep the parser stable.\n\n## Decisions\n- Use SQLite WAL.",
            "entries": [
                {"kind":"decision","topic":"decisions","text":"Use SQLite WAL for concurrent reads."},
                {"kind":"pitfall","topic":"pitfalls","text":"Refresh runs before hydration."}
            ]
        })
        .to_string();
        let body = format!(
            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            json!({"choices":[{"delta":{"content":payload}}]}),
            json!({"choices":[{"delta":{},"finish_reason":"stop"}]})
        );
        Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(body))
            .unwrap()
    }

    #[tokio::test]
    async fn model_consolidation_writes_entries_index_and_summary() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route("/chat/completions", post(mock_provider)),
            )
            .await
            .unwrap();
        });
        let provider = Provider::new(ProviderConfig {
            mode: "opencode".to_string(),
            base_url: format!("http://{address}"),
            api_key: "test".to_string(),
            model: "mock".to_string(),
        })
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(dir.path()).await.unwrap();
        store
            .record(
                "_general",
                "chat",
                None,
                "Fix the parser",
                "Parser is stable now.",
            )
            .await
            .unwrap();
        let outcome = consolidate_workspace(
            &provider,
            &CancellationToken::new(),
            &store,
            "_general",
            ConsolidateMode::Model,
        )
        .await
        .unwrap();
        assert_eq!(outcome.source, "model");
        assert!(!outcome.fallback);
        assert_eq!(outcome.entries, 2);
        assert_eq!(store.pending_count("_general").await.unwrap(), 0);
        let index = store.index("_general", 8_000).await.unwrap();
        assert!(index.contains("SQLite WAL"), "{index}");
        let summary = store.summary("_general", 8_000).await.unwrap();
        assert!(summary.contains("## Objective"));
    }
}
