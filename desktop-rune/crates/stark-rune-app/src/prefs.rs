//! Local desktop preferences that are not server configuration.
//!
//! These live beside the harness data so a portable install keeps its feel
//! when moved, but they are never exported with chats. API keys remain in the
//! separate secrets store; nothing here is a secret.

use serde::{Deserialize, Serialize};

/// Hex's Parakeet vocabulary limits, mirrored so the app saves a list the
/// server can use without silently dropping most of it.
pub const MAX_VOCABULARY_TERMS: usize = 500;
pub const MAX_VOCABULARY_TERM_CHARS: usize = 64;
pub const MAX_VOCABULARY_SOUNDS_LIKE: usize = 16;

/// One name Parakeet should spell right, plus the spellings it may show
/// instead ("sounds like").
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VocabularyEntry {
    #[serde(default)]
    pub term: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sounds_like: Vec<String>,
}

/// How readily transcription swaps heard words for a vocabulary term.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VocabularyBoost {
    Off,
    Light,
    #[default]
    Normal,
    Strong,
}

impl VocabularyBoost {
    pub const ALL: [Self; 4] = [Self::Off, Self::Light, Self::Normal, Self::Strong];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Light => "Light",
            Self::Normal => "Normal",
            Self::Strong => "Strong",
        }
    }
}

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
    /// Names Parakeet should know. The bundled server reads this list with
    /// every dictation; the default keeps the old built-in "Stark" bias
    /// visible and editable.
    #[serde(default = "default_vocabulary")]
    pub vocabulary: Vec<VocabularyEntry>,
    /// Parakeet's vocabulary-rescoring strength.
    pub vocabulary_boost: VocabularyBoost,
}

fn default_vocabulary() -> Vec<VocabularyEntry> {
    vec![VocabularyEntry {
        term: "Stark".to_string(),
        sounds_like: Vec::new(),
    }]
}

impl Default for LivePrefs {
    fn default() -> Self {
        Self {
            island: true,
            auto_send_voice: false,
            exclude_from_capture: false,
            start_with_windows: false,
            vocabulary: default_vocabulary(),
            vocabulary_boost: VocabularyBoost::Normal,
        }
    }
}

impl LivePrefs {
    /// Normalize the list the way Hex and the server do: trimmed, unique
    /// terms in list order, at most 64 characters each, and up to 16 usable
    /// spellings per term.
    pub fn sanitized(mut self) -> Self {
        let mut entries: Vec<VocabularyEntry> = Vec::new();
        for entry in std::mem::take(&mut self.vocabulary) {
            let Some(term) = clean_term(&entry.term, MAX_VOCABULARY_TERM_CHARS) else {
                continue;
            };
            if entries
                .iter()
                .any(|kept| kept.term.to_lowercase() == term.to_lowercase())
            {
                continue;
            }
            if entries.len() >= MAX_VOCABULARY_TERMS {
                break;
            }
            let mut sounds_like: Vec<String> = Vec::new();
            for spelling in entry.sounds_like {
                let Some(spelling) = clean_term(&spelling, MAX_VOCABULARY_TERM_CHARS) else {
                    continue;
                };
                if spelling.to_lowercase() == term.to_lowercase()
                    || sounds_like
                        .iter()
                        .any(|kept| kept.to_lowercase() == spelling.to_lowercase())
                {
                    continue;
                }
                if sounds_like.len() >= MAX_VOCABULARY_SOUNDS_LIKE {
                    break;
                }
                sounds_like.push(spelling);
            }
            entries.push(VocabularyEntry { term, sounds_like });
        }
        self.vocabulary = entries;
        self
    }

    /// The list as the Settings field shows it: one entry per line, with
    /// `term | spelling, spelling` for "sounds like" spellings.
    pub fn vocabulary_text(&self) -> String {
        self.vocabulary
            .iter()
            .map(|entry| {
                if entry.sounds_like.is_empty() {
                    entry.term.clone()
                } else {
                    format!("{} | {}", entry.term, entry.sounds_like.join(", "))
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Replace the list from the Settings field. Returns whether it changed,
    /// so the caller can skip an identical write on every keypress.
    pub fn set_vocabulary_text(&mut self, text: &str) -> bool {
        let entries = text
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                if line.is_empty() {
                    return None;
                }
                let (term, sounds_like) = match line.split_once('|') {
                    Some((term, spellings)) => (term, spellings),
                    None => (line, ""),
                };
                Some(VocabularyEntry {
                    term: term.trim().to_string(),
                    sounds_like: sounds_like
                        .split(',')
                        .map(|spelling| spelling.trim().to_string())
                        .filter(|spelling| !spelling.is_empty())
                        .collect(),
                })
            })
            .collect::<Vec<_>>();
        let before = self.vocabulary.clone();
        self.vocabulary = entries;
        *self = self.clone().sanitized();
        self.vocabulary != before
    }

    pub fn set_vocabulary_boost(&mut self, boost: VocabularyBoost) -> bool {
        if self.vocabulary_boost == boost {
            return false;
        }
        self.vocabulary_boost = boost;
        true
    }
}

/// Collapses whitespace and trims to `max` characters, or `None` when
/// nothing is left.
fn clean_term(text: &str, max: usize) -> Option<String> {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed: String = collapsed.chars().take(max).collect();
    let trimmed = trimmed.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
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
    use super::{LivePrefs, VocabularyBoost, VocabularyEntry};

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
        assert_eq!(prefs.vocabulary[0].term, "Stark");
        assert_eq!(prefs.vocabulary_boost, VocabularyBoost::Normal);
    }

    #[test]
    fn serde_keeps_unknown_keys_compatible_and_reads_partial_files() {
        let parsed: LivePrefs =
            serde_json::from_str(r#"{"auto_send_voice":true,"future":1}"#).unwrap();
        assert!(parsed.island, "missing fields keep their defaults");
        assert!(parsed.auto_send_voice);
        assert_eq!(
            parsed.vocabulary[0].term, "Stark",
            "older files gain the visible default term"
        );
        let round = serde_json::to_string(&parsed).unwrap();
        assert!(round.contains("auto_send_voice"));
        assert!(round.contains("vocabulary"));
    }

    #[test]
    fn the_settings_text_round_trips_terms_and_sounds_like() {
        let mut prefs = LivePrefs::default();
        assert!(prefs
            .set_vocabulary_text("Stark\nOpenCode | open code, open-code\n\n  Rune  |  roon  "));
        assert_eq!(
            prefs.vocabulary,
            [
                VocabularyEntry {
                    term: "Stark".into(),
                    sounds_like: Vec::new()
                },
                VocabularyEntry {
                    term: "OpenCode".into(),
                    sounds_like: vec!["open code".into(), "open-code".into()]
                },
                VocabularyEntry {
                    term: "Rune".into(),
                    sounds_like: vec!["roon".into()]
                },
            ]
        );
        assert_eq!(
            prefs.vocabulary_text(),
            "Stark\nOpenCode | open code, open-code\nRune | roon"
        );
        let text = prefs.vocabulary_text();
        assert!(
            !prefs.set_vocabulary_text(&text),
            "an unchanged field is not written again"
        );
    }

    #[test]
    fn vocabulary_text_sanitizes_duplicates_and_caps() {
        let mut prefs = LivePrefs::default();
        prefs.set_vocabulary_text("Rune | roon\nrune | roon, roon\n  | ignored");
        assert_eq!(
            prefs.vocabulary,
            [VocabularyEntry {
                term: "Rune".into(),
                sounds_like: vec!["roon".into()]
            }]
        );
    }

    #[test]
    fn an_empty_vocabulary_survives_missing_fields_and_reloads_empty() {
        let parsed: LivePrefs = serde_json::from_str(r#"{"vocabulary":[]}"#).unwrap();
        assert!(
            parsed.vocabulary.is_empty(),
            "an explicit empty list stays empty"
        );
    }
}
