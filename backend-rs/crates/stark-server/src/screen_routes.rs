use std::path::Path;

use axum::Json;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::ServerState;

#[derive(Debug, Deserialize)]
pub struct VisionFrameRequest {
    pub image_b64: String,
    #[serde(default = "default_frame_mime")]
    pub mime: String,
    #[serde(default)]
    pub chat_id: String,
    #[serde(default = "default_frame_label")]
    pub label: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub req_id: String,
}

fn default_frame_mime() -> String {
    "image/jpeg".to_string()
}

fn default_frame_label() -> String {
    "turn".to_string()
}

#[derive(Debug, Serialize)]
struct FrameAttachment {
    id: String,
    name: String,
    #[serde(rename = "type")]
    mime: String,
    size: usize,
    path: String,
    #[serde(rename = "isImage")]
    is_image: bool,
    #[serde(rename = "screenFrame")]
    screen_frame: bool,
    note: String,
}

pub async fn vision_frame_handler(
    State(state): State<ServerState>,
    Json(request): Json<VisionFrameRequest>,
) -> Response {
    const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
    let Some(ext) = frame_extension(&request.mime) else {
        return error_json(
            StatusCode::BAD_REQUEST,
            &format!("Unsupported mime '{}'", request.mime),
        );
    };
    let raw = match base64::engine::general_purpose::STANDARD.decode(&request.image_b64) {
        Ok(raw) if !raw.is_empty() => raw,
        Ok(_) => return error_json(StatusCode::BAD_REQUEST, "empty frame"),
        Err(_) => return error_json(StatusCode::BAD_REQUEST, "image_b64 is not valid base64"),
    };
    if raw.len() > MAX_FRAME_BYTES {
        return error_json(StatusCode::BAD_REQUEST, "frame larger than 8 MB");
    }
    let folder = state
        .harness_root
        .join(".deep_agents")
        .join("live_screen")
        .join(safe_frame_name(&request.chat_id));
    let req_id = request.req_id.clone();
    let result =
        tokio::task::spawn_blocking(move || save_frame(&folder, &raw, ext, &request)).await;
    let attachment = match result {
        Ok(Ok(attachment)) => attachment,
        Ok(Err(error)) => {
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("failed to store frame: {error:#}"),
            );
        }
        Err(error) => {
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("frame task failed: {error}"),
            );
        }
    };
    stark_tools::screen_bridge().set_latest_frame(attachment.path.clone());
    if !req_id.is_empty() {
        stark_tools::screen_bridge().resolve_frame(
            &req_id,
            serde_json::to_value(&attachment).unwrap_or(Value::Null),
        );
    }
    Json(json!({"ok": true, "attachment": attachment})).into_response()
}

fn frame_extension(mime: &str) -> Option<&'static str> {
    match mime.trim().to_ascii_lowercase().as_str() {
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/webp" => Some("webp"),
        _ => None,
    }
}

fn safe_frame_name(value: &str) -> String {
    let value: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    if value.is_empty() {
        "_default".to_string()
    } else {
        value
    }
}

fn save_frame(
    folder: &Path,
    raw: &[u8],
    ext: &str,
    request: &VisionFrameRequest,
) -> anyhow::Result<FrameAttachment> {
    std::fs::create_dir_all(folder)?;
    let mime = match ext {
        "png" => "image/png",
        "webp" => "image/webp",
        _ => "image/jpeg",
    };
    let (path, name) = if request.label == "continuous" {
        let path = folder.join(format!("latest.{ext}"));
        atomic_write(&path, raw)?;
        (path, format!("latest.{ext}"))
    } else {
        let frames = folder.join("frames");
        std::fs::create_dir_all(&frames)?;
        let name = format!(
            "{}_{}.{}",
            chrono::Utc::now().timestamp_millis(),
            &Uuid::new_v4().to_string()[..4],
            ext
        );
        let path = frames.join(&name);
        std::fs::write(&path, raw)?;
        atomic_write(&folder.join(format!("latest.{ext}")), raw)?;
        (path, name)
    };
    let mut note = match request.label.as_str() {
        "continuous" => "Live screen (continuous share) — the user's current view".to_string(),
        "tool" => "Live screen — captured on request".to_string(),
        _ => "Live screen — what the user was viewing when they spoke".to_string(),
    };
    if !request.source.is_empty() {
        note.push_str(&format!(" (source: {})", request.source));
    }
    Ok(FrameAttachment {
        id: name.clone(),
        name: format!("screen_{name}"),
        mime: mime.to_string(),
        size: raw.len(),
        path: path.to_string_lossy().into_owned(),
        is_image: true,
        screen_frame: true,
        note,
    })
}

fn atomic_write(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let temp = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|e| e.to_str()).unwrap_or("img")
    ));
    std::fs::write(&temp, bytes)?;
    std::fs::rename(&temp, path)?;
    Ok(())
}

fn error_json(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"ok": false, "error": message}))).into_response()
}

pub async fn voice_ws_handler(State(state): State<ServerState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| voice_ws(state, socket))
}

async fn voice_ws(state: ServerState, socket: WebSocket) {
    let (mut sender, mut receiver) = socket.split();
    let (outbox_tx, mut outbox_rx) = mpsc::unbounded_channel::<Value>();
    let screen = stark_tools::screen_bridge();
    screen.attach(outbox_tx.clone());
    let capture = state.voice.as_ref().and_then(|v| v.capture.clone());
    let stt = state.voice.as_ref().and_then(|v| v.stt.clone());
    let tts = state.voice.as_ref().map(|v| v.tts.clone());

    let send_task = tokio::spawn(async move {
        while let Some(value) = outbox_rx.recv().await {
            if sender
                .send(Message::Text(value.to_string().into()))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    let (live_tx, mut live_rx) = mpsc::unbounded_channel::<stark_voice::LiveEvent>();
    let live_out = outbox_tx.clone();
    let live_forward = tokio::spawn(async move {
        while let Some(event) = live_rx.recv().await {
            if live_out.send(live_event_json(event)).is_err() {
                break;
            }
        }
    });

    let mut live_session: Option<stark_voice::LiveSession> = None;

    while let Some(message) = receiver.next().await {
        let Ok(message) = message else {
            break;
        };
        match message {
            Message::Text(text) => {
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                match value.get("type").and_then(Value::as_str).unwrap_or("") {
                    "session.start" => {
                        let Some(stt) = stt.clone() else {
                            let _ = outbox_tx.send(json!({
                                "type": "session.error",
                                "error": "Moonshine STT is not available in this build",
                            }));
                            continue;
                        };
                        if let Some(mut old) = live_session.take() {
                            old.stop();
                        }
                        match stark_voice::LiveSession::start(stt, live_tx.clone()) {
                            Ok(session) => {
                                let _ = outbox_tx.send(json!({
                                    "type": "session.ready",
                                    "model": session.model(),
                                }));
                                live_session = Some(session);
                            }
                            Err(error) => {
                                let _ = outbox_tx.send(json!({
                                    "type": "session.error",
                                    "error": format!("{error:#}"),
                                }));
                            }
                        }
                    }
                    "session.stop" => {
                        if let Some(mut session) = live_session.take() {
                            session.stop();
                        }
                        let _ = outbox_tx.send(json!({"type": "session.stopped"}));
                    }
                    "screen-ack" => screen.resolve_ack(&value),
                    "screen-state" => screen.update_state(&value),
                    "interrupt" => {
                        let played = value
                            .get("played_ms")
                            .and_then(Value::as_i64)
                            .unwrap_or(0)
                            .max(0);
                        if let Some(tts) = &tts {
                            tts.cancel_current();
                        }
                        let _ = outbox_tx.send(json!({
                            "type": "interrupted",
                            "played_ms": played,
                        }));
                    }
                    "cancel-tts" => {
                        if let Some(tts) = &tts {
                            tts.cancel_current();
                        }
                    }
                    "send" => {
                        let Some(capture) = capture.clone() else {
                            let _ = outbox_tx.send(json!({
                                "type": "final",
                                "text": "",
                                "error": "Moonshine STT is not available in this build",
                            }));
                            continue;
                        };
                        let text = tokio::task::spawn_blocking(move || capture.flush_round())
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .unwrap_or_default();
                        let _ = outbox_tx.send(json!({
                            "type": "final",
                            "text": text,
                            "engine": "moonshine",
                        }));
                    }
                    "ping" => {
                        let _ = outbox_tx.send(json!({"type": "pong"}));
                    }
                    _ => {}
                }
            }
            Message::Binary(bytes) => {
                if let Some(session) = live_session.as_ref() {
                    session.feed_pcm16(&bytes);
                }
            }
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) => {}
        }
    }

    if let Some(mut session) = live_session.take() {
        session.stop();
    }
    drop(outbox_tx);
    drop(live_tx);
    live_forward.abort();
    send_task.abort();
    screen.detach();
}

fn live_event_json(event: stark_voice::LiveEvent) -> Value {
    match event {
        stark_voice::LiveEvent::SpeechStarted { turn_id, level } => {
            json!({"type": "speech_started", "turn_id": turn_id, "level": level})
        }
        stark_voice::LiveEvent::Partial { turn_id, text } => {
            json!({"type": "transcript", "turn_id": turn_id, "text": text, "final": false})
        }
        stark_voice::LiveEvent::TurnFinal {
            turn_id,
            text,
            duration_ms,
        } => json!({
            "type": "turn_final",
            "turn_id": turn_id,
            "text": text,
            "duration_ms": duration_ms,
        }),
        stark_voice::LiveEvent::Error { message } => {
            json!({"type": "session.error", "error": message})
        }
    }
}
