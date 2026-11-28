//! Screen sharing for the island's Live mode.
//!
//! The overlay shares one display. A small worker captures a JPEG every few
//! seconds and uploads it to the backend's `/api/vision/frame`; the returned
//! attachment rides the next chat turn. Capture and upload never run on the
//! Rune UI thread. Platforms without `xcap` get an honest unavailable state.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;

use crate::chats::ChatAttachment;
use crate::island::IslandScreenSource;

const FRAME_WIDTH: u32 = 1280;
const FRAME_QUALITY: u8 = 72;

#[derive(Clone)]
pub struct ScreenShare {
    inner: Arc<Inner>,
}

struct Inner {
    state: Mutex<State>,
}

struct State {
    generation: u64,
    sharing: bool,
    source_id: String,
    source_name: String,
    latest: Option<ChatAttachment>,
    stop: Arc<AtomicBool>,
}

impl Default for ScreenShare {
    fn default() -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    generation: 0,
                    sharing: false,
                    source_id: String::new(),
                    source_name: String::new(),
                    latest: None,
                    stop: Arc::new(AtomicBool::new(false)),
                }),
            }),
        }
    }
}

impl std::fmt::Debug for ScreenShare {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScreenShare")
            .field("sharing", &self.is_sharing())
            .field("source", &self.source_name())
            .finish()
    }
}

impl ScreenShare {
    pub fn is_sharing(&self) -> bool {
        self.inner
            .state
            .lock()
            .map(|state| state.sharing)
            .unwrap_or(false)
    }

    pub fn source_name(&self) -> String {
        self.inner
            .state
            .lock()
            .map(|state| state.source_name.clone())
            .unwrap_or_default()
    }

    pub fn source_id(&self) -> String {
        self.inner
            .state
            .lock()
            .map(|state| state.source_id.clone())
            .unwrap_or_default()
    }

    /// The freshest frame for this chat. `target_key` is used as the backend's
    /// chat id so a new chat never inherits another chat's frame.
    pub fn latest(&self, target_key: &str) -> Option<ChatAttachment> {
        let state = self.inner.state.lock().ok()?;
        let attachment = state.latest.clone()?;
        // The backend embeds the chat folder in the path; requiring the key
        // keeps a rekeyed or switched chat from picking up the old picture.
        if !target_key.is_empty()
            && !attachment.path.is_empty()
            && !attachment.path.contains(&safe_key(target_key))
        {
            return None;
        }
        Some(attachment)
    }

    /// Start (or restart) sharing one display. The worker captures immediately
    /// and then refreshes every few seconds for the next turn.
    pub fn start(
        &self,
        host: &str,
        port: u16,
        target_key: &str,
        source_id: &str,
    ) -> Result<String, String> {
        self.stop();
        let sources = list_sources();
        let selected = if source_id.trim().is_empty() {
            sources
                .iter()
                .find(|source| source.primary)
                .or_else(|| sources.first())
        } else {
            sources.iter().find(|source| source.id == source_id)
        }
        .cloned()
        .ok_or_else(|| {
            if sources.is_empty() {
                "No display is available to share.".to_string()
            } else {
                "That display is no longer available.".to_string()
            }
        })?;
        let stop = Arc::new(AtomicBool::new(false));
        let generation = {
            let mut state = self
                .inner
                .state
                .lock()
                .map_err(|_| "Screen state is unavailable.".to_string())?;
            state.generation = state.generation.wrapping_add(1);
            state.sharing = true;
            state.source_id = selected.id.clone();
            state.source_name = selected.name.clone();
            state.latest = None;
            state.stop = stop.clone();
            state.generation
        };
        let share = self.clone();
        let host = host.to_string();
        let target = target_key.to_string();
        let source = selected.id.clone();
        let source_name = selected.name.clone();
        std::thread::Builder::new()
            .name("stark-screen".to_string())
            .spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    if let Ok(attachment) =
                        capture_upload(&host, port, &target, &source, &source_name)
                    {
                        if let Ok(mut state) = share.inner.state.lock() {
                            if state.generation == generation {
                                state.latest = Some(attachment);
                            }
                        }
                    }
                    // Sleep in short slices so stopping is immediate.
                    for _ in 0..25 {
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            })
            .map_err(|error| format!("Screen worker: {error}"))?;
        Ok(selected.name)
    }

    pub fn stop(&self) {
        let mut state = match self.inner.state.lock() {
            Ok(state) => state,
            Err(_) => return,
        };
        state.stop.store(true, Ordering::Relaxed);
        state.generation = state.generation.wrapping_add(1);
        state.sharing = false;
        state.latest = None;
        state.source_id.clear();
        state.source_name.clear();
    }
}

fn safe_key(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .take(80)
        .collect()
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub fn list_sources() -> Vec<IslandScreenSource> {
    let mut items = Vec::new();
    let Ok(monitors) = xcap::Monitor::all() else {
        return items;
    };
    for (index, monitor) in monitors.iter().enumerate() {
        let Ok(id) = monitor.id() else {
            continue;
        };
        let primary = monitor.is_primary().unwrap_or(false);
        let name = monitor
            .friendly_name()
            .or_else(|_| monitor.name())
            .ok()
            .filter(|name| !name.trim().is_empty() && !name.starts_with("Unknown Monitor"))
            .unwrap_or_else(|| format!("Display {}", index + 1));
        items.push(IslandScreenSource {
            id: format!("monitor:{id}"),
            name,
            primary,
        });
    }
    items.sort_by_key(|source| !source.primary);
    items
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn capture_upload(
    host: &str,
    port: u16,
    target_key: &str,
    source_id: &str,
    source_name: &str,
) -> Result<ChatAttachment, String> {
    let image = capture(source_id)?;
    let (encoded, _, _) = encode_jpeg(&image, FRAME_WIDTH, FRAME_QUALITY)
        .ok_or_else(|| "Could not encode the screen frame.".to_string())?;
    let body = serde_json::json!({
        "image_b64": encoded,
        "mime": "image/jpeg",
        "chat_id": target_key,
        "label": "continuous",
        "source": source_name,
    });
    let response = crate::transport::post_json_status(host, port, "/api/vision/frame", &body)?;
    if !(200..300).contains(&response.status) {
        return Err(response
            .body
            .get("error")
            .and_then(|value| value.as_str())
            .unwrap_or("The screen frame was not accepted.")
            .to_string());
    }
    let attachment = response
        .body
        .get("attachment")
        .ok_or_else(|| "The screen frame response had no attachment.".to_string())?;
    Ok(ChatAttachment::from_json(attachment))
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn capture(source_id: &str) -> Result<image::RgbaImage, String> {
    let raw = source_id
        .strip_prefix("monitor:")
        .ok_or_else(|| "Unknown display.".to_string())?;
    let id: u32 = raw.parse().map_err(|_| "Unknown display.".to_string())?;
    let monitors = xcap::Monitor::all().map_err(|error| error.to_string())?;
    let monitor = monitors
        .into_iter()
        .find(|monitor| monitor.id().ok() == Some(id))
        .ok_or_else(|| "That display is no longer available.".to_string())?;
    monitor
        .capture_image()
        .map_err(|error| format!("Screen capture failed: {error}"))
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn encode_jpeg(
    image: &image::RgbaImage,
    max_width: u32,
    quality: u8,
) -> Option<(String, u32, u32)> {
    use image::imageops::FilterType;
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return None;
    }
    let scaled = if width > max_width {
        let target_height = ((height as f64) * (max_width as f64 / width as f64)).round() as u32;
        image::imageops::resize(image, max_width, target_height.max(1), FilterType::Triangle)
    } else {
        image.clone()
    };
    let (out_width, out_height) = scaled.dimensions();
    let rgb = image::DynamicImage::ImageRgba8(scaled).to_rgb8();
    let mut bytes = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut bytes);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, quality)
        .encode_image(&rgb)
        .ok()?;
    Some((
        base64::engine::general_purpose::STANDARD.encode(bytes),
        out_width,
        out_height,
    ))
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn list_sources() -> Vec<IslandScreenSource> {
    Vec::new()
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn capture_upload(
    _host: &str,
    _port: u16,
    _target_key: &str,
    _source_id: &str,
    _source_name: &str,
) -> Result<ChatAttachment, String> {
    Err("Screen sharing is available on Windows and macOS builds.".to_string())
}
