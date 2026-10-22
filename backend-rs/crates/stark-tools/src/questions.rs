use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use uuid::Uuid;

struct Pending {
    questions: Vec<Value>,
    created: i64,
    sender: oneshot::Sender<Vec<Value>>,
}

#[derive(Clone, Default)]
pub struct QuestionBroker {
    pending: Arc<DashMap<String, Pending>>,
}

impl QuestionBroker {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn ask(
        &self,
        questions: Vec<Value>,
        timeout_secs: u64,
    ) -> Result<Vec<Value>, String> {
        let batch_id = Uuid::new_v4().to_string();
        let (sender, receiver) = oneshot::channel();
        self.pending.insert(
            batch_id.clone(),
            Pending {
                questions,
                created: chrono::Utc::now().timestamp_millis(),
                sender,
            },
        );

        let result = tokio::time::timeout(Duration::from_secs(timeout_secs), receiver).await;
        self.pending.remove(&batch_id);
        match result {
            Ok(Ok(answers)) => Ok(answers),
            Ok(Err(_)) => Err("question batch was cancelled".to_string()),
            Err(_) => Err(format!("timed out after {timeout_secs}s")),
        }
    }

    pub fn pending_batches(&self) -> Vec<Value> {
        let mut out: Vec<Value> = self
            .pending
            .iter()
            .map(|entry| {
                json!({
                    "batch_id": entry.key(),
                    "questions": entry.value().questions,
                    "created": entry.value().created,
                })
            })
            .collect();
        out.sort_by_key(|batch| batch["created"].as_i64().unwrap_or(0));
        out
    }

    pub fn answer(&self, batch_id: &str, answers: Vec<Value>) -> bool {
        let Some((_, pending)) = self.pending.remove(batch_id) else {
            return false;
        };
        pending.sender.send(answers).is_ok()
    }
}
