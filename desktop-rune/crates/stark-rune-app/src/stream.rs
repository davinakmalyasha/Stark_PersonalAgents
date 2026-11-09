//! Backend run wiring — `POST /api/run` + SSE stream into transcript rows.
//!
//! Mirrors `frontend/src/services/api/stream.ts` (`streamPipeline`):
//! - request body keys: `prompt/model/temperature/workspace_path/chat_id/
//!   attachments/workspace_kind`.
//! - SSE framing: `event:` + `data:` lines, blank-line dispatch, trailing
//!   buffer flush, `delta/state/done/error/warning/turn_aborted` events.
//!
//! Transport (HTTP) stays out: this module parses bytes and mutates
//! `RuneTranscriptState`, so headless tests replay recorded streams
//! deterministically. Pure `std` + `serde_json`, no async runtime.

use serde_json::Value;

use crate::rune_transcript::{RowData, RuneTranscriptState};

/// POST /api/run body. Field names must match `streamPipeline` exactly.
#[derive(Debug, Clone)]
pub struct RunRequest {
    pub prompt: String,
    pub model: String,
    pub temperature: f64,
    pub workspace_path: String,
    pub chat_id: String,
    pub workspace_kind: String,
}

impl RunRequest {
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "prompt": self.prompt,
            "model": self.model,
            "temperature": self.temperature,
            "workspace_path": self.workspace_path,
            "chat_id": self.chat_id,
            "attachments": [],
            "workspace_kind": self.workspace_kind,
        })
    }
}

/// One dispatched SSE frame.
#[derive(Debug, Clone, PartialEq)]
pub struct SseFrame {
    pub event: String,
    pub data: Value,
}

/// Parse an SSE byte stream into frames. Tolerates a missing trailing blank
/// line (flushes the buffer, like `streamPipeline`'s `done` path) and skips
/// undecodable `data:` lines. Implemented over [`crate::transport::SseDecoder`]
/// so streaming and one-shot parsing share framing rules.
pub fn parse_sse_stream(text: &str) -> Vec<SseFrame> {
    let mut decoder = crate::transport::SseDecoder::new();
    let mut out = Vec::new();
    decoder.push(text.as_bytes(), &mut out);
    decoder.finish(&mut out);
    out
}

/// Frames without an `event:` line: `done`/`error` payloads and `AppState`
/// snapshots (anything with `active_node`/`state_version`) are classified.
pub(crate) fn infer_event(data: &Value) -> String {
    if data.get("status").is_some() {
        "done".to_string()
    } else if data.get("error").is_some() && data.get("active_node").is_none() {
        "error".to_string()
    } else {
        "state".to_string()
    }
}

/// Streaming agent row key (mirrors `live-agent-run` in `App.tsx`).
pub const LIVE_ROW_KEY: &str = "live-agent";

fn live_row(state: &mut RuneTranscriptState) -> &mut RowData {
    if !state.rows.last().is_some_and(|r| r.key == LIVE_ROW_KEY) {
        state.rows.push(RowData {
            key: LIVE_ROW_KEY.to_string(),
            role: "agent",
            body: String::new(),
        });
    }
    state.rows.last_mut().expect("live row just pushed")
}

/// Apply one frame to the transcript. Returns a short human status for tests.
pub fn apply_frame(state: &mut RuneTranscriptState, frame: &SseFrame) -> &'static str {
    match frame.event.as_str() {
        "delta" => {
            let kind = frame.data.get("kind").and_then(Value::as_str).unwrap_or("");
            let text = frame.data.get("text").and_then(Value::as_str).unwrap_or("");
            match kind {
                "text" | "reasoning" => {
                    live_row(state).body.push_str(text);
                    state.live = true;
                    "streaming"
                }
                "tool_started" => {
                    let tool = frame
                        .data
                        .get("tool")
                        .and_then(Value::as_str)
                        .unwrap_or("tool");
                    state.rows.push(RowData {
                        key: format!("tool-{}-{}", state.rows.len(), tool),
                        role: "system",
                        body: format!("running {tool}…"),
                    });
                    "tool"
                }
                "tool_finished" => {
                    let tool = frame
                        .data
                        .get("tool")
                        .and_then(Value::as_str)
                        .unwrap_or("tool");
                    let ok = frame
                        .data
                        .get("ok")
                        .and_then(Value::as_bool)
                        .unwrap_or(true);
                    state.rows.push(RowData {
                        key: format!("tool-{}-{}", state.rows.len(), tool),
                        role: if ok { "system" } else { "error" },
                        body: if ok {
                            format!("{tool} done")
                        } else {
                            format!("{tool} failed")
                        },
                    });
                    "tool"
                }
                _ => "ignored",
            }
        }
        "state" => {
            if let Some(err) = frame.data.get("error").and_then(Value::as_str) {
                live_row(state).body = format!("Run failed: {err}");
                state.live = false;
                return "failed";
            }
            state.live = true;
            "state"
        }
        "done" => {
            // Mirrors `streamPipeline`: `status === 'error'` fails the run
            // instead of landing it clean.
            if frame.data.get("status").and_then(Value::as_str) == Some("error") {
                let msg = frame
                    .data
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("The agent pipeline failed.");
                live_row(state).body = msg.to_string();
                live_row(state).role = "error";
                state.live = false;
                return "failed";
            }
            state.live = false;
            "done"
        }
        "error" => {
            let msg = frame
                .data
                .get("message")
                .or_else(|| frame.data.get("error"))
                .and_then(Value::as_str)
                .unwrap_or("The agent pipeline failed.");
            live_row(state).body = msg.to_string();
            live_row(state).role = "error";
            state.live = false;
            "failed"
        }
        "warning" | "turn_aborted" => "noted",
        _ => "ignored",
    }
}

/// Replay a whole recorded stream. Returns the per-frame statuses.
pub fn replay(state: &mut RuneTranscriptState, text: &str) -> Vec<&'static str> {
    parse_sse_stream(text)
        .iter()
        .map(|f| apply_frame(state, f))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> RuneTranscriptState {
        RuneTranscriptState::screenshot()
    }

    #[test]
    fn request_body_matches_stream_pipeline() {
        let req = RunRequest {
            prompt: "hi".to_string(),
            model: "Automatic Fallback".to_string(),
            temperature: 0.7,
            workspace_path: "D:/x".to_string(),
            chat_id: "c1".to_string(),
            workspace_kind: "general".to_string(),
        };
        let body = req.to_json();
        for key in [
            "prompt",
            "model",
            "temperature",
            "workspace_path",
            "chat_id",
            "attachments",
            "workspace_kind",
        ] {
            assert!(body.get(key).is_some(), "missing {key}");
        }
        assert_eq!(body["temperature"], serde_json::json!(0.7));
    }

    #[test]
    fn sse_framing_handles_events_and_trailing_buffer() {
        let stream = "event: delta\ndata: {\"kind\":\"text\",\"text\":\"hel\"}\n\nevent: delta\ndata: {\"kind\":\"text\",\"text\":\"lo\"}\n\nevent: done\ndata: {\"status\":\"ok\"}";
        let frames = parse_sse_stream(stream);
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].event, "delta");
        assert_eq!(frames[2].event, "done");
    }

    #[test]
    fn replay_streams_tokens_then_lands() {
        let mut s = state();
        let n = s.rows.len();
        let statuses = replay(
            &mut s,
            "event: delta\ndata: {\"kind\":\"text\",\"text\":\"42\"}\n\nevent: delta\ndata: {\"kind\":\"reasoning\",\"text\":\"…\"}\n\nevent: done\ndata: {\"status\":\"ok\"}\n\n",
        );
        assert_eq!(statuses, vec!["streaming", "streaming", "done"]);
        assert_eq!(s.rows.len(), n + 1);
        assert_eq!(s.rows.last().unwrap().body, "42…");
        assert!(!s.live);
    }

    #[test]
    fn replay_error_marks_live_row_failed() {
        let mut s = state();
        s.live = true;
        let statuses = replay(&mut s, "event: error\ndata: {\"message\":\"boom\"}\n\n");
        assert_eq!(statuses, vec!["failed"]);
        assert!(!s.live);
        assert_eq!(s.rows.last().unwrap().role, "error");
    }

    #[test]
    fn replay_done_error_fails_like_stream_pipeline() {
        let mut s = state();
        s.live = true;
        let statuses = replay(
            &mut s,
            "event: done\ndata: {\"status\":\"error\",\"error\":\"pipeline blew up\"}\n\n",
        );
        assert_eq!(statuses, vec!["failed"]);
        assert!(!s.live);
        assert_eq!(s.rows.last().unwrap().role, "error");
        assert_eq!(s.rows.last().unwrap().body, "pipeline blew up");
    }
}
