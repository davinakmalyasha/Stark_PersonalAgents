use std::io::Cursor;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use image::{DynamicImage, RgbaImage, codecs::jpeg::JpegEncoder, imageops::FilterType};
use serde_json::{Value, json};
use xcap::{Monitor, Window};

const THUMBNAIL_WIDTH: u32 = 320;
const FRAME_WIDTH: u32 = 1280;
const CACHE_TTL: Duration = Duration::from_secs(10);

#[derive(Default)]
pub(crate) struct ScreenState {
    cache: Mutex<Option<CachedSources>>,
    selected: Mutex<Option<String>>,
}

struct CachedSources {
    captured_at: Instant,
    items: Vec<Value>,
}

#[derive(Clone)]
enum Target {
    Monitor(u32),
    Window(u32),
}

impl Target {
    fn parse(id: &str) -> Option<Self> {
        let (kind, raw) = id.split_once(':')?;
        let number = raw.parse::<u32>().ok()?;
        match kind {
            "monitor" => Some(Self::Monitor(number)),
            "window" => Some(Self::Window(number)),
            _ => None,
        }
    }
}

fn encode_jpeg(image: &RgbaImage, max_width: u32, quality: u8) -> Option<(String, u32, u32)> {
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return None;
    }
    let image = if width > max_width {
        let target_height = ((height as f64) * (max_width as f64 / width as f64)).round() as u32;
        image::imageops::resize(image, max_width, target_height.max(1), FilterType::Triangle)
    } else {
        image.clone()
    };
    let (out_width, out_height) = image.dimensions();
    let rgb = DynamicImage::ImageRgba8(image).to_rgb8();
    let mut bytes = Vec::new();
    let mut cursor = Cursor::new(&mut bytes);
    JpegEncoder::new_with_quality(&mut cursor, quality)
        .encode_image(&rgb)
        .ok()?;
    Some((BASE64.encode(bytes), out_width, out_height))
}

fn source_name(_kind: &str, name: &str) -> String {
    name.to_string()
}

fn list_sources() -> Vec<Value> {
    let mut items = Vec::new();

    if let Ok(monitors) = Monitor::all() {
        let mut monitors = monitors;
        monitors.sort_by_key(|monitor| !monitor.is_primary().unwrap_or(false));
        for (index, monitor) in monitors.iter().enumerate() {
            let Ok(id) = monitor.id() else {
                continue;
            };
            let name = monitor
                .friendly_name()
                .or_else(|_| monitor.name())
                .ok()
                .filter(|name| !name.starts_with("Unknown Monitor"))
                .unwrap_or_else(|| format!("Display {}", index + 1));
            let thumbnail = monitor
                .capture_image()
                .ok()
                .and_then(|image| encode_jpeg(&image, THUMBNAIL_WIDTH, 55))
                .map(|(b64, _, _)| format!("data:image/jpeg;base64,{b64}"));
            items.push(json!({
                "id": format!("monitor:{id}"),
                "kind": "screen",
                "name": source_name("screen", &name),
                "thumbnail": thumbnail,
            }));
        }
    }

    if let Ok(windows) = Window::all() {
        let own_pid = std::process::id();
        let mut added = 0;
        for window in windows {
            if added >= 40 {
                break;
            }
            if window.is_minimized().unwrap_or(true) {
                continue;
            }
            let Ok(pid) = window.pid() else {
                continue;
            };
            if pid == own_pid {
                continue;
            }
            let Ok(title) = window.title() else {
                continue;
            };
            let title = title.trim();
            if title.is_empty() || title.starts_with("Stark") {
                continue;
            }
            let app_name = window.app_name().unwrap_or_default();
            if app_name.to_lowercase().contains("stark") {
                continue;
            }
            let Ok(width) = window.width() else {
                continue;
            };
            let Ok(height) = window.height() else {
                continue;
            };
            if width < 160 || height < 120 {
                continue;
            }
            let Ok(id) = window.id() else {
                continue;
            };
            let thumbnail = window
                .capture_image()
                .ok()
                .and_then(|image| encode_jpeg(&image, THUMBNAIL_WIDTH, 55))
                .map(|(b64, _, _)| format!("data:image/jpeg;base64,{b64}"));
            items.push(json!({
                "id": format!("window:{id}"),
                "kind": "window",
                "name": source_name("window", title),
                "thumbnail": thumbnail,
            }));
            added += 1;
        }
    }

    items
}

pub(crate) fn sources(state: &ScreenState) -> Vec<Value> {
    let mut cache = state
        .cache
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(cached) = cache.as_ref() {
        if cached.captured_at.elapsed() < CACHE_TTL {
            return cached.items.clone();
        }
    }
    let items = list_sources();
    *cache = Some(CachedSources {
        captured_at: Instant::now(),
        items: items.clone(),
    });
    items
}

fn primary_target() -> Option<Target> {
    let monitors = Monitor::all().ok()?;
    let monitor = monitors
        .iter()
        .find(|monitor| monitor.is_primary().unwrap_or(false))
        .or_else(|| monitors.first())?;
    monitor.id().ok().map(Target::Monitor)
}

fn target_for(id: Option<&str>) -> Result<Target, String> {
    match id {
        Some(id) if !id.is_empty() => {
            Target::parse(id).ok_or_else(|| "Invalid screen source.".to_string())
        }
        _ => primary_target().ok_or_else(|| "No screen is available.".to_string()),
    }
}

fn target_label(target: &Target) -> String {
    match target {
        Target::Monitor(id) => Monitor::all()
            .ok()
            .and_then(|monitors| {
                monitors
                    .into_iter()
                    .find(|monitor| monitor.id().ok() == Some(*id))
            })
            .and_then(|monitor| monitor.friendly_name().or_else(|_| monitor.name()).ok())
            .map(|name| {
                if name.starts_with("Unknown Monitor") {
                    "Display".to_string()
                } else {
                    source_name("screen", &name)
                }
            })
            .unwrap_or_else(|| "Screen".to_string()),
        Target::Window(id) => Window::all()
            .ok()
            .and_then(|windows| {
                windows
                    .into_iter()
                    .find(|window| window.id().ok() == Some(*id))
            })
            .and_then(|window| window.title().ok())
            .map(|title| source_name("window", &title))
            .unwrap_or_else(|| "Window".to_string()),
    }
}

fn target_kind(target: &Target) -> &'static str {
    match target {
        Target::Monitor(_) => "screen",
        Target::Window(_) => "window",
    }
}

fn target_id(target: &Target) -> String {
    match target {
        Target::Monitor(id) => format!("monitor:{id}"),
        Target::Window(id) => format!("window:{id}"),
    }
}

fn capture(target: &Target) -> Result<RgbaImage, String> {
    match target {
        Target::Monitor(id) => Monitor::all()
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|monitor| monitor.id().ok() == Some(*id))
            .ok_or_else(|| "That screen is no longer available.".to_string())?
            .capture_image()
            .map_err(|error| error.to_string()),
        Target::Window(id) => Window::all()
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|window| window.id().ok() == Some(*id))
            .ok_or_else(|| "That window is no longer available.".to_string())?
            .capture_image()
            .map_err(|error| error.to_string()),
    }
}

pub(crate) fn select(state: &ScreenState, id: &str) -> Value {
    let target = match target_for(if id.is_empty() { None } else { Some(id) }) {
        Ok(target) => target,
        Err(error) => return json!({ "ok": false, "id": "", "name": "", "error": error }),
    };
    let id = target_id(&target);
    if let Ok(mut selected) = state.selected.lock() {
        *selected = Some(id.clone());
    }
    json!({
        "ok": true,
        "id": id,
        "kind": target_kind(&target),
        "name": target_label(&target),
    })
}

pub(crate) fn frame(state: &ScreenState) -> Result<Value, String> {
    let selected = state
        .selected
        .lock()
        .map_err(|_| "Screen state is unavailable.".to_string())?
        .clone();
    let target = target_for(selected.as_deref())?;
    let image = capture(&target)?;
    let (image_b64, width, height) = encode_jpeg(&image, FRAME_WIDTH, 72)
        .ok_or_else(|| "Could not encode the screen frame.".to_string())?;
    Ok(json!({
        "ok": true,
        "image_b64": image_b64,
        "mime": "image/jpeg",
        "width": width,
        "height": height,
        "source": target_label(&target),
        "id": target_id(&target),
    }))
}
