//! Native system-tray presence for Stark Rune.
//!
//! Rune's `Tray` runs its own Windows message loop, so the icon survives the
//! main window being hidden. That is the missing half of close-to-tray: the
//! island is the live surface, the tray is the way back and the explicit quit.
//!
//! The menu is intentionally small. Every action goes through the app handle;
//! the tray thread never touches shell state directly.

use std::sync::Mutex;

use crate::island::{IslandAttentionKind, IslandModel};
use crate::prefs::LivePrefs;
use crate::shell::ShellState;
use rune::{Tray, TrayEvent, TrayIcon, TrayItem, TraySpec};

static TRAY: Mutex<Option<Tray>> = Mutex::new(None);
static LAST: Mutex<Option<TrayUi>> = Mutex::new(None);

#[derive(Clone, Debug, PartialEq)]
struct TrayUi {
    tooltip: String,
    menu: Vec<TrayItem>,
}

fn tooltip(model: &IslandModel) -> String {
    if model.listening {
        return "Stark — listening".to_string();
    }
    if let Some(attention) = &model.attention {
        return match attention.kind {
            IslandAttentionKind::Question => "Stark — needs you".to_string(),
            IslandAttentionKind::Error => "Stark — a run failed".to_string(),
            IslandAttentionKind::Done => "Stark — run finished".to_string(),
            IslandAttentionKind::Draft => "Stark — voice draft ready".to_string(),
        };
    }
    if model.armed {
        return "Stark — live mode on".to_string();
    }
    match model.runs.len() {
        0 => "Stark — idle".to_string(),
        1 => "Stark — 1 agent running".to_string(),
        n => format!("Stark — {n} agents running"),
    }
}

fn menu(model: &IslandModel, prefs: &LivePrefs) -> Vec<TrayItem> {
    let mut items = vec![
        TrayItem::action("open", "Open Stark"),
        TrayItem::action(
            "listen",
            if model.listening {
                "Stop listening"
            } else if model.armed {
                "Start listening"
            } else {
                "Turn live mode on"
            },
        ),
        TrayItem::Separator,
        TrayItem::action(
            "island",
            if prefs.island {
                "Hide Stark Island"
            } else {
                "Show Stark Island"
            },
        ),
        TrayItem::action(
            "auto",
            if prefs.auto_send_voice {
                "Auto-send voice: on"
            } else {
                "Auto-send voice: off"
            },
        ),
    ];
    #[cfg(windows)]
    items.push(TrayItem::action(
        "startup",
        if prefs.start_with_windows {
            "Start with Windows: on"
        } else {
            "Start with Windows: off"
        },
    ));
    items.push(TrayItem::Separator);
    items.push(TrayItem::action("quit", "Quit Stark"));
    items
}

/// Create the tray once. A missing desktop tray (or a platform without one)
/// is a soft failure: the app keeps its island and window shortcuts.
pub fn install(handle: rune::AppHandle<ShellState>) {
    let mut slot = TRAY.lock().unwrap();
    if slot.is_some() {
        return;
    }
    let icon = tray_icon();
    let spec = TraySpec::new("stark-rune", "Stark", icon)
        .tooltip("Stark — starting")
        .menu(vec![
            TrayItem::action("open", "Open Stark"),
            TrayItem::Separator,
            TrayItem::action("quit", "Quit Stark"),
        ]);
    let tray = match Tray::new(spec, move |event| match event {
        TrayEvent::Activate { .. } => handle.open(),
        TrayEvent::Menu(id) => match id.as_str() {
            "open" => handle.open(),
            "listen" => {
                handle.update(|s, _| s.presence.listen_requested = true);
            }
            "island" => {
                handle.update(|s, _| {
                    let next = !s.settings.live.island;
                    s.settings.set_live_island(next);
                });
            }
            "auto" => {
                handle.update(|s, _| {
                    let next = !s.settings.live.auto_send_voice;
                    s.settings.set_auto_send_voice(next);
                });
            }
            #[cfg(windows)]
            "startup" => {
                handle.update(|s, _| {
                    let next = !s.settings.live.start_with_windows;
                    s.settings.set_start_with_windows(next);
                });
            }
            "quit" => handle.quit(),
            _ => {}
        },
        _ => {}
    }) {
        Ok(tray) => tray,
        Err(error) => {
            eprintln!("stark-rune: tray unavailable: {error}");
            return;
        }
    };
    *slot = Some(tray);
}

/// Keep the tiny tray label/menu current. Called from the shell poll; the
/// cache makes this free when nothing visible changed.
pub fn sync(model: &IslandModel, prefs: &LivePrefs) {
    let ui = TrayUi {
        tooltip: tooltip(model),
        menu: menu(model, prefs),
    };
    let changed = {
        let mut last = LAST.lock().unwrap();
        if last.as_ref() == Some(&ui) {
            false
        } else {
            *last = Some(ui.clone());
            true
        }
    };
    if !changed {
        return;
    }
    if let Some(tray) = TRAY.lock().unwrap().as_ref() {
        tray.set_tooltip(ui.tooltip);
        tray.set_menu(ui.menu);
    }
}

fn tray_icon() -> TrayIcon {
    const SIZE: u32 = 64;
    if let Ok(image) = image::load_from_memory(include_bytes!("../../../assets/stark-icon.png")) {
        let rgba = image.to_rgba8();
        let resized =
            image::imageops::resize(&rgba, SIZE, SIZE, image::imageops::FilterType::Lanczos3);
        return TrayIcon::rgba(SIZE, SIZE, resized.into_raw());
    }
    // A white circle is a poor logo but a better tray than a missing icon.
    let mut pixels = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    let c = SIZE as f32 / 2.0;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 + 0.5 - c;
            let dy = y as f32 + 0.5 - c;
            let inside = dx * dx + dy * dy <= (c - 4.0) * (c - 4.0);
            pixels.extend_from_slice(if inside {
                &[255, 255, 255, 255]
            } else {
                &[0, 0, 0, 0]
            });
        }
    }
    TrayIcon::rgba(SIZE, SIZE, pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::island::{IslandAttention, IslandTarget};

    fn model() -> IslandModel {
        IslandModel::default()
    }

    #[test]
    fn tooltip_names_the_live_state() {
        let mut m = model();
        assert!(tooltip(&m).contains("idle"));
        m.runs.push(crate::island::IslandRun {
            target: IslandTarget::default(),
            activity: "Working".to_string(),
            elapsed: "0:01".to_string(),
            elapsed_secs: 1,
            stoppable: true,
        });
        assert!(tooltip(&m).contains("1 agent"));
        m.listening = true;
        assert!(tooltip(&m).contains("listening"));
        m.listening = false;
        m.armed = true;
        assert!(tooltip(&m).contains("live mode on"));
    }

    #[test]
    fn menu_reflects_the_toggles_and_attention() {
        let prefs = LivePrefs {
            island: false,
            auto_send_voice: true,
            exclude_from_capture: false,
            start_with_windows: false,
            ..LivePrefs::default()
        };
        let labels: Vec<String> = menu(&model(), &prefs)
            .into_iter()
            .filter_map(|item| match item {
                TrayItem::Action { label, .. } => Some(label),
                _ => None,
            })
            .collect();
        assert!(labels
            .iter()
            .any(|label| label.contains("Show Stark Island")));
        assert!(labels
            .iter()
            .any(|label| label.contains("Turn live mode on")));

        // Armed: the tray offers capture; live: it offers stop.
        let mut armed = model();
        armed.armed = true;
        let armed_labels: Vec<String> = menu(&armed, &prefs)
            .into_iter()
            .filter_map(|item| match item {
                TrayItem::Action { label, .. } => Some(label),
                _ => None,
            })
            .collect();
        assert!(armed_labels.iter().any(|l| l == "Start listening"));
        let mut live = model();
        live.listening = true;
        let live_labels: Vec<String> = menu(&live, &prefs)
            .into_iter()
            .filter_map(|item| match item {
                TrayItem::Action { label, .. } => Some(label),
                _ => None,
            })
            .collect();
        assert!(live_labels.iter().any(|l| l == "Stop listening"));
        assert!(labels
            .iter()
            .any(|label| label.contains("Auto-send voice: on")));
        assert!(labels.iter().any(|label| label.contains("Quit Stark")));

        let mut m = model();
        m.attention = Some(IslandAttention {
            kind: IslandAttentionKind::Question,
            title: "Needs you".to_string(),
            body: "Approve?".to_string(),
            target: IslandTarget::default(),
            expires_ms: 0,
        });
        assert!(tooltip(&m).contains("needs you"));
    }
}
