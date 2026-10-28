use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::{Multipart, State};
use axum::http::{StatusCode, header};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use stark_voice::{CaptureManager, SttService, TtsEvent, TtsService};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::ServerState;

const MAX_TTS_TEXT: usize = 4000;
const TTS_CACHE_MAX_AGE_SECS: u64 = 7 * 24 * 3600;
const TTS_CACHE_MAX_FILES: usize = 200;

pub struct VoiceRuntime {
    pub tts: Arc<TtsService>,
    pub stt: Option<Arc<SttService>>,
    pub capture: Option<Arc<CaptureManager>>,
}

impl VoiceRuntime {
    pub fn new(models_dir: PathBuf) -> Self {
        let tts = Arc::new(TtsService::new(models_dir.clone()));
        let stt = match SttService::load(&models_dir) {
            Ok(stt) => {
                tracing::info!("Moonshine STT model: {}", stt.model_name());
                let stt = Arc::new(stt);
                Some(stt)
            }
            Err(error) => {
                tracing::warn!("Moonshine STT unavailable: {error:#}");
                None
            }
        };
        let capture = stt.clone().map(CaptureManager::new).map(Arc::new);
        Self { tts, stt, capture }
    }
}

#[derive(Debug, Deserialize)]
pub struct TtsRequest {
    pub text: String,
    #[serde(default = "default_voice")]
    pub voice: String,
    #[serde(default = "default_format")]
    pub format: String,
}

fn default_voice() -> String {
    "jarvis".to_string()
}

fn default_format() -> String {
    "wav".to_string()
}

pub async fn tts_handler(
    State(state): State<ServerState>,
    Json(request): Json<TtsRequest>,
) -> Response {
    let Some(voice) = state.voice.as_ref() else {
        return unavailable("voice models are not installed");
    };
    let text = request.text.trim();
    if text.is_empty() {
        return error_json(StatusCode::BAD_REQUEST, "text is required");
    }
    if text.len() > MAX_TTS_TEXT {
        return error_json(StatusCode::BAD_REQUEST, "text is too long");
    }
    let voice_name = request.voice.trim().to_string();
    let cache = tts_cache_path(&state.harness_root, text, &voice_name);

    if let Some(bytes) = read_cache(&cache).await {
        return audio_response(bytes);
    }

    let result = voice
        .tts
        .synthesize_wav(text.to_string(), voice_name.clone())
        .await;
    match result {
        Ok(bytes) => {
            write_cache(cache, bytes.clone()).await;
            audio_response(bytes)
        }
        Err(error) => unavailable(&format!("TTS unavailable: {error:#}")),
    }
}

pub async fn tts_stream_handler(
    State(state): State<ServerState>,
    Json(request): Json<TtsRequest>,
) -> Response {
    let Some(voice) = state.voice.as_ref() else {
        return unavailable("voice models are not installed");
    };
    let text = request.text.trim().to_string();
    if text.is_empty() {
        return error_json(StatusCode::BAD_REQUEST, "text is required");
    }
    if text.len() > MAX_TTS_TEXT {
        return error_json(StatusCode::BAD_REQUEST, "text is too long");
    }
    let voice_name = request.voice.trim().to_string();
    let want_pcm16 = matches!(
        request.format.trim().to_ascii_lowercase().as_str(),
        "pcm16" | "pcm" | "raw"
    );
    let cache = tts_cache_path(&state.harness_root, &text, &voice_name);

    if let Some(bytes) = read_cache(&cache).await {
        let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(16);
        tokio::spawn(async move {
            if let Err(error) = send_cached_sse(tx, bytes, want_pcm16).await {
                tracing::warn!("cached TTS SSE failed: {error:#}");
            }
        });
        return Sse::new(ReceiverStream::new(rx)).into_response();
    }

    if let Err(error) = voice.tts.warm().await {
        return unavailable(&format!("TTS unavailable: {error:#}"));
    }
    let stream = match voice.tts.start_stream(text.clone(), voice_name.clone()) {
        Ok(stream) => stream,
        Err(error) => return unavailable(&format!("TTS unavailable: {error:#}")),
    };
    let sr = stream.sample_rate;
    let cancel = stream.cancel.clone();
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(32);
    tokio::spawn(async move {
        let mut stream = stream;
        let mut pending: Vec<f32> = Vec::new();
        let mut cache_parts: Vec<f32> = Vec::new();
        let mut finished_cleanly = false;
        while let Some(event) = stream.rx.recv().await {
            match event {
                TtsEvent::Samples(chunk) => {
                    pending.extend_from_slice(&chunk);
                    cache_parts.extend_from_slice(&chunk);
                    while pending.len() >= 1920 {
                        let rest = pending.split_off(1920);
                        let chunk = std::mem::replace(&mut pending, rest);
                        if send_pcm_sse(&tx, &chunk, sr, want_pcm16).await.is_err() {
                            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                            return;
                        }
                    }
                }
                TtsEvent::Done { cancelled } => {
                    if !pending.is_empty()
                        && send_pcm_sse(&tx, &pending, sr, want_pcm16).await.is_err()
                    {
                        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                        return;
                    }
                    let _ = tx
                        .send(Ok(Event::default().data(
                            json!({"done": true, "cancelled": cancelled}).to_string(),
                        )))
                        .await;
                    finished_cleanly = !cancelled;
                    break;
                }
                TtsEvent::Error(error) => {
                    let _ = tx
                        .send(Ok(
                            Event::default().data(json!({"error": error}).to_string())
                        ))
                        .await;
                    break;
                }
            }
        }
        if finished_cleanly
            && !cache_parts.is_empty()
            && let Ok(bytes) = stark_voice::wav::wav_bytes(&cache_parts, sr)
        {
            write_cache(cache, bytes).await;
        }
    });
    Sse::new(ReceiverStream::new(rx)).into_response()
}

async fn send_cached_sse(
    tx: mpsc::Sender<Result<Event, Infallible>>,
    bytes: Vec<u8>,
    want_pcm16: bool,
) -> anyhow::Result<()> {
    let mut reader = hound::WavReader::new(std::io::Cursor::new(bytes))?;
    let spec = reader.spec();
    let samples: Vec<f32> = reader
        .samples::<i16>()
        .map(|sample| sample.map(|v| v as f32 / 32768.0))
        .collect::<Result<_, _>>()?;
    for chunk in samples.chunks(1920) {
        send_pcm_sse(&tx, chunk, spec.sample_rate, want_pcm16).await?;
    }
    let _ = tx
        .send(Ok(
            Event::default().data(json!({"done": true, "cancelled": false}).to_string())
        ))
        .await;
    Ok(())
}

async fn send_pcm_sse(
    tx: &mpsc::Sender<Result<Event, Infallible>>,
    samples: &[f32],
    sample_rate: u32,
    want_pcm16: bool,
) -> anyhow::Result<()> {
    let payload = if want_pcm16 {
        json!({
            "pcm16": base64::engine::general_purpose::STANDARD.encode(stark_voice::wav::pcm16(samples)),
            "sr": sample_rate,
            "n": samples.len(),
        })
    } else {
        let wav = stark_voice::wav::wav_bytes(samples, sample_rate)?;
        json!({
            "chunk": base64::engine::general_purpose::STANDARD.encode(wav),
            "sr": sample_rate,
        })
    };
    tx.send(Ok(Event::default().data(payload.to_string())))
        .await
        .map_err(|_| anyhow::anyhow!("TTS client disconnected"))
}

pub async fn tts_warm_handler(State(state): State<ServerState>) -> Json<Value> {
    let Some(voice) = state.voice.as_ref() else {
        return Json(
            json!({"tts_warm": false, "tts_loading": false, "error": "voice models are not installed"}),
        );
    };
    if voice.tts.is_warm() {
        return Json(json!({"tts_warm": true, "tts_loading": false}));
    }
    let tts = voice.tts.clone();
    tokio::spawn(async move {
        if let Err(error) = tts.warm().await {
            tracing::warn!("TTS warm failed: {error:#}");
        }
    });
    Json(json!({"tts_warm": false, "tts_loading": true}))
}

pub async fn tts_release_handler(State(state): State<ServerState>) -> Json<Value> {
    let released = state
        .voice
        .as_ref()
        .map(|voice| voice.tts.release())
        .unwrap_or(false);
    Json(json!({"released": released}))
}

pub async fn tts_status_handler(State(state): State<ServerState>) -> Json<Value> {
    let warm = state
        .voice
        .as_ref()
        .map(|voice| voice.tts.is_warm())
        .unwrap_or(false);
    Json(json!({"tts_warm": warm}))
}

pub async fn release_asr_handler() -> Json<Value> {
    Json(json!({"released": true}))
}

pub async fn tts_cancel_handler(State(state): State<ServerState>) -> Json<Value> {
    if let Some(voice) = state.voice.as_ref() {
        voice.tts.cancel_current();
    }
    Json(json!({"cancelled": true}))
}

pub async fn voice_start_handler(State(state): State<ServerState>) -> Response {
    let Some(capture) = state.voice.as_ref().and_then(|v| v.capture.clone()) else {
        return unavailable("Moonshine STT is not available in this build");
    };
    match tokio::task::spawn_blocking(move || capture.start()).await {
        Ok(Ok(active)) => Json(json!({"active": active})).into_response(),
        Ok(Err(error)) => unavailable(&format!("Capture unavailable: {error:#}")),
        Err(error) => unavailable(&format!("Capture task failed: {error}")),
    }
}

pub async fn voice_status_handler(State(state): State<ServerState>) -> Json<Value> {
    let active = state
        .voice
        .as_ref()
        .and_then(|v| v.capture.as_ref())
        .map(|capture| capture.active())
        .unwrap_or(false);
    let stt_model = state
        .voice
        .as_ref()
        .and_then(|v| v.stt.as_ref())
        .map(|stt| stt.model_name())
        .unwrap_or("unavailable");
    Json(json!({"active": active, "stt_model": stt_model}))
}

pub async fn voice_stop_handler(
    State(state): State<ServerState>,
    Json(body): Json<Value>,
) -> Response {
    let discard = body
        .get("discard")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let Some(capture) = state.voice.as_ref().and_then(|v| v.capture.clone()) else {
        return unavailable("Moonshine STT is not available in this build");
    };
    match tokio::task::spawn_blocking(move || capture.stop(discard)).await {
        Ok(Ok(text)) => {
            if discard {
                Json(json!({"text": "", "engine": "moonshine", "discarded": true})).into_response()
            } else if text.is_empty() {
                error_json(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Moonshine could not transcribe the audio (empty result)",
                )
            } else {
                Json(json!({"text": text, "engine": "moonshine"})).into_response()
            }
        }
        Ok(Err(error)) => unavailable(&format!("Moonshine ASR unavailable: {error:#}")),
        Err(error) => unavailable(&format!("Capture task failed: {error}")),
    }
}

pub async fn voice_mute_handler(
    State(state): State<ServerState>,
    Json(body): Json<Value>,
) -> Response {
    let muted = body.get("muted").and_then(Value::as_bool).unwrap_or(false);
    let Some(capture) = state.voice.as_ref().and_then(|v| v.capture.as_ref()) else {
        return unavailable("Moonshine STT is not available in this build");
    };
    capture.set_muted(muted);
    Json(json!({"muted": muted})).into_response()
}

pub async fn voice_interrupt_handler(
    State(state): State<ServerState>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let played = body
        .get("played_ms")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .max(0);
    if let Some(voice) = state.voice.as_ref() {
        voice.tts.cancel_current();
    }
    Json(json!({"interrupted_at_ms": played, "tts_cancelled": true}))
}

pub async fn voice_peek_handler(State(state): State<ServerState>) -> Json<Value> {
    let (text, level, active) = state
        .voice
        .as_ref()
        .and_then(|v| v.capture.as_ref())
        .map(|capture| (capture.live_text(), capture.live_peak(), capture.active()))
        .unwrap_or_default();
    Json(json!({"text": text, "level": level, "active": active}))
}

pub async fn voice_send_handler(State(state): State<ServerState>) -> Response {
    let Some(capture) = state.voice.as_ref().and_then(|v| v.capture.clone()) else {
        return unavailable("Moonshine STT is not available in this build");
    };
    match tokio::task::spawn_blocking(move || capture.flush_round()).await {
        Ok(Ok(text)) if !text.is_empty() => {
            Json(json!({"text": text, "engine": "moonshine"})).into_response()
        }
        Ok(Ok(_)) => error_json(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Moonshine could not transcribe the audio (empty result)",
        ),
        Ok(Err(error)) => unavailable(&format!("Moonshine ASR unavailable: {error:#}")),
        Err(error) => unavailable(&format!("Capture task failed: {error}")),
    }
}

pub async fn transcribe_handler(
    State(state): State<ServerState>,
    mut multipart: Multipart,
) -> Response {
    let Some(stt) = state.voice.as_ref().and_then(|v| v.stt.clone()) else {
        return unavailable("Moonshine STT is not available in this build");
    };
    let mut bytes = Vec::new();
    let mut extension = "wav".to_string();
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            if let Some(name) = field.file_name()
                && let Some(ext) = Path::new(name).extension().and_then(|e| e.to_str())
            {
                let safe: String = ext
                    .chars()
                    .filter(|ch| ch.is_ascii_alphanumeric())
                    .take(8)
                    .collect();
                if !safe.is_empty() {
                    extension = safe;
                }
            }
            match field.bytes().await {
                Ok(data) => bytes = data.to_vec(),
                Err(error) => {
                    return error_json(
                        StatusCode::BAD_REQUEST,
                        &format!("cannot read upload: {error}"),
                    );
                }
            }
        }
    }
    if bytes.is_empty() {
        return error_json(StatusCode::BAD_REQUEST, "Empty audio file");
    }
    let temp = std::env::temp_dir().join(format!("stark_stt_{}.{}", Uuid::new_v4(), extension));
    if let Err(error) = tokio::fs::write(&temp, &bytes).await {
        return unavailable(&format!("cannot store upload: {error}"));
    }
    let path = temp.clone();
    let result = tokio::task::spawn_blocking(move || stt.transcribe_file(&path)).await;
    let _ = tokio::fs::remove_file(&temp).await;
    match result {
        Ok(Ok(text)) if !text.trim().is_empty() => {
            Json(json!({"text": text.trim(), "engine": "moonshine"})).into_response()
        }
        Ok(Ok(_)) => error_json(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Moonshine could not transcribe the audio (empty result)",
        ),
        Ok(Err(error)) => unavailable(&format!("Moonshine ASR unavailable: {error:#}")),
        Err(error) => unavailable(&format!("Transcription task failed: {error}")),
    }
}

pub async fn turn_detection_handler() -> Json<Value> {
    Json(json!({
        "sensitivity": std::env::var("TURN_SENSITIVITY").unwrap_or_else(|_| "medium".to_string()),
        "prefix_padding_ms": env_i64("TURN_PREFIX_PADDING_MS", 300),
        "silence_duration_ms": env_i64("TURN_SILENCE_MS", 650),
        "sound_level": env_f64("TURN_SOUND_LEVEL", 0.01),
        "min_round_ms": env_i64("TURN_MIN_ROUND_MS", 250),
        "max_round_ms": env_i64("TURN_MAX_ROUND_MS", 30000),
    }))
}

fn env_i64(name: &str, default: i64) -> i64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_f64(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn audio_response(bytes: Vec<u8>) -> Response {
    (StatusCode::OK, [(header::CONTENT_TYPE, "audio/wav")], bytes).into_response()
}

fn unavailable(message: &str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": message})),
    )
        .into_response()
}

fn error_json(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error": message}))).into_response()
}

fn tts_cache_path(root: &Path, text: &str, voice: &str) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(b"pocket-tts-18m\0");
    hasher.update(voice.as_bytes());
    hasher.update(b"\0");
    hasher.update(text.as_bytes());
    let key = format!("{:x}", hasher.finalize());
    root.join(".deep_agents")
        .join("tts_cache")
        .join(format!("{key}.wav"))
}

async fn read_cache(path: &Path) -> Option<Vec<u8>> {
    tokio::fs::read(path)
        .await
        .ok()
        .filter(|bytes| !bytes.is_empty())
}

async fn write_cache(path: PathBuf, bytes: Vec<u8>) {
    if let Some(parent) = path.parent()
        && tokio::fs::create_dir_all(parent).await.is_err()
    {
        return;
    }
    let temp = path.with_extension("wav.tmp");
    if tokio::fs::write(&temp, bytes).await.is_ok() {
        let _ = tokio::fs::rename(&temp, &path).await;
    }
    prune_cache(&path).await;
}

async fn prune_cache(path: &Path) {
    let Some(dir) = path.parent() else {
        return;
    };
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };
    let now = std::time::SystemTime::now();
    let mut files = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Ok(metadata) = entry.metadata().await else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let age = metadata
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .map(|age| age.as_secs())
            .unwrap_or(0);
        if age > TTS_CACHE_MAX_AGE_SECS {
            let _ = tokio::fs::remove_file(entry.path()).await;
        } else {
            files.push((metadata.modified().ok(), entry.path()));
        }
    }
    if files.len() > TTS_CACHE_MAX_FILES {
        files.sort_by_key(|(modified, _)| *modified);
        let remove = files.len().saturating_sub(TTS_CACHE_MAX_FILES);
        for (_, path) in files.into_iter().take(remove) {
            let _ = tokio::fs::remove_file(path).await;
        }
    }
}
