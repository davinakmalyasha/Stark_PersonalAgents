//! Parakeet Unified English STT, following Hex's transcription path.
//!
//! One GGUF model is loaded once and shared through cheap `Model` clones.
//! Every turn gets its own session and stream; a buffered Parakeet stream is
//! used only on a GPU, because a CPU stream re-encodes an overlapping window
//! for every chunk. A turn's complete PCM is kept and transcribed offline if
//! the stream fails, returns no text, loses its opening/trailing speech, or
//! drops the closing punctuation.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use tokio::sync::mpsc::UnboundedSender;
use transcribe_cpp::{
    Backend, DeviceType, Model, ModelOptions, ParakeetBufferedStreamOptions,
    ParakeetVocabularyOptions, RunExtension, RunOptions, StreamExtension, StreamOptions, Task,
    TimestampKind,
};

use crate::LiveEvent;

use super::decode::load_audio_16k;
use super::silence::{SAMPLE_RATE as GATE_SAMPLE_RATE, level_gate};
use super::speech::{PauseCap, UNIFIED_TRAILING_SILENCE, check_closing_mark, pad_clip};
use super::vad::SpeechDetector;
use super::vocabulary::Vocabulary;

pub const MODEL_DIR: &str = "parakeet-unified-en-0.6b";
pub const MODEL_FILE: &str = "parakeet-unified-en-0.6b-Q8_0.gguf";
pub const MODEL_NAME: &str = "parakeet-unified-en-0.6b";
const STREAM_LEFT_MS: i32 = 5_600;
const STREAM_CHUNK_MS: i32 = 1_040;
const STREAM_RIGHT_MS: i32 = 1_040;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SttBackend {
    Gpu,
    Cpu,
}

impl SttBackend {
    pub fn label(self) -> &'static str {
        match self {
            Self::Gpu => "vulkan",
            Self::Cpu => "cpu",
        }
    }
}

/// The loaded model and its capability snapshot. `Model` is Arc-backed and
/// cheap to clone; sessions are created per turn.
pub struct SttService {
    path: PathBuf,
    model: Mutex<Option<Model>>,
    load_lock: Mutex<()>,
    name: String,
    backend: SttBackend,
    device: String,
    can_stream: bool,
    /// Optional Silero VAD, loaded on its first clip and dropped with this
    /// service so an idle server holds no speech weights.
    vad: SpeechDetector,
}

impl SttService {
    /// Load the Parakeet model. The caller constructs this only when live
    /// mode needs ASR, so an idle server holds no speech weights.
    pub fn load(models_dir: &Path) -> Result<Self> {
        let path = model_path(models_dir)?;
        let vad = SpeechDetector::new(models_dir);
        if !vad.is_installed() {
            log::warn!(
                "Silero VAD model missing under {}; dictation uses the level gate only",
                models_dir.display()
            );
        }
        let model = load_model(&path)?;
        let capabilities = model.capabilities().context("Parakeet capabilities")?;
        let device = model.device().ok();
        let device_type = device.as_ref().map(|device| device.device_type);
        let backend = match device_type {
            Some(DeviceType::Gpu | DeviceType::Igpu) => SttBackend::Gpu,
            _ => SttBackend::Cpu,
        };
        let device = device
            .map(|device| device.name)
            .unwrap_or_else(|| backend.label().to_string());
        let can_stream = capabilities.supports_streaming && backend == SttBackend::Gpu;
        log::info!(
            "Parakeet Unified English loaded from {} (backend={}, device={}, stream={}, arch={})",
            path.display(),
            model.backend(),
            device,
            can_stream,
            model.arch()
        );
        Ok(Self {
            path,
            model: Mutex::new(Some(model)),
            load_lock: Mutex::new(()),
            name: MODEL_NAME.to_string(),
            backend,
            device,
            can_stream,
            vad,
        })
    }

    pub fn model_name(&self) -> &str {
        &self.name
    }

    pub fn backend(&self) -> SttBackend {
        self.backend
    }

    pub fn backend_label(&self) -> &'static str {
        self.backend.label()
    }

    pub fn device(&self) -> &str {
        &self.device
    }

    pub fn can_stream(&self) -> bool {
        self.can_stream
    }

    /// Whether the Silero detector is installed; without it the level gate
    /// still rejects near-silent clips.
    pub fn vad_installed(&self) -> bool {
        self.vad.is_installed()
    }

    /// Load the model if `release` dropped it.
    fn ensure_model(&self) -> Result<Model> {
        if let Some(model) = self
            .model
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
        {
            return Ok(model);
        }
        let _guard = self
            .load_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(model) = self
            .model
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
        {
            return Ok(model);
        }
        let model = load_model(&self.path)?;
        *self.model.lock().unwrap_or_else(|error| error.into_inner()) = Some(model.clone());
        Ok(model)
    }

    /// Drop the weights. A turn already holding a `Model` clone keeps them
    /// alive until it finishes; the next turn reloads them.
    pub fn release(&self) -> bool {
        let mut model = self.model.lock().unwrap_or_else(|error| error.into_inner());
        model.take().is_some()
    }

    pub fn is_loaded(&self) -> bool {
        self.model
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_some()
    }

    /// Warm the model and its stream graph without opening a capture.
    pub fn warm(&self) -> Result<()> {
        let model = self.ensure_model()?;
        if self.can_stream {
            let mut session = model.session()?;
            let options = stream_options();
            if let Ok(mut stream) = session.stream(&run_options(&Vocabulary::default()), &options) {
                let _ = stream.feed(&vec![0.0; 1_600]);
                let _ = stream.finalize();
            }
        }
        Ok(())
    }

    /// Whether a whole capture is worth transcribing: Hex's level gate first,
    /// then Silero when it is installed. A missing detector keeps the
    /// level-only decision; a detector that says "no speech" drops the clip
    /// even when its level passed (a click, a fan, music).
    pub fn accepts_speech(&self, samples: &[f32]) -> bool {
        let gate = level_gate(samples);
        if !gate.passes() {
            log::info!(
                "discarded {:.0} ms of near-silent audio (level {:.1} dBFS, floor {}, sustained {} ms)",
                samples.len() as f64 * 1_000.0 / GATE_SAMPLE_RATE as f64,
                gate.loudest_dbfs(),
                gate.noise_floor_dbfs()
                    .map(|floor| format!("{floor:.1} dBFS"))
                    .unwrap_or_else(|| "none".to_string()),
                gate.sustained_ms()
            );
            return false;
        }
        match self.vad.contains_speech(samples) {
            Some(false) => {
                log::info!(
                    "discarded {:.0} ms without detected speech (level {:.1} dBFS)",
                    samples.len() as f64 * 1_000.0 / GATE_SAMPLE_RATE as f64,
                    gate.loudest_dbfs()
                );
                false
            }
            Some(true) | None => true,
        }
    }

    /// Decode and transcribe a file. `Ok(None)` means the gate found no
    /// speech, which the API reports as an expected empty clip rather than a
    /// model failure.
    pub fn transcribe_file(&self, path: &Path, vocabulary: &Vocabulary) -> Result<Option<String>> {
        let samples = load_audio_16k(path)?;
        if !self.accepts_speech(&samples) {
            return Ok(None);
        }
        self.transcribe_pcm(&samples, vocabulary).map(Some)
    }

    pub fn transcribe_pcm(&self, samples: &[f32], vocabulary: &Vocabulary) -> Result<String> {
        let model = self.ensure_model()?;
        let mut session = model.session().context("Parakeet session")?;
        let clip = pad_clip(samples.to_vec());
        let transcript = session
            .run(&clip, &run_options(vocabulary))
            .context("Parakeet transcription")?;
        Ok(transcript.text.trim().to_string())
    }

    /// Start one turn. The thread owns its session/stream and sends partials
    /// and the final to `events`. The caller's complete PCM stays in
    /// `LiveSession` for the offline fallback.
    pub fn start_turn(
        self: &Arc<Self>,
        events: UnboundedSender<LiveEvent>,
        turn_id: String,
        vocabulary: Vocabulary,
    ) -> Result<TurnHandle> {
        let (tx, rx) = mpsc::sync_channel(4_000);
        let stt = self.clone();
        let thread_id = turn_id.clone();
        let thread = thread::Builder::new()
            .name("stark-parakeet-turn".to_string())
            .spawn(move || run_turn(stt, rx, events, thread_id, vocabulary))
            .context("failed to spawn Parakeet turn")?;
        Ok(TurnHandle {
            turn_id,
            tx,
            thread: Some(thread),
        })
    }
}

pub struct TurnHandle {
    turn_id: String,
    tx: SyncSender<TurnCommand>,
    thread: Option<JoinHandle<()>>,
}

impl TurnHandle {
    pub fn turn_id(&self) -> &str {
        &self.turn_id
    }

    pub fn feed(&self, samples: Vec<f32>) -> bool {
        if samples.is_empty() {
            return true;
        }
        match self.tx.try_send(TurnCommand::Feed(samples)) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => false,
            Err(TrySendError::Disconnected(_)) => false,
        }
    }

    /// Finish with the complete turn PCM and wait for the final event.
    pub fn finish(mut self, clip: Vec<f32>) {
        let _ = self.tx.send(TurnCommand::Finish(clip));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    pub fn cancel(mut self) {
        let _ = self.tx.send(TurnCommand::Cancel);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for TurnHandle {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = self.tx.send(TurnCommand::Cancel);
            let _ = thread.join();
        }
    }
}

enum TurnCommand {
    Feed(Vec<f32>),
    Finish(Vec<f32>),
    Cancel,
}

fn run_turn(
    stt: Arc<SttService>,
    rx: Receiver<TurnCommand>,
    events: UnboundedSender<LiveEvent>,
    turn_id: String,
    vocabulary: Vocabulary,
) {
    let started = Instant::now();
    let mut collected: Vec<f32> = Vec::new();
    let mut last_partial = String::new();

    let stream_result = if stt.can_stream() {
        stream_turn(
            &stt,
            rx,
            &events,
            &turn_id,
            &mut collected,
            &mut last_partial,
            &vocabulary,
        )
    } else {
        offline_turn(rx, &mut collected, &mut last_partial)
    };

    let text = match stream_result {
        TurnResult::Final(text) if !text.trim().is_empty() => text.trim().to_string(),
        TurnResult::Final(_) | TurnResult::Fallback => {
            match stt.transcribe_pcm(&collected, &vocabulary) {
                Ok(text) if !text.trim().is_empty() => text.trim().to_string(),
                Ok(_) if !last_partial.trim().is_empty() => last_partial.trim().to_string(),
                Ok(_) => String::new(),
                Err(error) if !last_partial.trim().is_empty() => {
                    log::warn!("Parakeet offline fallback failed: {error:#}");
                    last_partial.trim().to_string()
                }
                Err(error) => {
                    let _ = events.send(LiveEvent::Error {
                        message: format!("Parakeet transcription failed: {error:#}"),
                    });
                    String::new()
                }
            }
        }
        TurnResult::Cancelled => return,
    };
    if !text.is_empty() {
        let _ = events.send(LiveEvent::TurnFinal {
            turn_id,
            text,
            duration_ms: started.elapsed().as_millis() as u64,
        });
    }
}

enum TurnResult {
    Final(String),
    Fallback,
    Cancelled,
}

fn stream_turn(
    stt: &Arc<SttService>,
    rx: Receiver<TurnCommand>,
    events: &UnboundedSender<LiveEvent>,
    turn_id: &str,
    collected: &mut Vec<f32>,
    last_partial: &mut String,
    vocabulary: &Vocabulary,
) -> TurnResult {
    let Ok(model) = stt.ensure_model() else {
        drain_to_finish(rx, collected);
        return TurnResult::Fallback;
    };
    let Ok(mut session) = model.session() else {
        drain_to_finish(rx, collected);
        return TurnResult::Fallback;
    };
    let mut stream = match session.stream(&run_options(vocabulary), &stream_options()) {
        Ok(stream) => stream,
        Err(error) => {
            log::warn!("Parakeet stream begin failed ({error}); this turn will decode offline");
            drain_to_finish(rx, collected);
            return TurnResult::Fallback;
        }
    };
    let mut cap = PauseCap::new(0);
    loop {
        match rx.recv() {
            Ok(TurnCommand::Feed(samples)) => {
                collected.extend_from_slice(&samples);
                let mut out = Vec::new();
                cap.push(&samples, &mut out);
                if out.is_empty() {
                    continue;
                }
                if let Err(error) = stream.feed(&out) {
                    log::warn!("Parakeet feed failed: {error}");
                    drain_to_finish(rx, collected);
                    return TurnResult::Fallback;
                }
                cap.note_sent(out.len());
                let text = stream.text().display();
                let text = text.trim().to_string();
                if !text.is_empty() && text != *last_partial {
                    *last_partial = text.clone();
                    let _ = events.send(LiveEvent::Partial {
                        turn_id: turn_id.to_string(),
                        text,
                    });
                }
            }
            Ok(TurnCommand::Finish(clip)) => {
                collected.clear();
                collected.extend_from_slice(&clip);
                let mut out = Vec::new();
                if let Err(error) = cap.finish(&mut out) {
                    log::warn!("Parakeet pause cap: {error}");
                    return TurnResult::Fallback;
                }
                if !out.is_empty() {
                    if let Err(error) = stream.feed(&out) {
                        log::warn!("Parakeet final feed failed: {error}");
                        return TurnResult::Fallback;
                    }
                    cap.note_sent(out.len());
                }
                if let Err(error) = stream.finalize() {
                    log::warn!("Parakeet finalize failed: {error}");
                    return TurnResult::Fallback;
                }
                let text = stream.text().full.trim().to_string();
                if text.is_empty() {
                    return TurnResult::Fallback;
                }
                let tokens = stream.snapshot().tokens;
                let result = check_closing_mark(&text).and_then(|text| {
                    cap.check_coverage(&tokens)?;
                    Ok(text)
                });
                return match result {
                    Ok(text) => TurnResult::Final(text),
                    Err(reason) => {
                        log::warn!("Parakeet stream unusable: {reason}; using the whole clip");
                        TurnResult::Fallback
                    }
                };
            }
            Ok(TurnCommand::Cancel) | Err(_) => return TurnResult::Cancelled,
        }
    }
}

/// Keep the complete clip even when the stream could not start or failed.
fn drain_to_finish(rx: Receiver<TurnCommand>, collected: &mut Vec<f32>) {
    loop {
        match rx.recv() {
            Ok(TurnCommand::Feed(samples)) => collected.extend_from_slice(&samples),
            Ok(TurnCommand::Finish(clip)) => {
                collected.clear();
                collected.extend_from_slice(&clip);
                return;
            }
            Ok(TurnCommand::Cancel) | Err(_) => return,
        }
    }
}

fn offline_turn(
    rx: Receiver<TurnCommand>,
    collected: &mut Vec<f32>,
    last_partial: &mut String,
) -> TurnResult {
    loop {
        match rx.recv() {
            Ok(TurnCommand::Feed(samples)) => {
                collected.extend_from_slice(&samples);
                *last_partial = String::new();
            }
            Ok(TurnCommand::Finish(clip)) => {
                if collected.is_empty() {
                    collected.extend_from_slice(&clip);
                }
                return TurnResult::Final(String::new());
            }
            Ok(TurnCommand::Cancel) | Err(_) => return TurnResult::Cancelled,
        }
    }
}

fn model_path(models_dir: &Path) -> Result<PathBuf> {
    if let Ok(path) = std::env::var("STARK_STT_MODEL") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        bail!(
            "STARK_STT_MODEL points at a missing file: {}",
            path.display()
        );
    }
    let dir = models_dir.join("stt").join(MODEL_DIR);
    let exact = dir.join(MODEL_FILE);
    if exact.is_file() {
        return Ok(exact);
    }
    let found = std::fs::read_dir(&dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|value| value == "gguf"));
    found.ok_or_else(|| {
        anyhow::anyhow!(
            "Parakeet Unified English model missing at {}; install {}",
            exact.display(),
            MODEL_FILE
        )
    })
}

fn load_model(path: &Path) -> Result<Model> {
    let auto = ModelOptions::default();
    match Model::load_with(path, &auto) {
        Ok(model) => Ok(model),
        Err(error) => {
            log::warn!(
                "Parakeet auto backend failed ({error}); retrying on CPU: {}",
                path.display()
            );
            Model::load_with(
                path,
                &ModelOptions {
                    backend: Backend::Cpu,
                    ..ModelOptions::default()
                },
            )
            .with_context(|| format!("cannot load Parakeet model {}", path.display()))
        }
    }
}

fn run_options(vocabulary: &Vocabulary) -> RunOptions {
    let mut options = RunOptions {
        task: Task::Transcribe,
        timestamps: TimestampKind::Token,
        language: Some("en".to_string()),
        ..RunOptions::default()
    };
    if let Some(bias) = vocabulary.bias_parts() {
        options.vocabulary = bias.terms;
        options.family = Some(RunExtension::ParakeetVocabulary(
            ParakeetVocabularyOptions {
                boost: Some(bias.boost),
                sounds_like: bias.sounds_like,
            },
        ));
    }
    options
}

fn stream_options() -> StreamOptions {
    StreamOptions {
        family: Some(StreamExtension::ParakeetBuffered(
            ParakeetBufferedStreamOptions {
                left_ms: Some(STREAM_LEFT_MS),
                chunk_ms: Some(STREAM_CHUNK_MS),
                right_ms: Some(STREAM_RIGHT_MS),
            },
        )),
        ..StreamOptions::default()
    }
}

/// Unified English's trailing silence, exposed for tests and status.
pub const TRAILING_SILENCE_SAMPLES: usize = UNIFIED_TRAILING_SILENCE;

#[cfg(test)]
mod tests {
    use super::super::vocabulary::{VocabularyBoost, VocabularyEntry};
    use super::*;

    #[test]
    fn model_path_names_the_bundled_file() {
        let dir = PathBuf::from("models");
        let expected = dir.join("stt").join(MODEL_DIR).join(MODEL_FILE);
        assert_eq!(
            model_path(&dir).unwrap_err().to_string(),
            format!(
                "Parakeet Unified English model missing at {}; install {}",
                expected.display(),
                MODEL_FILE
            )
        );
    }

    #[test]
    fn stream_geometry_is_hexs_faster_live_text() {
        let options = stream_options();
        let Some(StreamExtension::ParakeetBuffered(options)) = options.family else {
            panic!("parakeet buffered");
        };
        assert_eq!(options.left_ms, Some(5_600));
        assert_eq!(options.chunk_ms, Some(1_040));
        assert_eq!(options.right_ms, Some(1_040));
    }

    #[test]
    fn vocabulary_reaches_the_parakeet_run_extension() {
        let vocabulary = Vocabulary {
            entries: vec![
                VocabularyEntry {
                    term: "OpenCode".into(),
                    sounds_like: vec!["open code".into()],
                },
                VocabularyEntry {
                    term: "Rune".into(),
                    sounds_like: Vec::new(),
                },
            ],
            boost: VocabularyBoost::Strong,
        };
        let options = run_options(&vocabulary);
        assert_eq!(options.vocabulary, ["OpenCode", "Rune"]);
        let Some(RunExtension::ParakeetVocabulary(bias)) = options.family else {
            panic!("parakeet vocabulary extension");
        };
        assert_eq!(bias.boost, Some(4.0));
        assert_eq!(bias.sounds_like, [(0, "open code".to_string())]);
    }

    #[test]
    fn an_empty_vocabulary_adds_no_decoder_work() {
        let options = run_options(&Vocabulary::default());
        assert!(options.vocabulary.is_empty());
        assert!(options.family.is_none());
    }
}
