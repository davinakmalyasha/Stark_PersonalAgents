//! Local desktop preferences that are not server configuration.
//!
//! These live beside the harness data so a portable install keeps its feel
//! when moved, but they are never exported with chats. API keys remain in the
//! separate secrets store; nothing here is a secret.

use serde::{Deserialize, Serialize};

/// Stark Island and voice-dock preferences.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LivePrefs {
    /// Show the native top-edge island while the app runs. The island is the
    /// live surface even when no chat window is open.
    pub island: bool,
    /// Send a finished voice transcript immediately instead of leaving it in
    /// the active chat's composer as a draft.
    pub auto_send_voice: bool,
    /// Hide the island from screen recordings (`WDA_EXCLUDEFROMCAPTURE`).
    pub exclude_from_capture: bool,
    /// Launch the installed app in the background at sign-in.
    pub start_with_windows: bool,
}

impl Default for LivePrefs {
    fn default() -> Self {
        Self {
            island: true,
            auto_send_voice: false,
            exclude_from_capture: false,
            start_with_windows: false,
        }
    }
}

impl LivePrefs {
    /// Future schema migrations can normalize here. Unknown JSON keys are
    /// already ignored by serde, so older files stay forward compatible.
    pub fn sanitized(self) -> Self {
        self
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn prefs_path() -> Option<std::path::PathBuf> {
    let root = std::env::var("STARK_HARNESS_ROOT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var("LOCALAPPDATA").ok().map(|value| {
                std::path::PathBuf::from(value)
                    .join("StarkRune")
                    .join("data")
            })
        })?;
    Some(root.join(".deep_agents").join("desktop.json"))
}

/// Load preferences, falling back to defaults on a missing or malformed file.
pub fn load() -> LivePrefs {
    #[cfg(target_arch = "wasm32")]
    {
        LivePrefs::default()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(path) = prefs_path() else {
            return LivePrefs::default();
        };
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<LivePrefs>(&text).ok())
            .unwrap_or_default()
            .sanitized()
    }
}

/// Persist preferences atomically. Best effort: a read-only data root must
/// never stop the island from running.
pub fn save(prefs: &LivePrefs) {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = prefs;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(path) = prefs_path() else {
            return;
        };
        let Some(parent) = path.parent() else {
            return;
        };
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
        let Ok(text) = serde_json::to_vec_pretty(prefs) else {
            return;
        };
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(tmp, path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LivePrefs;

    #[test]
    fn defaults_show_the_island_and_stage_voice_drafts() {
        let prefs = LivePrefs::default();
        assert!(prefs.island, "the island is the live surface");
        assert!(
            !prefs.auto_send_voice,
            "voice defaults to a reviewable draft"
        );
        assert!(!prefs.exclude_from_capture);
        assert!(!prefs.start_with_windows);
    }

    #[test]
    fn serde_keeps_unknown_keys_compatible_and_reads_partial_files() {
        let parsed: LivePrefs =
            serde_json::from_str(r#"{"auto_send_voice":true,"future":1}"#).unwrap();
        assert!(parsed.island, "missing fields keep their defaults");
        assert!(parsed.auto_send_voice);
        let round = serde_json::to_string(&parsed).unwrap();
        assert!(round.contains("auto_send_voice"));
    }
}
