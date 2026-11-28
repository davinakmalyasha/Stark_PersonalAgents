//! Silero VAD v6.2 through transcribe.cpp's VAD role, following Hex's
//! `transcription/speech_detector.rs`.
//!
//! The detector is optional: while its GGUF is missing every caller keeps its
//! level-only behavior. It runs on the CPU so a first call never waits on the
//! Vulkan model, and the weights live only as long as the `SttService` that
//! owns this detector.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use transcribe_cpp::{Backend, Model, ModelOptions, VadOptions, VadSession};

pub const VAD_FILE: &str = "silero-vad-v6.2-F32.gguf";
const VAD_DIR: &str = "silero-vad-v6.2";

/// Speech shorter than this is ignored. Silero's default of 250 ms drops a
/// short "Ok" or "Ciao"; 160 ms keeps those and still rejects clicks, keys
/// and room noise.
const MIN_SPEECH_MS: i32 = 160;

/// Speech probability that counts as speech. Silero's default of 0.5 misses
/// a whisper at -43 dBFS, which the level gate deliberately keeps; 0.4 hears
/// it and still rejects the silence in a recorded meeting.
pub const THRESHOLD: f64 = 0.4;

/// A lazily loaded Silero detector. Failures disable detection for this
/// process instead of failing dictation; the level gate still runs.
pub struct SpeechDetector {
    path: PathBuf,
    session: Mutex<Option<VadSession>>,
    disabled: AtomicBool,
}

impl SpeechDetector {
    /// Point at the installed detector. Nothing is loaded until the first
    /// clip needs it, so an idle `SttService` holds no VAD weights.
    pub fn new(models_dir: &Path) -> Self {
        Self {
            path: model_path(models_dir),
            session: Mutex::new(None),
            disabled: AtomicBool::new(false),
        }
    }

    pub fn is_installed(&self) -> bool {
        self.path.is_file()
    }

    /// Whether `samples` contain speech, or `None` when the detector is not
    /// installed or failed, and the caller should keep its current behavior.
    pub fn contains_speech(&self, samples: &[f32]) -> Option<bool> {
        if samples.is_empty() {
            return Some(false);
        }
        if self.disabled.load(Ordering::Relaxed) {
            return None;
        }
        // A dictation turn is at most 30 s; a file can be longer. Score long
        // clips in 30 s windows so the detector's probability buffer stays
        // bounded, and accept as soon as one window hears speech.
        const WINDOW_SAMPLES: usize = 30 * 16_000;
        if samples.len() > WINDOW_SAMPLES {
            for window in samples.chunks(WINDOW_SAMPLES) {
                match self.contains_speech(window) {
                    Some(true) => return Some(true),
                    Some(false) => {}
                    None => return None,
                }
            }
            return Some(false);
        }
        let mut session = self
            .session
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if session.is_none() {
            if !self.path.is_file() {
                return None;
            }
            match load(&self.path) {
                Ok(loaded) => *session = Some(loaded),
                Err(error) => {
                    self.disabled.store(true, Ordering::Relaxed);
                    log::warn!(
                        "Silero VAD unavailable at {} ({error:#}); using the level gate only",
                        self.path.display()
                    );
                    return None;
                }
            }
        }
        let session = session.as_mut()?;
        let options = VadOptions {
            threshold: THRESHOLD,
            min_speech_ms: MIN_SPEECH_MS,
            ..VadOptions::default()
        };
        match session.run(samples, &options) {
            Ok(result) => Some(!result.segments.is_empty()),
            Err(error) => {
                self.disabled.store(true, Ordering::Relaxed);
                log::warn!("Silero VAD failed ({error:#}); using the level gate only");
                None
            }
        }
    }
}

/// Loads the detector on the CPU: it loads in tens of milliseconds there,
/// while a first Vulkan start can take seconds and would hold up dictation.
fn load(path: &Path) -> Result<VadSession> {
    let model = Model::load_with(
        path,
        &ModelOptions {
            backend: Backend::Cpu,
            ..ModelOptions::default()
        },
    )
    .with_context(|| format!("cannot load Silero VAD {}", path.display()))?;
    model.vad_session().context("Silero VAD session")
}

/// `STARK_VAD_MODEL`, then the shared `stt` install layouts, then the models
/// root itself. The returned path is the primary install location when none
/// exists, so the error names where the installer puts it.
pub fn model_path(models_dir: &Path) -> PathBuf {
    if let Ok(value) = std::env::var("STARK_VAD_MODEL") {
        let path = PathBuf::from(value);
        if path.is_file() {
            return path;
        }
        log::warn!(
            "STARK_VAD_MODEL points at a missing file: {}",
            path.display()
        );
    }
    let flat = models_dir.join("stt").join(VAD_FILE);
    let nested = models_dir.join("stt").join(VAD_DIR).join(VAD_FILE);
    let root = models_dir.join(VAD_FILE);
    [flat.clone(), nested, root]
        .into_iter()
        .find(|candidate| candidate.is_file())
        .unwrap_or(flat)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_detector_is_optional() {
        let dir = std::env::temp_dir().join(format!("stark-vad-missing-{}", std::process::id()));
        let detector = SpeechDetector::new(&dir);
        assert!(!detector.is_installed());
        assert_eq!(detector.contains_speech(&[0.1; 1_600]), None);
        assert_eq!(
            model_path(&dir),
            dir.join("stt").join(VAD_FILE),
            "the error names the installer's path"
        );
    }

    #[test]
    fn an_installed_layout_is_found() {
        let dir = std::env::temp_dir().join(format!("stark-vad-layout-{}", std::process::id()));
        let nested = dir.join("stt").join(VAD_DIR);
        std::fs::create_dir_all(&nested).unwrap();
        let file = nested.join(VAD_FILE);
        std::fs::write(&file, b"test").unwrap();
        assert_eq!(model_path(&dir), file);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn speech_clip(variable: &str) -> Vec<f32> {
        let mut reader = hound::WavReader::open(std::env::var(variable).unwrap()).unwrap();
        let spec = reader.spec();
        let samples: Vec<f32> = reader
            .samples::<i16>()
            .map(|sample| sample.unwrap() as f32 / 32_768.0)
            .collect();
        if spec.channels > 1 {
            return samples
                .chunks(spec.channels as usize)
                .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
                .collect();
        }
        samples
    }

    #[test]
    #[ignore = "requires STARK_MODELS_DIR with Silero VAD and STARK_VAD_SPEECH_WAV"]
    fn real_detector_hears_speech_and_rejects_silence() {
        let models = std::env::var("STARK_MODELS_DIR").unwrap();
        let detector = SpeechDetector::new(Path::new(&models));
        assert!(detector.is_installed(), "install silero-vad-v6.2-F32.gguf");
        assert_eq!(
            detector.contains_speech(&vec![0.0; 32_000]),
            Some(false),
            "digital silence is not speech"
        );
        let speech = speech_clip("STARK_VAD_SPEECH_WAV");
        assert_eq!(
            detector.contains_speech(&speech),
            Some(true),
            "the fixture holds speech"
        );
    }
}
