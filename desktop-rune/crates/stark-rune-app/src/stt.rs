//! Streaming speech-to-text producer (Moonshine ONNX, medium-streaming-en).
//!
//! Mirrors the backend's `stark-voice::live` turn lifecycle: voice activity
//! detection with a learned noise floor plus pre-roll feeds an
//! `OwnedTranscriberStream`; interim text arrives as
//! [`VoiceMsg::Transcript`] with `is_final: false`, and the settled
//! utterance as `Transcript` with `is_final: true` (the existing overlay,
//! live-bubble and draft-staging path consumes both without changes).
//! Endpointing, pre-roll, poll cadence and the mid-thought extension use the
//! same `VOICE_*` environment knobs and defaults as the backend.
//!
//! The `voice-stt` feature + MSVC gate matches the backend (`stt`
//! feature): the native libmoonshine prebuilt is MSVC-only on Windows, so
//! other targets compile an honest stub and the toggle degrades to
//! meter-only levels instead of failing.

//! MSVC-only items are exercised by the test suite below; on other targets
//! the worker is stubbed out, so unused-code lints are silenced there (same
//! `cfg_attr` the backend applies to its gated live session).
#![cfg_attr(not(all(target_env = "msvc", feature = "voice-stt")), allow(dead_code))]

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
#[cfg(all(target_env = "msvc", feature = "voice-stt"))]
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;
#[cfg(all(target_env = "msvc", feature = "voice-stt"))]
use std::time::Instant;

use crate::voice::{rms_f32, VoiceMsg};

/// Fixed input rate for the streaming frontend (Moonshine resamples
/// internally, but the shell only forwards natively-16 kHz captures).
pub const SAMPLE_RATE: u32 = 16_000;
/// Poll the decoder for interim text at the backend's cadence.
pub const POLL_INTERVAL: Duration = Duration::from_millis(60);
/// Bounded audio queue between capture and the STT worker (drops on
/// overflow, like the backend's `try_send`).
pub const MAX_AUDIO_QUEUE: usize = 96;
/// Pre-roll frames so the first syllable is never clipped.
pub const PRE_ROLL_FRAMES: usize = 24;

/// Model files the streaming transcriber needs (same set the backend checks).
pub const REQUIRED_FILES: &[&str] = &[
    "encoder.ort",
    "decoder_kv.ort",
    "frontend.weights.ort",
    "frontend.model.ort",
    "cross_kv.ort",
    "adapter.ort",
    "streaming_config.json",
    "tokenizer.bin",
];

/// Endpointing + VAD tuning. Environment overrides use the backend's
/// `VOICE_*` names and clamp ranges so both surfaces behave identically.
#[derive(Debug, Clone)]
pub struct TurnConfig {
    /// Silence after speech that ends a turn.
    pub endpoint_ms: u64,
    /// Minimum voiced audio before a turn may end.
    pub min_speech_ms: u64,
    /// Hard cap on one turn.
    pub max_turn_ms: u64,
    /// Longer gap when the decoder ends mid-thought.
    pub extended_endpoint_ms: u64,
    /// Absolute RMS floor for voice (the learned room noise raises it).
    pub min_level: f32,
}

impl Default for TurnConfig {
    fn default() -> Self {
        Self {
            endpoint_ms: 900,
            min_speech_ms: 300,
            max_turn_ms: 30_000,
            extended_endpoint_ms: 1_800,
            min_level: 0.008,
        }
    }
}

impl TurnConfig {
    /// Load from the environment with the backend's defaults and clamps.
    pub fn from_env() -> Self {
        let base = Self::default();
        Self {
            endpoint_ms: env_u64("VOICE_ENDPOINT_MS", base.endpoint_ms).clamp(400, 3_000),
            min_speech_ms: env_u64("VOICE_MIN_SPEECH_MS", base.min_speech_ms).clamp(80, 1_500),
            max_turn_ms: env_u64("VOICE_MAX_TURN_MS", base.max_turn_ms).clamp(2_000, 120_000),
            extended_endpoint_ms: env_u64("VOICE_ENDPOINT_EXTENDED_MS", base.extended_endpoint_ms)
                .clamp(
                    env_u64("VOICE_ENDPOINT_MS", base.endpoint_ms).clamp(400, 3_000),
                    4_000,
                ),
            min_level: env_f32("VOICE_MIN_LEVEL", base.min_level).clamp(0.001, 0.2),
        }
    }
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_f32(name: &str, default: f32) -> f32 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Do not cut a thought after a short pause: if the decoder currently ends
/// on a continuation word, the backend allows a longer gap (same list).
pub fn ends_mid_thought(text: &str) -> bool {
    let last = text
        .trim()
        .trim_end_matches(|c: char| c.is_ascii_punctuation())
        .split_whitespace()
        .last()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        last.as_str(),
        "a" | "an"
            | "and"
            | "are"
            | "as"
            | "at"
            | "be"
            | "because"
            | "but"
            | "by"
            | "can"
            | "could"
            | "do"
            | "does"
            | "for"
            | "from"
            | "how"
            | "i"
            | "if"
            | "in"
            | "is"
            | "it"
            | "me"
            | "my"
            | "of"
            | "on"
            | "or"
            | "so"
            | "that"
            | "the"
            | "their"
            | "then"
            | "there"
            | "these"
            | "they"
            | "this"
            | "to"
            | "was"
            | "we"
            | "were"
            | "what"
            | "when"
            | "where"
            | "which"
            | "who"
            | "why"
            | "will"
            | "with"
            | "would"
            | "you"
            | "your"
    )
}

/// Turn-end decision shared by the worker and the tests: enough speech plus
/// enough trailing silence (extended mid-thought), or the hard cap.
pub fn should_finish_turn(
    cfg: &TurnConfig,
    speech_ms: u64,
    silence_ms: u64,
    total_ms: u64,
    last_partial: &str,
) -> bool {
    let endpoint = if ends_mid_thought(last_partial) {
        cfg.extended_endpoint_ms
    } else {
        cfg.endpoint_ms
    };
    (speech_ms >= cfg.min_speech_ms && silence_ms >= endpoint) || total_ms >= cfg.max_turn_ms
}

/// Model directory for the medium streaming model (the backend's pick).
pub fn stt_model_dir(models_dir: &Path) -> PathBuf {
    models_dir.join("stt").join("medium-streaming-en")
}

/// Missing model files, if any (fast pre-check before the slow ONNX load).
pub fn validate_model_dir(dir: &Path) -> Result<(), Vec<String>> {
    let missing: Vec<String> = REQUIRED_FILES
        .iter()
        .filter(|file| !dir.join(file).is_file())
        .map(|file| file.to_string())
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing)
    }
}

/// Where the shell looks for `models/`: `STARK_MODELS_DIR` first, then the
/// executable-relative candidates (mirrors the backend), then the pinned
/// shared dir so the committed voice models are reused, never re-downloaded.
pub fn resolve_models_dir() -> Option<PathBuf> {
    if let Ok(value) = std::env::var("STARK_MODELS_DIR") {
        let path = PathBuf::from(value);
        if path.is_dir() {
            return Some(path);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for candidate in [
                dir.join("models"),
                dir.join("resources").join("models"),
                dir.join("..").join("models"),
                dir.join("..").join("resources").join("models"),
            ] {
                if candidate.is_dir() {
                    return Some(candidate);
                }
            }
        }
    }
    let pinned = PathBuf::from(r"D:\Stark\resources\models");
    if pinned.is_dir() {
        return Some(pinned);
    }
    None
}

/// One open utterance inside [`TurnTracker`].
#[derive(Debug, Clone, Default)]
pub struct ActiveTurn {
    pub speech_ms: u64,
    pub silence_ms: u64,
    pub total_ms: u64,
    pub last_partial: String,
}

/// What the worker must do after observing one capture frame.
#[derive(Debug)]
pub enum TrackerEvent {
    /// Silence with no open turn (room learning only).
    Silence,
    /// Speech started: open a stream and feed `backlog` (pre-roll plus the
    /// triggering frame) before any further frames.
    SpeechStarted { level: f32, backlog: Vec<Vec<f32>> },
    /// Turn open: feed `frame` to the stream (and poll on the worker's timer).
    SpeechContinue { frame: Vec<f32> },
    /// Endpoint reached: append the silence tail and finalize.
    TurnEnded,
}

/// Pure VAD + endpointing state machine (mirrors the backend worker loop).
/// Moonshine I/O stays in the worker so this is unit-testable without a
/// model; `frame_ms` is derived from the frame length at [`SAMPLE_RATE`].
#[derive(Debug)]
pub struct TurnTracker {
    cfg: TurnConfig,
    noise_floor: f32,
    pre_roll: VecDeque<Vec<f32>>,
    turn: Option<ActiveTurn>,
}

impl TurnTracker {
    pub fn new(cfg: TurnConfig) -> Self {
        Self {
            cfg,
            noise_floor: 0.003,
            pre_roll: VecDeque::new(),
            turn: None,
        }
    }

    pub fn in_turn(&self) -> bool {
        self.turn.is_some()
    }

    /// Record freshly decoded interim text (drives the mid-thought extension).
    pub fn note_partial(&mut self, text: String) {
        if let Some(turn) = self.turn.as_mut() {
            turn.last_partial = text;
        }
    }

    /// Drop an open turn without finalizing (stream errors).
    pub fn abort_turn(&mut self) {
        self.turn = None;
    }

    pub fn observe(&mut self, frame: Vec<f32>, frame_ms: u64) -> TrackerEvent {
        if frame.is_empty() {
            return TrackerEvent::Silence;
        }
        let rms = rms_f32(&frame);
        let threshold = self.cfg.min_level.max(self.noise_floor * 2.5);
        let voiced = rms >= threshold;

        if let Some(turn) = self.turn.as_mut() {
            turn.total_ms += frame_ms;
            if voiced {
                turn.speech_ms += frame_ms;
                turn.silence_ms = 0;
            } else {
                turn.silence_ms += frame_ms;
            }
            if should_finish_turn(
                &self.cfg,
                turn.speech_ms,
                turn.silence_ms,
                turn.total_ms,
                &turn.last_partial.clone(),
            ) {
                self.turn = None;
                return TrackerEvent::TurnEnded;
            }
            return TrackerEvent::SpeechContinue { frame };
        }

        if voiced {
            let mut backlog: Vec<Vec<f32>> = self.pre_roll.drain(..).collect();
            backlog.push(frame);
            self.turn = Some(ActiveTurn {
                speech_ms: frame_ms,
                silence_ms: 0,
                total_ms: frame_ms,
                last_partial: String::new(),
            });
            return TrackerEvent::SpeechStarted {
                level: rms,
                backlog,
            };
        }

        // Learn the room while idle (same constants as the backend).
        self.noise_floor = (self.noise_floor * 0.98 + rms * 0.02).clamp(0.0005, 0.03);
        self.pre_roll.push_back(frame);
        while self.pre_roll.len() > PRE_ROLL_FRAMES {
            self.pre_roll.pop_front();
        }
        TrackerEvent::Silence
    }
}

/// Average stereo pairs into mono (the 16 kHz capture prefers mono configs;
/// this is the stereo fallback so no device is meter-only for channels).
pub fn mono_mix(input: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 || input.is_empty() {
        return input.to_vec();
    }
    input
        .chunks_exact(channels as usize)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// Milliseconds covered by a frame at the streaming rate.
pub fn frame_ms_for(samples: usize) -> u64 {
    ((samples as u64 * 1000) / SAMPLE_RATE as u64).max(1)
}

/// The loaded Moonshine transcriber (MSVC + `voice-stt` only).
#[cfg(all(target_env = "msvc", feature = "voice-stt"))]
pub struct SttEngine {
    pub(crate) transcriber: moonshine_rs::Transcriber,
}

#[cfg(all(target_env = "msvc", feature = "voice-stt"))]
impl SttEngine {
    pub fn load(models_dir: &Path) -> Result<Self, String> {
        let dir = stt_model_dir(models_dir);
        if let Err(missing) = validate_model_dir(&dir) {
            return Err(format!(
                "medium-streaming model missing in {}: {}",
                dir.display(),
                missing.join(", ")
            ));
        }
        let transcriber = moonshine_rs::Transcriber::from_files(
            &dir,
            moonshine_rs::ModelArch::MediumStreaming,
            None,
        )
        .map_err(|e| format!("failed to load medium-streaming model: {e}"))?;
        let keyterms = std::env::var("STARK_STT_KEYTERMS").unwrap_or_else(|_| "Stark".to_string());
        if !keyterms.trim().is_empty() {
            transcriber
                .set_keyterms(keyterms.trim())
                .map_err(|e| format!("keyterms rejected: {e}"))?;
        }
        Ok(Self { transcriber })
    }
}

/// Honest stub everywhere the prebuilt cannot link (mirrors the backend's
/// MSVC gate): meter-only capture keeps working, dictation reports why.
#[cfg(not(all(target_env = "msvc", feature = "voice-stt")))]
pub struct SttEngine;

#[cfg(not(all(target_env = "msvc", feature = "voice-stt")))]
impl SttEngine {
    pub fn load(_models_dir: &Path) -> Result<Self, String> {
        Err("on-device dictation needs an MSVC build with --features voice-stt".to_string())
    }
}

static ENGINE: std::sync::OnceLock<std::sync::Arc<SttEngine>> = std::sync::OnceLock::new();

/// Load once per process (the medium model is ~190 MB); fast validation runs
/// on every call so a missing dir fails without the slow load.
#[cfg(all(target_env = "msvc", feature = "voice-stt"))]
pub fn ensure_engine() -> Result<std::sync::Arc<SttEngine>, String> {
    if let Some(engine) = ENGINE.get() {
        return Ok(engine.clone());
    }
    let models_dir =
        resolve_models_dir().ok_or_else(|| "voice models dir not found".to_string())?;
    let engine = std::sync::Arc::new(SttEngine::load(&models_dir)?);
    let _ = ENGINE.set(engine.clone());
    Ok(engine)
}

#[cfg(not(all(target_env = "msvc", feature = "voice-stt")))]
pub fn ensure_engine() -> Result<std::sync::Arc<SttEngine>, String> {
    Err("on-device dictation needs an MSVC build with --features voice-stt".to_string())
}

/// Start the producer worker: VAD + endpointing over `pcm_rx` (16 kHz mono),
/// interim/final text into `tx`, until `stop` flips or the channel drops.
/// Mirrors the backend's `run_session` (pre-roll, 60 ms poll, silence tail).
pub fn spawn_producer(
    pcm_rx: Receiver<Vec<f32>>,
    tx: Sender<VoiceMsg>,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    #[cfg(all(target_env = "msvc", feature = "voice-stt"))]
    {
        let engine = ensure_engine()?;
        std::thread::Builder::new()
            .name("stark-stt-producer".to_string())
            .spawn(move || run_producer(pcm_rx, tx, stop, engine))
            .map(|_| ())
            .map_err(|e| format!("failed to spawn STT worker: {e}"))
    }
    #[cfg(not(all(target_env = "msvc", feature = "voice-stt")))]
    {
        let _ = (pcm_rx, tx, stop);
        Err("on-device dictation needs an MSVC build with --features voice-stt".to_string())
    }
}

#[cfg(all(target_env = "msvc", feature = "voice-stt"))]
fn run_producer(
    pcm_rx: Receiver<Vec<f32>>,
    tx: Sender<VoiceMsg>,
    stop: Arc<AtomicBool>,
    engine: Arc<SttEngine>,
) {
    use std::sync::mpsc::RecvTimeoutError;

    let cfg = TurnConfig::from_env();
    let mut tracker = TurnTracker::new(cfg);
    let mut stream: Option<moonshine_rs::OwnedTranscriberStream> = None;
    let mut last_poll = Instant::now();

    let send_error = |tx: &Sender<VoiceMsg>, message: String| {
        let _ = tx.send(VoiceMsg::Error(message));
    };

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match pcm_rx.recv_timeout(POLL_INTERVAL) {
            Ok(frame) => {
                let frame_ms = frame_ms_for(frame.len());
                match tracker.observe(frame, frame_ms) {
                    TrackerEvent::Silence => {}
                    TrackerEvent::SpeechStarted { level: _, backlog } => {
                        let mut fresh = match engine.transcriber.create_owned_stream() {
                            Ok(fresh) => fresh,
                            Err(error) => {
                                send_error(&tx, format!("cannot start STT stream: {error}"));
                                tracker.abort_turn();
                                continue;
                            }
                        };
                        let mut failed = false;
                        for buffered in &backlog {
                            if let Err(error) = fresh.add_audio(buffered, SAMPLE_RATE) {
                                send_error(&tx, format!("STT pre-roll failed: {error}"));
                                failed = true;
                                break;
                            }
                        }
                        if failed {
                            tracker.abort_turn();
                            continue;
                        }
                        stream = Some(fresh);
                    }
                    TrackerEvent::SpeechContinue { frame } => {
                        if let Some(active) = stream.as_mut() {
                            if let Err(error) = active.add_audio(&frame, SAMPLE_RATE) {
                                send_error(&tx, format!("STT add_audio failed: {error}"));
                                tracker.abort_turn();
                                stream = None;
                            }
                        }
                    }
                    TrackerEvent::TurnEnded => {
                        if let Some(mut active) = stream.take() {
                            let silence = vec![0.0f32; (SAMPLE_RATE / 5) as usize];
                            let _ = active.add_audio(&silence, SAMPLE_RATE);
                            match active.finalize() {
                                Ok(transcript) => {
                                    let text = transcript.text().trim().replace('\n', " ");
                                    if !text.is_empty() {
                                        let _ = tx.send(VoiceMsg::Transcript {
                                            text,
                                            is_final: true,
                                        });
                                    }
                                }
                                Err(error) => {
                                    send_error(&tx, format!("STT finalize failed: {error}"));
                                }
                            }
                        }
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        if last_poll.elapsed() >= POLL_INTERVAL {
            last_poll = Instant::now();
            if let Some(active) = stream.as_mut() {
                match active.poll(false) {
                    Ok(transcript) => {
                        let text = transcript.text().trim().replace('\n', " ");
                        if !text.is_empty() {
                            // Only changed text crosses the channel (the live
                            // bubble overwrites in place per update).
                            let changed = tracker
                                .turn
                                .as_ref()
                                .is_some_and(|t| t.last_partial != text);
                            if changed {
                                tracker.note_partial(text.clone());
                                let _ = tx.send(VoiceMsg::Transcript {
                                    text,
                                    is_final: false,
                                });
                            }
                        }
                    }
                    Err(error) => {
                        send_error(&tx, format!("STT poll failed: {error}"));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mid_thought_words_extend_the_endpoint() {
        assert!(ends_mid_thought("remind me to"));
        assert!(ends_mid_thought("Call the"));
        assert!(!ends_mid_thought("remind me tomorrow"));
        assert!(!ends_mid_thought(""));
    }

    #[test]
    fn config_defaults_match_the_backend() {
        let cfg = TurnConfig::default();
        assert_eq!(cfg.endpoint_ms, 900);
        assert_eq!(cfg.min_speech_ms, 300);
        assert_eq!(cfg.max_turn_ms, 30_000);
        assert_eq!(cfg.extended_endpoint_ms, 1_800);
        assert!((cfg.min_level - 0.008).abs() < f32::EPSILON);
        // from_env without overrides resolves to the same defaults.
        let loaded = TurnConfig::from_env();
        let unset = [
            "VOICE_ENDPOINT_MS",
            "VOICE_MIN_SPEECH_MS",
            "VOICE_MAX_TURN_MS",
            "VOICE_ENDPOINT_EXTENDED_MS",
            "VOICE_MIN_LEVEL",
        ]
        .iter()
        .all(|key| std::env::var(key).is_err());
        if unset {
            assert_eq!(loaded.endpoint_ms, cfg.endpoint_ms);
            assert_eq!(loaded.min_speech_ms, cfg.min_speech_ms);
        }
    }

    #[test]
    fn finish_matrix_covers_speech_silence_and_cap() {
        let cfg = TurnConfig::default();
        // Too little speech never ends, even after a long pause.
        assert!(!should_finish_turn(&cfg, 100, 5_000, 5_100, ""));
        // Enough speech + endpoint silence ends the turn.
        assert!(should_finish_turn(&cfg, 500, 900, 1_400, "done now"));
        // A continuation word waits for the extended gap.
        assert!(!should_finish_turn(&cfg, 500, 900, 1_400, "remind me to"));
        assert!(should_finish_turn(&cfg, 500, 1_800, 2_300, "remind me to"));
        // The hard cap always wins.
        assert!(should_finish_turn(&cfg, 500, 0, 30_000, ""));
    }

    fn loud_frame(samples: usize) -> Vec<f32> {
        vec![0.5; samples]
    }

    fn quiet_frame(samples: usize) -> Vec<f32> {
        vec![0.0001; samples]
    }

    #[test]
    fn tracker_opens_closes_and_keeps_preroll() {
        let cfg = TurnConfig::default();
        let mut tracker = TurnTracker::new(cfg);
        // 30 quiet frames: room learning + pre-roll fill, no turn.
        for _ in 0..30 {
            assert!(matches!(
                tracker.observe(quiet_frame(960), 60),
                TrackerEvent::Silence
            ));
        }
        assert!(!tracker.in_turn());
        // Voiced frame opens a turn carrying the pre-roll backlog.
        match tracker.observe(loud_frame(960), 60) {
            TrackerEvent::SpeechStarted { level, backlog } => {
                assert!(level > 0.1);
                assert_eq!(backlog.len(), PRE_ROLL_FRAMES + 1, "pre-roll plus trigger");
            }
            other => panic!("expected SpeechStarted, got {other:?}"),
        }
        assert!(tracker.in_turn());
        // 300ms of speech then 900ms of silence ends the turn.
        for _ in 0..5 {
            assert!(matches!(
                tracker.observe(loud_frame(960), 60),
                TrackerEvent::SpeechContinue { .. }
            ));
        }
        for i in 0..15 {
            let event = tracker.observe(quiet_frame(960), 60);
            if i < 14 {
                assert!(
                    matches!(event, TrackerEvent::SpeechContinue { .. }),
                    "still open at {i}"
                );
            } else {
                assert!(matches!(event, TrackerEvent::TurnEnded), "endpoint fires");
            }
        }
        assert!(!tracker.in_turn());
    }

    #[test]
    fn partial_text_drives_the_mid_thought_extension() {
        let cfg = TurnConfig::default();
        let mut tracker = TurnTracker::new(cfg);
        assert!(matches!(
            tracker.observe(loud_frame(960), 60),
            TrackerEvent::SpeechStarted { .. }
        ));
        for _ in 0..5 {
            let _ = tracker.observe(loud_frame(960), 60);
        }
        tracker.note_partial("call the".to_string());
        // 900ms of silence is not enough mid-thought (extended 1800ms).
        for i in 0..15 {
            let event = tracker.observe(quiet_frame(960), 60);
            assert!(
                matches!(event, TrackerEvent::SpeechContinue { .. }),
                "held open at {i}"
            );
        }
        // 900ms more crosses the extended gap.
        for i in 0..14 {
            let _ = tracker.observe(quiet_frame(960), 60);
            let _ = i;
        }
        assert!(matches!(
            tracker.observe(quiet_frame(960), 60),
            TrackerEvent::TurnEnded
        ));
    }

    #[test]
    fn validate_model_dir_reports_missing_files() {
        let dir = std::env::temp_dir().join("stark-stt-missing-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let Err(missing) = validate_model_dir(&dir) else {
            panic!("empty dir must fail validation");
        };
        assert_eq!(missing.len(), REQUIRED_FILES.len());
        for file in REQUIRED_FILES {
            std::fs::write(dir.join(file), b"").expect("stub file");
        }
        assert!(validate_model_dir(&dir).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mono_mix_averages_pairs_and_passes_mono_through() {
        assert_eq!(mono_mix(&[0.5, -0.5, 1.0], 1), vec![0.5, -0.5, 1.0]);
        assert_eq!(mono_mix(&[1.0, 3.0, 5.0, 7.0], 2), vec![2.0, 6.0]);
        assert!(mono_mix(&[], 2).is_empty());
    }

    #[test]
    fn frame_ms_scales_with_samples() {
        assert_eq!(frame_ms_for(960), 60);
        assert_eq!(frame_ms_for(16_000), 1000);
        assert_eq!(frame_ms_for(0), 1);
    }

    /// Non-MSVC (or non-`voice-stt`) builds keep the honest stub: no worker
    /// is spawned and the error names the MSVC requirement instead of
    /// failing obscurely. This is what the manifest's
    /// `target.'cfg(target_env = "msvc")'` gate guarantees on GNU.
    #[cfg(not(all(target_env = "msvc", feature = "voice-stt")))]
    #[test]
    fn stub_names_the_msvc_requirement() {
        let err = match ensure_engine() {
            Err(e) => e,
            Ok(_) => panic!("no prebuilt on this target"),
        };
        assert!(err.contains("MSVC"), "honest message, got: {err}");
        let (_pcm_tx, pcm_rx) = std::sync::mpsc::channel();
        let (msg_tx, _msg_rx) = std::sync::mpsc::channel();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let err = match spawn_producer(pcm_rx, msg_tx, stop) {
            Err(e) => e,
            Ok(_) => panic!("stub must not spawn"),
        };
        assert!(err.contains("MSVC"), "honest message, got: {err}");
    }
}
