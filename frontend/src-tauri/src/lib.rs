mod backend;
mod commands;
mod screen;
mod storage;
mod voice;

use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::Child;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{
    AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    webview::{NewWindowResponse, PageLoadEvent},
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tauri_plugin_opener::OpenerExt;

pub(crate) const DEFAULT_PORT: u16 = 8123;

#[derive(Default)]
pub(crate) struct ShellState {
    pub(crate) quitting: AtomicBool,
    pub(crate) backend: Mutex<Option<Child>>,
}

pub(crate) fn api_port() -> u16 {
    std::env::var("STARK_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_PORT)
}

pub(crate) fn api_origin() -> String {
    format!("http://127.0.0.1:{}", api_port())
}

pub(crate) fn ui_url() -> String {
    "http://localhost:9000".to_string()
}

pub(crate) fn is_allowed_url(url: &tauri::Url) -> bool {
    if url.scheme() == "tauri" {
        return true;
    }
    if let Some(host) = url.host_str() {
        if host == "tauri.localhost" {
            return true;
        }
        if tauri::is_dev()
            && (host == "localhost" || host == "127.0.0.1")
            && url.port() == Some(9000)
        {
            return true;
        }
        if (host == "127.0.0.1" || host == "localhost")
            && (url.port() == Some(api_port()) || url.port() == Some(8124))
        {
            return true;
        }
    }
    false
}

fn api_healthy() -> bool {
    let address = format!("127.0.0.1:{}", api_port());
    let Ok(mut stream) = TcpStream::connect_timeout(
        &address.parse().expect("valid API address"),
        Duration::from_millis(900),
    ) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(900)));
    let request = "GET /api/health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut buffer = [0_u8; 256];
    let read = stream.read(&mut buffer).unwrap_or(0);
    String::from_utf8_lossy(&buffer[..read]).contains(" 200 ")
}

fn spawn_main(app: AppHandle) {
    std::thread::spawn(move || {
        if let Err(error) = create_main_window(&app) {
            log::error!("failed to create main window: {error}");
        }
    });
}

pub(crate) fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    } else if let Err(error) = create_main_window(app) {
        log::error!("failed to reopen main window: {error}");
    }
}

fn create_main_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(window);
    }

    let url = if tauri::is_dev() {
        WebviewUrl::External(ui_url().parse().expect("valid Stark development UI URL"))
    } else {
        WebviewUrl::App("index.html".into())
    };
    let navigation_app = app.clone();
    let new_window_app = app.clone();

    let window = WebviewWindowBuilder::new(app, "main", url)
        .title("Stark — Agent Workspace")
        .inner_size(1440.0, 900.0)
        .min_inner_size(1100.0, 700.0)
        .background_color(tauri::webview::Color(12, 12, 14, 255))
        .additional_browser_args("--autoplay-policy=no-user-gesture-required")
        .decorations(false)
        .maximized(true)
        .visible(false)
        .initialization_script(format!(
            "{}\n{}",
            storage::seed_script(app),
            main_bridge_script()
        ))
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
        .on_document_title_changed(|window, title| {
            let _ = window.set_title(&title);
        })
        .on_page_load(|window, payload| {
            if payload.event() == PageLoadEvent::Finished {
                let _ = window.show();
                let _ = window.set_focus();
            }
        })
        .build()?;

    window.show()?;
    Ok(window)
}

fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Stark", true, None::<&str>)?;
    let voice = MenuItem::with_id(app, "voice", "Toggle Voice", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Stark", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &voice, &quit])?;

    let mut builder = TrayIconBuilder::with_id("stark-tray")
        .tooltip("Stark — Voice Assistant (Ctrl+G)")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "voice" => {
                let _ = voice::toggle(app);
            }
            "quit" => {
                app.state::<ShellState>()
                    .quitting
                    .store(true, Ordering::SeqCst);
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app);
        }))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(ShellState::default())
        .manage(screen::ScreenState::default())
        .invoke_handler(tauri::generate_handler![
            commands::ping,
            commands::runtime_info,
            commands::pairing_base,
            commands::select_folder,
            commands::reveal_path,
            commands::voice_toggle,
            commands::voice_set_chat,
            commands::voice_close,
            commands::voice_emit_event,
            commands::voice_resize,
            commands::voice_screen_sources,
            commands::voice_screen_select,
            commands::voice_screen_frame,
            commands::voice_screen_protect,
        ])
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            backend::ensure_running(app.handle());
            setup_tray(app.handle())?;

            app.global_shortcut()
                .on_shortcut("CommandOrControl+G", |app, _shortcut, event| {
                    if event.state == ShortcutState::Pressed {
                        let _ = voice::toggle(app);
                    }
                })?;

            if tauri::is_dev() {
                let _ = create_main_window(app.handle());
            } else {
                spawn_main(app.handle().clone());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    let state = window.app_handle().state::<ShellState>();
                    if !state.quitting.load(Ordering::SeqCst) {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                } else if window.label() == "voice" {
                    let app = window.app_handle();
                    let _ = app.emit("voice-session-event", json!({ "type": "closed" }));
                    let _ = app.emit("voice-window-closed", ());
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while running Stark desktop")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                backend::stop(app);
            }
        });
}

fn main_bridge_script() -> String {
    const SCRIPT: &str = r#"
(function installStarkBridge() {
  const tauri = window.__TAURI__;
  if (!tauri) {
    window.setTimeout(installStarkBridge, 30);
    return;
  }
  const invoke = tauri.core.invoke;
  const listen = tauri.event.listen;
  const track = (promise) => {
    let disposed = false;
    let off = null;
    promise.then((unlisten) => {
      off = unlisten;
      if (disposed) unlisten();
    });
    return () => {
      disposed = true;
      if (off) off();
    };
  };
  window.__STARK_DESKTOP__ = "tauri";
  window.__STARK_PORT__ = __PORT__;
  window.__STARK_API_ORIGIN__ = "__API_ORIGIN__";
  window.antigravity = {
    selectFolder: () => invoke("select_folder"),
    getPairingBase: () => invoke("pairing_base"),
    revealPath: (target) => invoke("reveal_path", { target }),
    voiceToggle: () => invoke("voice_toggle"),
    voiceText: () => {},
    voiceStatus: () => {},
    setVoiceChat: (payload) => invoke("voice_set_chat", { payload }),
    onVoiceSessionEvent: (cb) =>
      track(listen("voice-session-event", (event) => cb(event.payload))),
    onVoiceWindowClosed: (cb) =>
      track(listen("voice-window-closed", () => cb())),
    offVoiceSessionEvent: () => {},
    offVoiceWindowClosed: () => {},
  };
})();
"#;
    SCRIPT
        .replace("__PORT__", &api_port().to_string())
        .replace("__API_ORIGIN__", &api_origin())
}

pub(crate) fn voice_bridge_script() -> String {
    const SCRIPT: &str = r#"
(function installVoiceBridge() {
  const tauri = window.__TAURI__;
  if (!tauri) {
    window.setTimeout(installVoiceBridge, 30);
    return;
  }
  const invoke = tauri.core.invoke;
  const listen = tauri.event.listen;
  window.__STARK_PORT__ = __PORT__;
  window.__STARK_API_ORIGIN__ = "__API_ORIGIN__";
  window.voiceBridge = {
    sendEvent: (payload) => invoke("voice_emit_event", { payload }),
    close: () => invoke("voice_close"),
    resize: (width, height, animate) =>
      invoke("voice_resize", { width, height, animate: !!animate }),
    onSetChat: (cb) => listen("voice-set-chat", (event) => cb(event.payload)),
    getStarkPort: () => String(window.__STARK_PORT__),
    listScreenSources: () => invoke("voice_screen_sources"),
    selectScreenSource: (id) => invoke("voice_screen_select", { id }),
    captureScreenFrame: () => invoke("voice_screen_frame"),
    setScreenProtection: (on) => invoke("voice_screen_protect", { on: !!on }),
  };
})();
"#;
    SCRIPT
        .replace("__PORT__", &api_port().to_string())
        .replace("__API_ORIGIN__", &api_origin())
}
