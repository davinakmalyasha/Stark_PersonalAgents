//! Live-screen bridge and tools.

//!

//! The Tauri voice overlay owns the desktop capture stream and the single `/api/voice/ws`

//! connection. Agent tools run inside the server and need to reach into that overlay:

//!

//! - `look_at_screen` asks for a fresh frame; the overlay uploads it to `/api/vision/frame`.

//! - `set_screen_share` switches the overlay's sharing mode.

//!

//! The bridge is intentionally process-global: the desktop app has exactly one voice overlay.

use std::collections::HashMap;

use std::sync::{Mutex, OnceLock};

use std::time::Duration;

use serde_json::{Value, json};

use tokio::sync::{mpsc, oneshot};

use crate::{ToolContext, ToolOutcome};

const FRAME_TIMEOUT: Duration = Duration::from_secs(12);

const MODE_TIMEOUT: Duration = Duration::from_secs(8);

const VALID_MODES: &[&str] = &["on", "off", "continuous"];

const OVERLAY_HINT: &str = "Ask the user to open live mode (Ctrl+G) in the Personal workspace.";

#[derive(Debug, Clone, Default)]

pub struct ScreenState {
    pub sharing: bool,

    pub mode: String,

    pub source: String,

    pub latest_frame: Option<String>,

    pub latest_at: Option<std::time::Instant>,
}

#[derive(Debug, Clone)]

pub struct ScreenFrame {
    pub ok: bool,

    pub path: String,

    pub attachment: Value,

    pub error: Option<String>,
}

struct Waiter<T> {
    tx: oneshot::Sender<T>,
}

struct Inner {
    outbox: Option<mpsc::UnboundedSender<Value>>,

    frames: HashMap<String, Waiter<ScreenFrame>>,

    modes: HashMap<String, Waiter<Result<Value, String>>>,

    state: ScreenState,
}

pub struct ScreenBridge {
    inner: Mutex<Inner>,
}

impl Default for ScreenBridge {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner {
                outbox: None,

                frames: HashMap::new(),

                modes: HashMap::new(),

                state: ScreenState {
                    sharing: false,

                    mode: "off".to_string(),

                    source: String::new(),

                    latest_frame: None,

                    latest_at: None,
                },
            }),
        }
    }
}

impl ScreenBridge {
    pub fn attach(&self, outbox: mpsc::UnboundedSender<Value>) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.outbox = Some(outbox);
        }
    }

    pub fn detach(&self) {
        let waiters = {
            let Ok(mut inner) = self.inner.lock() else {
                return;
            };

            inner.outbox = None;

            inner.state = ScreenState {
                sharing: false,

                mode: "off".to_string(),

                source: String::new(),

                latest_frame: None,

                latest_at: None,
            };

            let frames = std::mem::take(&mut inner.frames);

            let modes = std::mem::take(&mut inner.modes);

            (frames, modes)
        };

        for (_, waiter) in waiters.0 {
            let _ = waiter.tx.send(ScreenFrame {
                ok: false,

                path: String::new(),

                attachment: Value::Null,

                error: Some("Voice overlay disconnected".to_string()),
            });
        }

        for (_, waiter) in waiters.1 {
            let _ = waiter
                .tx
                .send(Err("Voice overlay disconnected".to_string()));
        }
    }

    pub fn connected(&self) -> bool {
        self.inner
            .lock()
            .map(|inner| inner.outbox.is_some())
            .unwrap_or(false)
    }

    pub fn state(&self) -> ScreenState {
        self.inner
            .lock()
            .map(|inner| inner.state.clone())
            .unwrap_or_default()
    }

    pub fn set_latest_frame(&self, path: String) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.state.latest_frame = Some(path);

            inner.state.latest_at = Some(std::time::Instant::now());
        }
    }

    pub fn latest_frame(&self, max_age: Duration) -> Option<String> {
        self.inner.lock().ok().and_then(|inner| {
            let path = inner.state.latest_frame.clone()?;

            let fresh = inner
                .state
                .latest_at
                .is_some_and(|at| at.elapsed() <= max_age);

            fresh.then_some(path)
        })
    }

    pub fn update_state(&self, value: &Value) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.state.sharing = value
                .get("sharing")
                .and_then(Value::as_bool)
                .unwrap_or(false);

            if let Some(mode) = value.get("mode").and_then(Value::as_str)
                && VALID_MODES.contains(&mode)
            {
                inner.state.mode = mode.to_string();
            }

            if let Some(source) = value.get("source").and_then(Value::as_str) {
                inner.state.source = source.to_string();
            }
        }
    }

    pub async fn request_frame(&self) -> ScreenFrame {
        let req_id = uuid::Uuid::new_v4().to_string();

        let (tx, rx) = oneshot::channel();

        {
            let Ok(mut inner) = self.inner.lock() else {
                return frame_error("Voice overlay is not running");
            };

            let Some(outbox) = inner.outbox.clone() else {
                return frame_error("Voice overlay is not running");
            };

            inner.frames.insert(req_id.clone(), Waiter { tx });

            if outbox
                .send(json!({"type": "screen-capture", "req_id": req_id}))
                .is_err()
            {
                inner.frames.remove(&req_id);

                return frame_error("Voice overlay is not reachable");
            }
        }

        match tokio::time::timeout(FRAME_TIMEOUT, rx).await {
            Ok(Ok(frame)) => frame,

            Ok(Err(_)) => frame_error("Voice overlay disconnected"),

            Err(_) => {
                if let Ok(mut inner) = self.inner.lock() {
                    inner.frames.remove(&req_id);
                }

                frame_error("Screen capture timed out after 12s")
            }
        }
    }

    pub async fn request_mode(&self, mode: &str) -> Result<Value, String> {
        let mode = mode.trim().to_ascii_lowercase();

        if !VALID_MODES.contains(&mode.as_str()) {
            return Err(format!(
                "Invalid mode '{mode}' (use on, off, or continuous)"
            ));
        }

        let req_id = uuid::Uuid::new_v4().to_string();

        let (tx, rx) = oneshot::channel();

        {
            let Ok(mut inner) = self.inner.lock() else {
                return Err("Voice overlay is not running".to_string());
            };

            let Some(outbox) = inner.outbox.clone() else {
                return Err("Voice overlay is not running".to_string());
            };

            inner.modes.insert(req_id.clone(), Waiter { tx });

            if outbox
                .send(json!({"type": "screen-mode", "req_id": req_id, "mode": mode}))
                .is_err()
            {
                inner.modes.remove(&req_id);

                return Err("Voice overlay is not reachable".to_string());
            }
        }

        match tokio::time::timeout(MODE_TIMEOUT, rx).await {
            Ok(Ok(result)) => result,

            Ok(Err(_)) => Err("Voice overlay disconnected".to_string()),

            Err(_) => {
                if let Ok(mut inner) = self.inner.lock() {
                    inner.modes.remove(&req_id);
                }

                Err("Screen mode switch timed out after 8s".to_string())
            }
        }
    }

    pub fn resolve_frame(&self, req_id: &str, attachment: Value) {
        let waiter = self
            .inner
            .lock()
            .ok()
            .and_then(|mut inner| inner.frames.remove(req_id));

        if let Some(waiter) = waiter {
            let path = attachment
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();

            let _ = waiter.tx.send(ScreenFrame {
                ok: true,

                path,

                attachment,

                error: None,
            });
        }
    }

    pub fn resolve_ack(&self, value: &Value) {
        let req_id = value
            .get("req_id")
            .and_then(Value::as_str)
            .unwrap_or_default();

        if req_id.is_empty() {
            return;
        }

        let frame = self
            .inner
            .lock()
            .ok()
            .and_then(|mut inner| inner.frames.remove(req_id));

        if let Some(waiter) = frame {
            let ok = value.get("ok").and_then(Value::as_bool).unwrap_or(false);

            let _ = waiter.tx.send(ScreenFrame {
                ok,

                path: value
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),

                attachment: Value::Null,

                error: (!ok).then(|| {
                    value
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("Screen request failed")
                        .to_string()
                }),
            });

            return;
        }

        let mode = self
            .inner
            .lock()
            .ok()
            .and_then(|mut inner| inner.modes.remove(req_id));

        if let Some(waiter) = mode {
            let result = if value.get("ok").and_then(Value::as_bool).unwrap_or(false) {
                Ok(value.clone())
            } else {
                Err(value
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("Screen request failed")
                    .to_string())
            };

            let _ = waiter.tx.send(result);
        }
    }
}

fn frame_error(message: &str) -> ScreenFrame {
    ScreenFrame {
        ok: false,

        path: String::new(),

        attachment: Value::Null,

        error: Some(message.to_string()),
    }
}

static BRIDGE: OnceLock<ScreenBridge> = OnceLock::new();

pub fn screen_bridge() -> &'static ScreenBridge {
    BRIDGE.get_or_init(ScreenBridge::default)
}

#[derive(Debug, Clone)]

pub struct PendingImage {
    pub url: String,

    pub note: String,
}

static PENDING_IMAGES: OnceLock<Mutex<HashMap<String, Vec<PendingImage>>>> = OnceLock::new();

fn pending_images() -> &'static Mutex<HashMap<String, Vec<PendingImage>>> {
    PENDING_IMAGES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn queue_image(chat_id: &str, url: String, note: String) {
    if let Ok(mut map) = pending_images().lock() {
        map.entry(chat_id.to_string())
            .or_default()
            .push(PendingImage { url, note });
    }
}

/// The rolling latest frame while the overlay shares continuously, encoded for inline vision.
pub fn latest_continuous_image() -> Option<PendingImage> {
    let bridge = screen_bridge();

    let state = bridge.state();

    if !state.sharing || state.mode != "continuous" {
        return None;
    }

    let path = bridge.latest_frame(Duration::from_secs(30))?;

    let path = std::path::Path::new(&path);

    let bytes = std::fs::read(path).ok()?;

    if bytes.is_empty() || bytes.len() > 6 * 1024 * 1024 {
        return None;
    }

    let mime = match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",

        "webp" => "image/webp",

        _ => "image/jpeg",
    };

    Some(PendingImage {
        url: format!(
            "data:{mime};base64,{}",
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
        ),

        note: "Live screen (continuous share — current view)".to_string(),
    })
}

pub fn drain_images(chat_id: &str) -> Vec<PendingImage> {
    pending_images()
        .lock()
        .ok()
        .and_then(|mut map| map.remove(chat_id))
        .unwrap_or_default()
}

pub async fn look_at_screen(ctx: &ToolContext, _args: &Value) -> ToolOutcome {
    if !screen_bridge().connected() {
        return ToolOutcome::err(format!(
            "Error: cannot look at the screen — Voice overlay is not running. {OVERLAY_HINT}"
        ));
    }

    let frame = screen_bridge().request_frame().await;

    if !frame.ok {
        let error = frame.error.unwrap_or_else(|| "unknown error".to_string());

        return ToolOutcome::err(format!(
            "Error: cannot look at the screen — {error}.{}",
            hint_for(&error)
        ));
    }

    let path = std::path::Path::new(&frame.path);

    let bytes = match std::fs::read(path) {
        Ok(bytes) if !bytes.is_empty() => bytes,

        Ok(_) => return ToolOutcome::err("Error: the screen frame was empty."),

        Err(error) => {
            return ToolOutcome::err(format!(
                "Error: the screen frame was saved but could not be read: {error}"
            ));
        }
    };

    if bytes.len() > 6 * 1024 * 1024 {
        return ToolOutcome::err(
            "Error: the screen frame is too large to attach inline (max 6 MB).",
        );
    }

    let mime = match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",

        "webp" => "image/webp",

        _ => "image/jpeg",
    };

    let url = format!(
        "data:{mime};base64,{}",
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
    );

    queue_image(
        &ctx.chat_id,
        url,
        "Live screen — captured by look_at_screen() just now".to_string(),
    );

    let mode = screen_bridge().state().mode;

    let tail = if mode == "on" {
        " Screen sharing was turned on for this capture."
    } else {
        ""
    };

    ToolOutcome::ok(format!(
        "[IMAGE] Live screen attached inline (vision) — the picture follows as an image block in the next user message. It was captured a moment ago, so describe only what is actually visible; if text is unreadable, say so.{tail}"
    ))
}

pub async fn set_screen_share(_ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let raw = args
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();

    let mode = match raw.as_str() {
        "on" | "start" | "share" | "look" => "on",

        "off" | "stop" | "hide" => "off",

        "continuous" | "always" | "live" => "continuous",

        _ => {
            return ToolOutcome::err("Error: mode must be one of 'on', 'off', or 'continuous'.");
        }
    };

    match screen_bridge().request_mode(mode).await {
        Ok(value) => {
            let current = value.get("mode").and_then(Value::as_str).unwrap_or(mode);

            ToolOutcome::ok(format!(
                "Screen sharing is now '{current}'. The overlay preview reflects the change."
            ))
        }

        Err(error) => ToolOutcome::err(format!(
            "Error: cannot change screen sharing — {error}.{}",
            hint_for(&error)
        )),
    }
}

fn hint_for(error: &str) -> String {
    let low = error.to_ascii_lowercase();

    if low.contains("not running")
        || low.contains("not reachable")
        || low.contains("disconnected")
        || low.contains("timed out")
    {
        format!(" {OVERLAY_HINT}")
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    use std::sync::Arc;

    #[tokio::test]

    async fn frame_request_resolves_from_the_overlay_upload() {
        let bridge = Arc::new(ScreenBridge::default());

        let (tx, mut rx) = mpsc::unbounded_channel();

        bridge.attach(tx);

        let task = {
            let bridge = bridge.clone();

            tokio::spawn(async move { bridge.request_frame().await })
        };

        let message = rx.recv().await.unwrap();

        assert_eq!(message["type"], "screen-capture");

        let req_id = message["req_id"].as_str().unwrap();

        bridge.resolve_frame(req_id, json!({"path": "C:/tmp/frame.jpg"}));

        let frame = task.await.unwrap();

        assert!(frame.ok);

        assert_eq!(frame.path, "C:/tmp/frame.jpg");

        bridge.detach();
    }

    #[tokio::test]

    async fn mode_request_resolves_from_an_ack() {
        let bridge = Arc::new(ScreenBridge::default());

        let (tx, mut rx) = mpsc::unbounded_channel();

        bridge.attach(tx);

        let task = {
            let bridge = bridge.clone();

            tokio::spawn(async move { bridge.request_mode("continuous").await })
        };

        let message = rx.recv().await.unwrap();

        assert_eq!(message["type"], "screen-mode");

        let req_id = message["req_id"].as_str().unwrap();

        bridge.resolve_ack(&json!({

            "type": "screen-ack",

            "req_id": req_id,

            "ok": true,

            "mode": "continuous",

        }));

        let result = task.await.unwrap().unwrap();

        assert_eq!(result["mode"], "continuous");

        bridge.detach();
    }

    #[tokio::test]

    async fn look_at_screen_queues_an_inline_image() {
        let dir = tempfile::tempdir().unwrap();

        let path = dir.path().join("frame.png");

        std::fs::write(&path, b"not-a-real-png-but-nonempty").unwrap();

        let bridge = screen_bridge();

        let (tx, mut rx) = mpsc::unbounded_channel();

        bridge.attach(tx);

        let ctx = crate::ToolContext::new(
            dir.path(),
            "screen-chat",
            Arc::new(crate::QuestionBroker::new()),
        );

        let task = tokio::spawn(async move { look_at_screen(&ctx, &json!({})).await });

        let message = rx.recv().await.unwrap();

        let req_id = message["req_id"].as_str().unwrap();

        bridge.resolve_frame(req_id, json!({"path": path.to_string_lossy()}));

        let outcome = task.await.unwrap();

        assert!(!outcome.is_error, "{}", outcome.content);

        let images = drain_images("screen-chat");

        assert_eq!(images.len(), 1);

        assert!(images[0].url.starts_with("data:image/png;base64,"));

        bridge.detach();
    }
}
