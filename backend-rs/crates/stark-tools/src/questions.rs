use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use uuid::Uuid;

struct Pending {
    chat_id: String,
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

    /// Ask a question batch on behalf of `chat_id`. Voice and UI clients use
    /// the chat id from `pending_batches` to route the batch to the right
    /// session (e.g. the voice overlay answers only its own chat's batch).
    pub async fn ask(
        &self,
        chat_id: &str,
        questions: Vec<Value>,
        timeout_secs: u64,
    ) -> Result<Vec<Value>, String> {
        let batch_id = Uuid::new_v4().to_string();
        let (sender, receiver) = oneshot::channel();
        self.pending.insert(
            batch_id.clone(),
            Pending {
                chat_id: chat_id.to_string(),
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
                    "chat_id": entry.value().chat_id,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn ask_roundtrip_carries_chat_id() {
        let broker = QuestionBroker::new();
        let broker2 = broker.clone();
        let handle = tokio::spawn(async move {
            broker2
                .ask("chat-1", vec![json!({"id": "q1", "question": "Go?"})], 30)
                .await
        });
        // Let the ask register before inspecting pending batches.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let batches = broker.pending_batches();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0]["chat_id"], json!("chat-1"));
        assert_eq!(batches[0]["questions"][0]["id"], json!("q1"));
        let batch_id = batches[0]["batch_id"].as_str().unwrap().to_string();
        assert!(broker.answer(&batch_id, vec![json!({"id": "q1", "custom": "yes"})]));
        let answers = handle.await.unwrap().expect("ask resolves");
        assert_eq!(answers[0]["custom"], json!("yes"));
        assert!(broker.pending_batches().is_empty());
    }

    #[tokio::test]
    async fn ask_times_out_and_clears_pending() {
        let broker = QuestionBroker::new();
        let result = broker.ask("chat-9", vec![json!({"id": "q"})], 1).await;
        assert!(result.is_err());
        assert!(broker.pending_batches().is_empty());
    }
}
