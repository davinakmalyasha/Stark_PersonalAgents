use serde_json::{Value, json};
use std::path::PathBuf;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::screen::{self, ScreenState};
use crate::voice;

#[tauri::command]
pub fn ping() -> &'static str {
    "pong"
}

#[tauri::command]
pub fn runtime_info() -> Value {
    json!({
        "port": crate::api_port(),
        "origin": crate::api_origin(),
        "desktop": "tauri",
        "dev": tauri::is_dev()
    })
}

#[tauri::command]
pub fn pairing_base() -> String {
    let ip = std::net::UdpSocket::bind("0.0.0.0:0")
        .ok()
        .and_then(|socket| {
            socket.connect("8.8.8.8:80").ok()?;
            socket.local_addr().ok()
        })
        .map(|address| address.ip().to_string())
        .unwrap_or_else(|| "127.0.0.1".to_string());
    format!("http://{ip}:{}", crate::api_port())
}

#[tauri::command]
pub fn select_folder(app: AppHandle) -> Option<String> {
    app.dialog()
        .file()
        .blocking_pick_folder()
        .and_then(|path| path.into_path().ok())
        .map(|path| path.to_string_lossy().into_owned())
}

fn projects_root() -> PathBuf {
    std::env::var("STARK_PROJECTS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(r"D:\StarkProjects"))
}

fn validated_project_path(target: &str) -> Result<PathBuf, String> {
    if target.trim().is_empty() {
        return Err("No path provided.".to_string());
    }
    let root = std::fs::canonicalize(projects_root())
        .map_err(|_| "The projects root does not exist.".to_string())?;
    let resolved =
        std::fs::canonicalize(target).map_err(|_| "That folder does not exist.".to_string())?;
    if !resolved.starts_with(&root) {
        return Err("That folder is outside the projects root.".to_string());
    }
    Ok(resolved)
}

#[tauri::command]
pub fn reveal_path(app: AppHandle, target: String) -> Result<(), String> {
    let path = validated_project_path(&target)?;
    if path.is_dir() {
        app.opener()
            .open_path(path.to_string_lossy().into_owned(), None::<&str>)
            .map_err(|error| error.to_string())
    } else {
        app.opener()
            .reveal_item_in_dir(&path)
            .map_err(|error| error.to_string())
    }
}

#[tauri::command]
pub fn voice_toggle(app: AppHandle) -> Result<bool, String> {
    voice::toggle(&app)
}

#[tauri::command]
pub fn voice_set_chat(app: AppHandle, payload: Value) -> Result<(), String> {
    voice::set_chat(&app, payload)
}

#[tauri::command]
pub fn voice_close(app: AppHandle) -> Result<(), String> {
    voice::close(&app)
}

#[tauri::command]
pub fn voice_emit_event(app: AppHandle, payload: Value) -> Result<(), String> {
    voice::emit_event(&app, payload)
}

#[tauri::command]
pub fn voice_resize(app: AppHandle, width: f64, height: f64, animate: bool) -> Result<(), String> {
    voice::resize(&app, width, height, animate)
}

#[tauri::command]
pub fn voice_screen_sources(state: State<'_, ScreenState>) -> Vec<Value> {
    screen::sources(&state)
}

#[tauri::command]
pub fn voice_screen_select(state: State<'_, ScreenState>, id: String) -> Value {
    screen::select(&state, &id)
}

#[tauri::command]
pub fn voice_screen_frame(state: State<'_, ScreenState>) -> Result<Value, String> {
    screen::frame(&state)
}

#[tauri::command]
pub fn voice_screen_protect(app: AppHandle, on: bool) -> Result<(), String> {
    voice::set_screen_protection(&app, on)
}
