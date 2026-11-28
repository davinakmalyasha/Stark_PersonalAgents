//! Native spoken replies for the island's Live voice mode.
//!
//! The backend owns Pocket TTS. This module streams `/api/tts/stream` with
//! `format: "pcm16"`, decodes each SSE chunk, and appends it to one rodio
//! player so sentences stay gapless. Everything runs on a worker thread; the
//! Rune view only polls [`TtsPlayer::is_speaking`].

use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;

const TTS_PATH: &str = "/api/tts/stream";
const VOICE: &str = "jarvis";
const MAX_SPOKEN_CHARS: usize = 4_000;

#[derive(Clone)]
pub struct TtsPlayer {
    inner: Arc<Inner>,
}

struct Inner {
    generation: AtomicU64,
    speaking: AtomicBool,
    stop: AtomicBool,
    player: Mutex<Option<rodio::Player>>,
    sink: Mutex<Option<rodio::MixerDeviceSink>>,
    note: Mutex<Option<String>>,
}

impl Default for TtsPlayer {
    fn default() -> Self {
        Self {
            inner: Arc::new(Inner {
                generation: AtomicU64::new(0),
                speaking: AtomicBool::new(false),
                stop: AtomicBool::new(false),
                player: Mutex::new(None),
                sink: Mutex::new(None),
                note: Mutex::new(None),
            }),
        }
    }
}

impl std::fmt::Debug for TtsPlayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TtsPlayer")
            .field("speaking", &self.is_speaking())
            .finish()
    }
}

impl TtsPlayer {
    pub fn is_speaking(&self) -> bool {
        self.inner.speaking.load(Ordering::Relaxed)
    }

    /// Take one playback/synthesis failure for an inline error card.
    pub fn take_note(&self) -> Option<String> {
        self.inner.note.lock().ok().and_then(|mut note| note.take())
    }

    /// Stop local playback and cancel the server's current synthesis. Cheap
    /// and safe to call from the UI thread: the HTTP cancel runs on a worker.
    pub fn stop(&self) {
        self.inner.generation.fetch_add(1, Ordering::SeqCst);
        self.inner.stop.store(true, Ordering::SeqCst);
        self.inner.speaking.store(false, Ordering::SeqCst);
        if let Ok(mut player) = self.inner.player.lock() {
            if let Some(player) = player.take() {
                player.stop();
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let host = crate::rune_transcript::DEFAULT_API_HOST.to_string();
            let port = crate::rune_transcript::api_port();
            std::thread::spawn(move || {
                let _ = crate::transport::post_json_status(
                    &host,
                    port,
                    "/api/tts/cancel",
                    &serde_json::json!({}),
                );
            });
        }
    }

    /// Speak one final assistant message. A new call replaces the current
    /// utterance, which is also the barge-in path when the mic opens.
    pub fn speak(&self, text: &str) {
        let clean = clean_for_tts(text);
        if clean.is_empty() {
            return;
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = clean;
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.stop();
            let generation = self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1;
            self.inner.stop.store(false, Ordering::SeqCst);
            self.inner.speaking.store(true, Ordering::SeqCst);
            if let Ok(mut note) = self.inner.note.lock() {
                *note = None;
            }
            let player = self.clone();
            std::thread::Builder::new()
                .name("stark-tts".to_string())
                .spawn(move || player.run(generation, clean))
                .ok();
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn run(&self, generation: u64, text: String) {
        if let Err(error) = self.stream(generation, &text) {
            if self.current(generation) {
                if let Ok(mut note) = self.inner.note.lock() {
                    *note = Some(error);
                }
            }
        }
        // Let the queued audio finish before the speaking state drops, so the
        // equalizer does not stop before the last syllable.
        while self.current(generation) && !self.inner.stop.load(Ordering::SeqCst) {
            let empty = self
                .inner
                .player
                .lock()
                .ok()
                .and_then(|player| player.as_ref().map(|player| player.empty()))
                .unwrap_or(true);
            if empty {
                break;
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        if self.current(generation) {
            self.inner.speaking.store(false, Ordering::SeqCst);
        }
    }

    fn current(&self, generation: u64) -> bool {
        self.inner.generation.load(Ordering::SeqCst) == generation
            && !self.inner.stop.load(Ordering::SeqCst)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn stream(&self, generation: u64, text: &str) -> Result<(), String> {
        let host = crate::rune_transcript::DEFAULT_API_HOST;
        let port = crate::rune_transcript::api_port();
        let url = format!("http://{host}:{port}{TTS_PATH}");
        let agent = ureq::AgentBuilder::new()
            .timeout_read(Duration::from_secs(60))
            .timeout_write(Duration::from_secs(10))
            .build();
        let body = serde_json::json!({
            "text": text,
            "voice": VOICE,
            "format": "pcm16",
        })
        .to_string();
        let response = agent
            .post(&url)
            .set("Content-Type", "application/json")
            .send_string(&body)
            .map_err(|error| format!("TTS request failed: {error}"))?;
        let reader = BufReader::new(response.into_reader());
        let mut sample_rate = 24_000u32;
        for line in reader.lines() {
            if !self.current(generation) {
                return Ok(());
            }
            let line = line.map_err(|error| format!("TTS stream: {error}"))?;
            let Some(payload) = line.strip_prefix("data:") else {
                continue;
            };
            let payload = payload.trim();
            if payload.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
                continue;
            };
            if let Some(error) = value.get("error").and_then(|v| v.as_str()) {
                return Err(format!("TTS unavailable: {error}"));
            }
            if value.get("done").and_then(|v| v.as_bool()).unwrap_or(false) {
                return Ok(());
            }
            if let Some(sr) = value.get("sr").and_then(|v| v.as_u64()) {
                sample_rate = sr as u32;
            }
            let Some(encoded) = value.get("pcm16").and_then(|v| v.as_str()) else {
                continue;
            };
            let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
                continue;
            };
            let samples: Vec<f32> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| i16::from_le_bytes(*pair) as f32 / 32768.0)
                .collect();
            if samples.is_empty() {
                continue;
            }
            let mut player = self
                .inner
                .player
                .lock()
                .map_err(|_| "TTS player is unavailable".to_string())?;
            if player.is_none() {
                let mixer = {
                    let mut sink = self
                        .inner
                        .sink
                        .lock()
                        .map_err(|_| "TTS sink is unavailable".to_string())?;
                    if sink.is_none() {
                        let mut opened = rodio::DeviceSinkBuilder::open_default_sink()
                            .map_err(|error| format!("No audio output device: {error}"))?;
                        opened.log_on_drop(false);
                        *sink = Some(opened);
                    }
                    sink.as_ref()
                        .map(|sink| sink.mixer().clone())
                        .ok_or_else(|| "No audio output mixer".to_string())?
                };
                *player = Some(rodio::Player::connect_new(&mixer));
            }
            if let Some(player) = player.as_ref() {
                let channels = std::num::NonZeroU16::new(1).expect("one channel");
                let rate = std::num::NonZeroU32::new(sample_rate.max(1)).expect("sample rate");
                player.append(rodio::buffer::SamplesBuffer::new(channels, rate, samples));
                player.play();
            }
        }
        Ok(())
    }
}

/// Strip non-spoken markdown and tool noise. Code blocks are removed before
/// inline punctuation so a fenced block never leaks half a line into speech.
pub fn clean_for_tts(text: &str) -> String {
    let mut out = String::new();
    let mut fenced = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        // Tables and bare rules do not read well.
        if trimmed.starts_with('|') || trimmed.chars().all(|ch| "-=*_".contains(ch)) {
            continue;
        }
        out.push_str(line);
        out.push(' ');
    }
    let mut clean = out
        .replace(['#', '*', '_', '>', '~', '`'], " ")
        .replace("![", " ")
        .replace("](", " ")
        .replace('|', " ");
    // Drop link targets and bare URLs.
    let mut words = Vec::new();
    for word in clean.split_whitespace() {
        if word.starts_with("http://") || word.starts_with("https://") || word.starts_with("www.") {
            continue;
        }
        words.push(word);
    }
    clean = words.join(" ");
    clean = clean
        .replace("  ", " ")
        .replace(" .", ".")
        .replace(" ,", ",")
        .replace(" !", "!")
        .replace(" ?", "?")
        .trim()
        .to_string();
    if clean.chars().count() > MAX_SPOKEN_CHARS {
        clean = clean.chars().take(MAX_SPOKEN_CHARS).collect();
    }
    clean
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_code_tables_and_urls() {
        let text = "Here is the change.\n```rust\nfn main() {}\n```\n| a | b |\nSee https://example.com now.\n**Done**";
        let clean = clean_for_tts(text);
        assert!(clean.contains("Here is the change"));
        assert!(clean.contains("Done"));
        assert!(!clean.contains("fn main"));
        assert!(!clean.contains("example.com"));
        assert!(!clean.contains('*'));
    }

    #[test]
    fn caps_long_replies() {
        let clean = clean_for_tts(&"word ".repeat(2_000));
        assert!(clean.chars().count() <= MAX_SPOKEN_CHARS);
    }
}
