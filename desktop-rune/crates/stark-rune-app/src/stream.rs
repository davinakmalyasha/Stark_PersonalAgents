//! Backend run wiring — `POST /api/run` + SSE stream into transcript rows.
//!
//! Mirrors `frontend/src/services/api/stream.ts` (`streamPipeline`):
//! - request body keys: `prompt/model/temperature/workspace_path/workspace_id/
//!   chat_id/attachments/workspace_kind`.
//! - SSE framing: `event:` + `data:` lines, blank-line dispatch, trailing
//!   buffer flush, `delta/state/done/error/warning/turn_aborted` events.
//!
//! The delta fold lives in [`crate::stream_accum`]; this module only routes
//! frames and owns the live row's lifecycle.

use serde_json::Value;

use crate::chats::ChatAttachment;
use crate::rune_transcript::{RowData, RuneTranscriptState};
use crate::stream_accum::{Delta, StreamAccum};

/// POST /api/run body. Field names must match `streamPipeline` exactly.
#[derive(Debug, Clone)]
pub struct RunRequest {
    pub prompt: String,
    pub model: String,
    pub temperature: f64,
    pub workspace_path: String,
    pub workspace_id: String,
    pub chat_id: String,
    pub workspace_kind: String,
    /// Uploaded/restored files (`attachments` in `streamPipeline`).
    pub attachments: Vec<ChatAttachment>,
}

impl RunRequest {
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "prompt": self.prompt,
            "model": self.model,
            "temperature": self.temperature,
            "workspace_path": self.workspace_path,
            "workspace_id": self.workspace_id,
            "chat_id": self.chat_id,
            "attachments": self.attachments.iter().map(ChatAttachment::to_json).collect::<Vec<_>>(),
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
    if !state
        .rows
        .last()
        .is_some_and(|row| row.key == LIVE_ROW_KEY && row.accum.is_some())
    {
        state.rows.push(RowData {
            key: LIVE_ROW_KEY.to_string(),
            role: "agent",
            at: crate::rune_transcript::now_iso(),
            duration: "active".to_string(),
            agent_name: "Stark".to_string(),
            is_streaming: true,
            accum: Some(StreamAccum::default()),
            ..RowData::default()
        });
    }
    let row = state.rows.last_mut().expect("live row just ensured");
    if row.accum.is_none() {
        row.accum = Some(StreamAccum::default());
        row.is_streaming = true;
        row.duration = "active".to_string();
    }
    row
}

fn live_accum(state: &mut RuneTranscriptState) -> &mut StreamAccum {
    live_row(state)
        .accum
        .as_mut()
        .expect("live accumulator ensured")
}

/// Refresh the row's mirrored live-state fields after a fold. The row owns
/// the accumulator; these copies are only for cheap rendering/tests.
fn sync_live(row: &mut RowData) {
    let Some(acc) = &row.accum else { return };
    row.retry = acc.retry.clone();
    row.live_status = acc.status.clone();
    if !acc.agent.is_empty() {
        row.agent_name = acc.agent.clone();
    }
}

/// Apply one frame to the transcript. Returns a short human status for tests.
pub fn apply_frame(state: &mut RuneTranscriptState, frame: &SseFrame) -> &'static str {
    match frame.event.as_str() {
        // Internal frame from the chat-create step. The real id replaces the
        // locally minted one before `/api/run` starts.
        "chat" => {
            let id = frame
                .data
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            if !id.is_empty() {
                state.chat_id = id.to_string();
                state.chat_ready = true;
                // A draft card created from the map rides a `local:*` key;
                // the shell renames it once this real id exists. Harmless for
                // already-real chats (the rekey checks the key prefix).
                let workspace = frame
                    .data
                    .get("workspace_id")
                    .and_then(Value::as_str)
                    .filter(|w| !w.is_empty())
                    .unwrap_or(&state.workspace_id)
                    .to_string();
                state.pending_rekey = Some((workspace, id.to_string()));
                // React refreshes the sidebar right after creating a chat.
                state.refresh_list = true;
            }
            if let Some(workspace) = frame.data.get("workspace_id").and_then(Value::as_str) {
                if !workspace.is_empty() {
                    state.workspace_id = workspace.to_string();
                }
            }
            "chat"
        }
        "delta" => {
            // Late frames after the terminal event must not resurrect a
            // finished live row (React's `settled` runs no further UI work).
            if !state.live {
                return "ignored";
            }
            let kind = frame.data.get("kind").and_then(Value::as_str).unwrap_or("");
            if kind == "usage" {
                let u64_of = |key: &str| frame.data.get(key).and_then(Value::as_u64).unwrap_or(0);
                let input = u64_of("input");
                let output = u64_of("output");
                let hits = {
                    let hits = u64_of("cached_tokens");
                    if hits > 0 {
                        hits
                    } else {
                        u64_of("cached")
                    }
                };
                let cost = frame
                    .data
                    .get("cost")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                {
                    let acc = live_accum(state);
                    acc.usage.add(input, output, hits, cost);
                }
                let u = &mut state.usage;
                u.input += input;
                u.output += output;
                u.cache_hit += hits;
                u.cost += cost;
                u.active = input;
                u.calls.push(crate::rune_transcript::UsageCall {
                    agent: "chat".to_string(),
                    model: state.model.clone(),
                    input,
                    output,
                    hits,
                    misses: input.saturating_sub(hits),
                    cost,
                    at: crate::rune_transcript::now_iso(),
                });
                state.live = true;
                if let Some(row) = state.rows.last_mut() {
                    sync_live(row);
                }
                return "metered";
            }
            let delta = Delta::from_value(&frame.data);
            {
                let acc = live_accum(state);
                let mut rate = std::mem::take(&mut acc.rate);
                acc.apply(&mut rate, &delta, crate::clock::now_millis());
                acc.rate = rate;
            }
            if let Some(row) = state.rows.last_mut() {
                sync_live(row);
            }
            state.live = true;
            match kind {
                "text" | "reasoning" => "streaming",
                "tool_started" | "tool_finished" => "tool",
                "status" => "status",
                "end" => "end",
                _ => "ignored",
            }
        }
        "state" => {
            if !state.live {
                return "ignored";
            }
            {
                let acc = live_accum(state);
                if let Some(report) = frame
                    .data
                    .get("outputs")
                    .and_then(|v| v.get("agent_report"))
                    .and_then(Value::as_str)
                {
                    acc.report = report.to_string();
                }
                if let Some(usage) = frame.data.get("token_usage") {
                    acc.usage_json = Some(usage.clone());
                    acc.usage = crate::stream_accum::RunUsage::from_json(usage);
                }
                if let Some(artifacts) = frame.data.get("artifacts") {
                    acc.artifacts = Some(artifacts.clone());
                }
                if let Some(error) = frame.data.get("error").and_then(Value::as_str) {
                    acc.state_error = Some(error.to_string());
                }
            }
            if let Some(error) = frame.data.get("error").and_then(Value::as_str) {
                crate::rune_transcript::finalize_run(state, Some(error.to_string()), false);
                return "failed";
            }
            if let Some(row) = state.rows.last_mut() {
                sync_live(row);
            }
            state.live = true;
            "state"
        }
        "done" => {
            // Abort already persisted the partial; wait for the backend's
            // marker, then refresh history once.
            if state.aborting {
                state.live = false;
                state.aborting = false;
                state.refresh_chat = Some((state.workspace_id.clone(), state.chat_id.clone()));
                return "done";
            }
            if frame.data.get("status").and_then(Value::as_str) == Some("error") {
                let message = frame
                    .data
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("The agent pipeline failed.")
                    .to_string();
                crate::rune_transcript::finalize_run(state, Some(message), false);
                return "failed";
            }
            crate::rune_transcript::finalize_run(state, None, false);
            "done"
        }
        "error" => {
            if state.aborting {
                state.live = false;
                state.aborting = false;
                state.refresh_chat = Some((state.workspace_id.clone(), state.chat_id.clone()));
                return "failed";
            }
            let msg = frame
                .data
                .get("message")
                .or_else(|| frame.data.get("error"))
                .and_then(Value::as_str)
                .unwrap_or("The agent pipeline failed.");
            // A locked server (HTTP 401) is not a dead backend: it wants
            // pairing (`PairingGate.tsx`, same status).
            let msg = if crate::transport::is_locked_error(msg) {
                "Server locked (401): approve this device from the desktop host, then retry."
            } else {
                msg
            };
            crate::rune_transcript::finalize_run(state, Some(msg.to_string()), false);
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
        .map(|frame| apply_frame(state, frame))
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
            workspace_id: "_general".to_string(),
            chat_id: "c1".to_string(),
            workspace_kind: "general".to_string(),
            attachments: Vec::new(),
        };
        let body = req.to_json();
        for key in [
            "prompt",
            "model",
            "temperature",
            "workspace_path",
            "workspace_id",
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
            "event: chat\ndata: {\"id\":\"real-chat\"}\n\nevent: delta\ndata: {\"kind\":\"text\",\"text\":\"42\"}\n\nevent: done\ndata: {\"status\":\"ok\"}\n\n",
        );
        assert_eq!(statuses, vec!["chat", "streaming", "done"]);
        assert_eq!(s.rows.len(), n + 1);
        assert_eq!(s.rows.last().unwrap().body, "42");
        assert!(!s.live);
        assert!(s.rows.last().unwrap().final_text == "42");
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
            "event: delta\ndata: {\"kind\":\"text\",\"text\":\"partial\"}\n\nevent: done\ndata: {\"status\":\"error\",\"error\":\"pipeline blew up\"}\n\n",
        );
        assert_eq!(statuses, vec!["streaming", "failed"]);
        assert!(!s.live);
        let last = s.rows.last().unwrap();
        assert_eq!(last.role, "error");
        assert_eq!(last.body, "pipeline blew up");
    }

    #[test]
    fn completion_freezes_timeline_and_queues_metadata() {
        let mut s = state();
        s.run_started_ms = Some(crate::clock::monotonic_millis() - 1250.0);
        s.live = true;
        let statuses = replay(
            &mut s,
            "event: delta\ndata: {\"kind\":\"reasoning\",\"text\":\"think\"}\n\nevent: delta\ndata: {\"kind\":\"tool_started\",\"tool\":\"grep\",\"args\":\"{\\\"pattern\\\":\\\"x\\\"}\"}\n\nevent: delta\ndata: {\"kind\":\"tool_finished\",\"tool\":\"grep\",\"ok\":true,\"ms\":4,\"text\":\"found\"}\n\nevent: delta\ndata: {\"kind\":\"text\",\"text\":\"answer\"}\n\nevent: delta\ndata: {\"kind\":\"usage\",\"input\":10,\"output\":2}\n\nevent: done\ndata: {\"status\":\"complete\"}\n\n",
        );
        assert_eq!(statuses.last(), Some(&"done"));
        let row = s.rows.last().unwrap();
        assert_eq!(row.body, "answer");
        assert_eq!(row.activity.len(), 2);
        assert_eq!(row.duration, "1s");
        assert!(row.show_trace);
        assert!(row.has_usage);
        assert_eq!(row.tokens_in, 10);
        assert_eq!(row.tokens_out, 2);
        assert_eq!(s.pending_saves.len(), 1);
        let save = &s.pending_saves[0];
        assert_eq!(save.role, "agent");
        assert_eq!(save.metadata["duration"], serde_json::json!("1s"));
        assert_eq!(save.metadata["thinking"], serde_json::json!("think"));
        assert_eq!(
            save.metadata["liveTimeline"].as_array().map(Vec::len),
            Some(3)
        );
        assert_eq!(save.metadata["usage"]["total_input_tokens"], 10);
    }

    #[test]
    fn persisted_round_trip_matches_the_live_presentation() {
        let mut s = state();
        s.run_started_ms = Some(crate::clock::monotonic_millis() - 800.0);
        s.live = true;
        replay(
            &mut s,
            "event: delta\ndata: {\"kind\":\"reasoning\",\"text\":\"plan\"}\n\nevent: delta\ndata: {\"kind\":\"tool_started\",\"tool\":\"read\",\"args\":\"{\\\"path\\\":\\\"a.rs\\\"}\"}\n\nevent: delta\ndata: {\"kind\":\"tool_finished\",\"tool\":\"read\",\"ok\":true,\"ms\":3,\"text\":\"ok\"}\n\nevent: delta\ndata: {\"kind\":\"text\",\"text\":\"done\"}\n\nevent: done\ndata: {\"status\":\"complete\"}\n\n",
        );
        let live = s.rows.last().unwrap().clone();
        let save = s.pending_saves[0].clone();
        let payload = serde_json::json!({"messages": [{
            "id": "persisted",
            "role": "assistant",
            "content": save.content,
            "timestamp": "2026-11-12T12:00:00Z",
            "metadata": save.metadata
        }]});
        let parsed = crate::chats::parse_messages(&payload);
        assert_eq!(parsed[0].body, live.body);
        assert_eq!(parsed[0].final_text, live.final_text);
        assert_eq!(parsed[0].activity, live.activity);
        assert_eq!(parsed[0].duration, live.duration);
        assert_eq!(parsed[0].tokens_in, live.tokens_in);
        assert_eq!(parsed[0].tokens_out, live.tokens_out);
    }

    #[test]
    fn error_after_partial_persists_agent_then_error() {
        let mut s = state();
        s.live = true;
        let statuses = replay(
            &mut s,
            "event: delta\ndata: {\"kind\":\"text\",\"text\":\"partial\"}\n\nevent: error\ndata: {\"message\":\"boom\"}\n\n",
        );
        assert_eq!(statuses.last(), Some(&"failed"));
        assert_eq!(s.rows.last().unwrap().role, "error");
        assert_eq!(s.rows.last().unwrap().body, "boom");
        assert_eq!(s.pending_saves.len(), 2);
        assert_eq!(s.pending_saves[0].role, "agent");
        assert_eq!(s.pending_saves[0].metadata["stopped"], true);
        assert_eq!(s.pending_saves[1].role, "error");
    }

    #[test]
    fn usage_frames_meter_the_orb() {
        let mut s = state();
        let statuses = replay(
            &mut s,
            "event: delta\ndata: {\"kind\":\"usage\",\"input\":200000,\"output\":50,\"cached_tokens\":1000,\"cost\":0.002}\n\n",
        );
        assert_eq!(statuses, vec!["metered"]);
        assert_eq!(s.usage.input, 200000);
        assert_eq!(s.usage.output, 50);
        assert_eq!(s.usage.cache_hit, 1000);
        assert!((s.usage.cost - 0.002).abs() < 1e-9);
        assert_eq!(s.usage.active, 200000);
        // 200k/1M = 20%.
        assert!((s.usage.context_percent() - 20.0).abs() < 1e-6);
    }

    #[test]
    fn locked_server_reports_pairing_not_transport_noise() {
        let mut s = state();
        s.live = true;
        let statuses = replay(
            &mut s,
            "event: error\ndata: {\"message\":\"HTTP HTTP/1.1 401 Unauthorized\"}\n\n",
        );
        assert_eq!(statuses, vec!["failed"]);
        assert!(!s.live);
        let body = &s.rows.last().unwrap().body;
        assert!(
            body.contains("locked") && body.contains("401"),
            "unexpected body: {body}"
        );
    }

    #[test]
    fn stage_abort_drops_live_but_keeps_inbox() {
        use crate::rune_transcript::stage_abort;
        let mut s = state();
        s.rows.push(RowData {
            key: LIVE_ROW_KEY.to_string(),
            role: "agent",
            is_streaming: true,
            accum: Some(StreamAccum::default()),
            ..RowData::default()
        });
        s.live = true;
        stage_abort(&mut s);
        assert!(!s.live);
        assert!(s.aborting);
        // Idempotent: a dead run stays dead.
        stage_abort(&mut s);
        assert!(!s.live);
    }
}
