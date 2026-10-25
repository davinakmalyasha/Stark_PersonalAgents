use serde_json::{Value, json};
use tauri::{
    AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    utils::config::BackgroundThrottlingPolicy,
    webview::{NewWindowResponse, PageLoadEvent, PermissionKind, PermissionResponse},
};
use tauri_plugin_opener::OpenerExt;

use crate::{is_allowed_url, ui_url, voice_bridge_script};

const VOICE_WIDTH: f64 = 84.0;
const VOICE_HEIGHT: f64 = 84.0;

pub fn create(app: &AppHandle) -> Result<WebviewWindow, String> {
    if let Some(window) = app.get_webview_window("voice") {
        let _ = window.show();
        return Ok(window);
    }

    let (x, y) = if let Ok(Some(monitor)) = app.primary_monitor() {
        let size = monitor.size();
        let position = monitor.position();
        let scale = monitor.scale_factor();
        let logical_width = size.width as f64 / scale;
        (
            position.x as f64 / scale + (logical_width - VOICE_WIDTH) / 2.0,
            position.y as f64 / scale + 8.0,
        )
    } else {
        (800.0, 20.0)
    };

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0);
    let url = format!("{}/voice.html?stark_build={stamp}", ui_url());
    let navigation_app = app.clone();
    let new_window_app = app.clone();

    let window = WebviewWindowBuilder::new(
        app,
        "voice",
        WebviewUrl::External(
            url.parse()
                .map_err(|error| format!("invalid voice URL: {error}"))?,
        ),
    )
    .title("Stark Voice")
    .inner_size(VOICE_WIDTH, VOICE_HEIGHT)
    .position(x, y)
    .resizable(false)
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .focusable(false)
    .visible_on_all_workspaces(true)
    .no_redirection_bitmap(true)
    .content_protected(false)
    .background_throttling(BackgroundThrottlingPolicy::Disabled)
    .additional_browser_args("--autoplay-policy=no-user-gesture-required")
    .initialization_script(voice_bridge_script())
    .on_permission_request(|_webview, kind| match kind {
        PermissionKind::Microphone | PermissionKind::DisplayCapture => PermissionResponse::Allow,
        _ => PermissionResponse::Default,
    })
    .on_navigation(move |url| {
        if is_allowed_url(&url) {
            true
        } else {
            let _ = navigation_app.opener().open_url(url.as_str(), None::<&str>);
            false
        }
    })
    .on_new_window(move |url, _features| {
        let _ = new_window_app.opener().open_url(url.as_str(), None::<&str>);
        NewWindowResponse::Deny
    })
    .on_page_load(|window, payload| {
        if payload.event() == PageLoadEvent::Finished {
            let _ = window.show();
        }
    })
    .build()
    .map_err(|error| error.to_string())?;

    window.show().map_err(|error| error.to_string())?;
    Ok(window)
}

pub fn toggle(app: &AppHandle) -> Result<bool, String> {
    if let Some(window) = app.get_webview_window("voice") {
        window.close().map_err(|error| error.to_string())?;
        return Ok(false);
    }

    // WebView2 window creation must not run synchronously on the UI/IPC
    // thread on Windows (Tauri's documented deadlock). Create it on a worker
    // thread and let the JS command resolve immediately.
    let app = app.clone();
    std::thread::spawn(move || match create(&app) {
        Ok(_) => {
            let _ = app.emit("voice-session-event", json!({ "type": "opened" }));
        }
        Err(error) => {
            log::error!("failed to create voice window: {error}");
        }
    });
    Ok(true)
}

pub fn close(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("voice") {
        window.close().map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub fn set_chat(app: &AppHandle, payload: Value) -> Result<(), String> {
    app.emit_to("voice", "voice-set-chat", payload)
        .map_err(|error| error.to_string())
}

pub fn emit_event(app: &AppHandle, payload: Value) -> Result<(), String> {
    app.emit("voice-session-event", payload)
        .map_err(|error| error.to_string())
}

pub fn resize(app: &AppHandle, width: f64, height: f64, _animate: bool) -> Result<(), String> {
    let Some(window) = app.get_webview_window("voice") else {
        return Ok(());
    };
    window
        .set_size(tauri::Size::Logical(tauri::LogicalSize::new(width, height)))
        .map_err(|error| error.to_string())
}

pub fn set_screen_protection(app: &AppHandle, on: bool) -> Result<(), String> {
    let Some(window) = app.get_webview_window("voice") else {
        return Ok(());
    };
    window
        .set_content_protected(on)
        .map_err(|error| error.to_string())
}

pub fn screen_sources() -> Vec<Value> {
    Vec::new()
}

pub fn screen_select(_id: &str) -> Value {
    json!({ "ok": false, "id": "", "name": "" })
}
