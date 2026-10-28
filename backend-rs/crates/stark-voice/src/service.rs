use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{Context, Result, bail};
use tokio::sync::mpsc;

use crate::tts::{Config, LoadOptions, TtsModel, VoiceState};
use crate::wav;

/// The CPU backend used by Pocket TTS. The 18M model is small enough that f32 weights are
/// cheaper than the quality risk of int8.
pub type F32Backend = xn::Unquantized<f32, xn::CpuDevice>;
pub type PocketModel = TtsModel<F32Backend>;

const MAX_TOKENS_PER_CHUNK: usize = 50;
const FRAMES_AFTER_EOS: usize = 2;
const DEFAULT_VOICE: &str = "jarvis";
const FALLBACK_VOICE: &str = "alba";

#[derive(Debug)]
pub enum TtsEvent {
    Samples(Vec<f32>),
    Done { cancelled: bool },
    Error(String),
}

pub struct TtsStream {
    pub rx: mpsc::Receiver<TtsEvent>,
    pub cancel: Arc<AtomicBool>,
    pub sample_rate: u32,
}

struct Loaded {
    model: Arc<PocketModel>,
    sample_rate: u32,
}

pub struct TtsService {
    models_dir: PathBuf,
    loaded: RwLock<Option<Arc<Loaded>>>,
    voices: RwLock<HashMap<String, Arc<VoiceState<F32Backend>>>>,
    generation: Arc<Mutex<()>>,
    active_cancel: Mutex<Option<Arc<AtomicBool>>>,
    warm_lock: tokio::sync::Mutex<()>,
}

impl TtsService {
    pub fn new(models_dir: impl Into<PathBuf>) -> Self {
        Self {
            models_dir: models_dir.into(),
            loaded: RwLock::new(None),
            voices: RwLock::new(HashMap::new()),
            generation: Arc::new(Mutex::new(())),
            active_cancel: Mutex::new(None),
            warm_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub fn models_dir(&self) -> &Path {
        &self.models_dir
    }

    pub fn is_warm(&self) -> bool {
        self.loaded.read().map(|g| g.is_some()).unwrap_or(false)
    }

    /// Load the model and the default voice. Safe to call repeatedly.
    pub async fn warm(self: &Arc<Self>) -> Result<()> {
        if self.is_warm() {
            return Ok(());
        }
        let _guard = self.warm_lock.lock().await;
        if self.is_warm() {
            return Ok(());
        }
        let service = self.clone();
        tokio::task::spawn_blocking(move || service.warm_blocking())
            .await
            .context("TTS warm task panicked")??;
        Ok(())
    }

    fn warm_blocking(self: &Arc<Self>) -> Result<()> {
        {
            let loaded = self.loaded.read().unwrap();
            if loaded.is_some() {
                return Ok(());
            }
        }

        let model_path = self
            .models_dir
            .join("tts")
            .join("english_18m_2026-10")
            .join("model.safetensors");
        let tokenizer_path = self
            .models_dir
            .join("tts")
            .join("english_18m_2026-10")
            .join("tokenizer.json");
        if !model_path.is_file() {
            bail!("TTS model missing at {}", model_path.display());
        }
        if !tokenizer_path.is_file() {
            bail!("TTS tokenizer missing at {}", tokenizer_path.display());
        }

        let started = std::time::Instant::now();
        let config = Config::english_18m()?.with_assets(&model_path, &tokenizer_path);
        let model = PocketModel::load(config, &LoadOptions::default())
            .context("failed to load Pocket TTS 18M model")?;
        let sample_rate = model.sample_rate() as u32;
        let loaded = Arc::new(Loaded {
            model: Arc::new(model),
            sample_rate,
        });

        // Resolve the default voice now so a broken bundle fails during warm-up, not on the
        // first spoken token.
        let voice = self.resolve_voice_path(DEFAULT_VOICE)?;
        let state = loaded
            .model
            .get_state_for_audio_prompt(&voice.to_string_lossy(), false)
            .with_context(|| format!("failed to load voice {}", voice.display()))?;
        self.voices
            .write()
            .unwrap()
            .insert(DEFAULT_VOICE.to_string(), Arc::new(state));

        *self.loaded.write().unwrap() = Some(loaded);
        log::info!(
            "Pocket TTS 18M warm in {} ms",
            started.elapsed().as_millis()
        );
        Ok(())
    }

    pub fn release(&self) -> bool {
        if let Ok(_guard) = self.generation.try_lock() {
            *self.loaded.write().unwrap() = None;
            self.voices.write().unwrap().clear();
            true
        } else {
            false
        }
    }

    /// Cancel the utterance currently being generated and any queued one.
    pub fn cancel_current(&self) {
        if let Ok(guard) = self.active_cancel.lock()
            && let Some(cancel) = guard.as_ref()
        {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn start_stream(self: &Arc<Self>, text: String, voice: String) -> Result<TtsStream> {
        let Some(loaded) = self.loaded.read().unwrap().clone() else {
            bail!("TTS is not warm");
        };
        if text.trim().is_empty() {
            bail!("text cannot be empty");
        }

        let service = self.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut active = self.active_cancel.lock().unwrap();
            if let Some(previous) = active.replace(cancel.clone()) {
                previous.store(true, Ordering::Relaxed);
            }
        }

        let sample_rate = loaded.sample_rate;
        let (tx, rx) = mpsc::channel::<TtsEvent>(32);
        let task_cancel = cancel.clone();
        let task_service = service.clone();
        let task_loaded = loaded.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = task_service.generation.lock().unwrap();
            let result = (|| -> Result<()> {
                let state = task_service.voice_state(&task_loaded, &voice)?;
                let mut on_chunk = |pcm: &[f32]| -> Result<()> {
                    if task_cancel.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    let _ = tx.blocking_send(TtsEvent::Samples(pcm.to_vec()));
                    Ok(())
                };
                task_loaded.model.generate_audio_stream(
                    &state,
                    &text,
                    MAX_TOKENS_PER_CHUNK,
                    Some(FRAMES_AFTER_EOS),
                    &task_cancel,
                    &mut on_chunk,
                )?;
                Ok(())
            })();

            let cancelled = task_cancel.load(Ordering::Relaxed);
            match result {
                Ok(()) => {
                    let _ = tx.blocking_send(TtsEvent::Done { cancelled });
                }
                Err(error) => {
                    let _ = tx.blocking_send(TtsEvent::Error(format!("{error:#}")));
                }
            }

            if let Ok(mut active) = task_service.active_cancel.lock()
                && active
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &task_cancel))
            {
                *active = None;
            }
        });

        Ok(TtsStream {
            rx,
            cancel,
            sample_rate,
        })
    }

    fn voice_state(
        &self,
        loaded: &Arc<Loaded>,
        voice: &str,
    ) -> Result<Arc<VoiceState<F32Backend>>> {
        let name = normalize_voice(voice);
        if let Some(state) = self.voices.read().unwrap().get(name).cloned() {
            return Ok(state);
        }
        let path = self.resolve_voice_path(name)?;
        let state = loaded
            .model
            .get_state_for_audio_prompt(&path.to_string_lossy(), false)
            .with_context(|| format!("failed to load voice {}", path.display()))?;
        let state = Arc::new(state);
        self.voices
            .write()
            .unwrap()
            .insert(name.to_string(), state.clone());
        Ok(state)
    }

    fn resolve_voice_path(&self, voice: &str) -> Result<PathBuf> {
        let name = normalize_voice(voice);
        let path = self
            .models_dir
            .join("tts")
            .join("voices")
            .join(format!("{name}.safetensors"));
        if path.is_file() {
            return Ok(path);
        }
        if name != FALLBACK_VOICE {
            let fallback = self
                .models_dir
                .join("tts")
                .join("voices")
                .join(format!("{FALLBACK_VOICE}.safetensors"));
            if fallback.is_file() {
                log::warn!("voice {name:?} not bundled; falling back to {FALLBACK_VOICE}");
                return Ok(fallback);
            }
        }
        bail!("voice state missing at {}", path.display());
    }

    /// Full WAV for `/api/tts`; the streaming route uses [`Self::start_stream`] directly.
    pub async fn synthesize_wav(self: &Arc<Self>, text: String, voice: String) -> Result<Vec<u8>> {
        if !self.is_warm() {
            self.warm().await?;
        }
        let mut stream = self.start_stream(text, voice)?;
        let sample_rate = stream.sample_rate;
        let cancel = stream.cancel.clone();
        let samples = tokio::task::spawn_blocking(move || -> Result<Vec<f32>> {
            let mut samples = Vec::new();
            while let Some(event) = stream.rx.blocking_recv() {
                match event {
                    TtsEvent::Samples(chunk) => samples.extend_from_slice(&chunk),
                    TtsEvent::Done { cancelled } => {
                        if cancelled {
                            bail!("TTS cancelled");
                        }
                        break;
                    }
                    TtsEvent::Error(error) => bail!(error),
                }
            }
            Ok(samples)
        })
        .await
        .context("TTS generation task panicked")??;
        if cancel.load(Ordering::Relaxed) {
            bail!("TTS cancelled");
        }
        wav::wav_bytes(&samples, sample_rate)
    }
}

fn normalize_voice(voice: &str) -> &str {
    let voice = voice.trim();
    match voice.to_ascii_lowercase().as_str() {
        "jarvis" | "" => "jarvis",
        "alba" => "alba",
        "anna" => "anna",
        _ => "jarvis",
    }
}

/// Locate the bundled models directory.
///
/// Priority:
/// 1. `STARK_MODELS_DIR`
/// 2. `<harness_root>/models`
/// 3. next to or above the server executable (`models`, `../models`, `../resources/models`)
pub fn resolve_models_dir(harness_root: &Path) -> Option<PathBuf> {
    if let Ok(value) = std::env::var("STARK_MODELS_DIR") {
        let path = PathBuf::from(value);
        if path.is_dir() {
            return Some(path);
        }
    }
    let harness = harness_root.join("models");
    if harness.is_dir() {
        return Some(harness);
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [
        dir.join("models"),
        dir.join("resources").join("models"),
        dir.join("..").join("models"),
        dir.join("..").join("resources").join("models"),
        dir.join("..").join("..").join("resources").join("models"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_dir())
}
