//! Moonshine streaming STT, in-process.
//!
//! The native `libmoonshine` prebuilt used by `moonshine-rs` is MSVC-only on Windows. The
//! GNU build therefore returns a clear unsupported error; release builds use the MSVC target.

use anyhow::Result;
use std::path::{Path, PathBuf};

#[cfg(all(target_env = "msvc", feature = "stt"))]
pub use imp::SttService;
#[cfg(not(all(target_env = "msvc", feature = "stt")))]
pub use imp::SttService;

#[cfg(all(target_env = "msvc", feature = "stt"))]
mod imp {
    use super::*;
    use std::sync::Arc;

    use moonshine_rs::audio::load_audio_for_transcription;
    use moonshine_rs::{ModelArch, OwnedTranscriberStream, Transcriber};

    const MODEL: (&str, &str, ModelArch) =
        ("medium", "medium-streaming-en", ModelArch::MediumStreaming);
    const REQUIRED_FILES: &[&str] = &[
        "encoder.ort",
        "decoder_kv.ort",
        "frontend.weights.ort",
        "frontend.model.ort",
        "cross_kv.ort",
        "adapter.ort",
        "streaming_config.json",
        "tokenizer.bin",
    ];

    pub struct SttService {
        transcriber: Arc<Transcriber>,
        model: &'static str,
    }

    impl SttService {
        pub fn load(models_dir: &Path) -> Result<Self> {
            let (name, dirname, arch) = MODEL;
            let dir = models_dir.join("stt").join(dirname);
            if !dir.is_dir() {
                anyhow::bail!(
                    "Moonshine medium-streaming model missing at {}; run prepare-voice-models.mjs",
                    dir.display()
                );
            }
            let missing: Vec<&str> = REQUIRED_FILES
                .iter()
                .copied()
                .filter(|file| !dir.join(*file).is_file())
                .collect();
            if !missing.is_empty() {
                anyhow::bail!(
                    "Moonshine medium-streaming model missing files in {}: {}",
                    dir.display(),
                    missing.join(", ")
                );
            }
            let transcriber = Transcriber::from_files(&dir, arch, None).map_err(|error| {
                anyhow::anyhow!("failed to load Moonshine medium-streaming: {error}")
            })?;
            let keyterms =
                std::env::var("STARK_STT_KEYTERMS").unwrap_or_else(|_| "Stark".to_string());
            if !keyterms.trim().is_empty()
                && let Err(error) = transcriber.set_keyterms(keyterms.trim())
            {
                log::warn!("Moonshine keyterms unavailable: {error}");
            }
            log::info!("Moonshine {name}-streaming loaded from {}", dir.display());
            Ok(Self {
                transcriber: Arc::new(transcriber),
                model: name,
            })
        }

        pub fn model_name(&self) -> &'static str {
            self.model
        }

        pub fn transcribe_file(&self, path: &Path) -> Result<String> {
            let pcm = load_audio_for_transcription(path)
                .map_err(|e| anyhow::anyhow!("cannot decode {}: {e}", path.display()))?;
            let transcript = self
                .transcriber
                .transcribe(&pcm, 16_000)
                .map_err(|e| anyhow::anyhow!("Moonshine transcription failed: {e}"))?;
            Ok(transcript.text())
        }

        pub fn transcribe_pcm(&self, pcm: &[f32], sample_rate: u32) -> Result<String> {
            let transcript = self
                .transcriber
                .transcribe(pcm, sample_rate)
                .map_err(|e| anyhow::anyhow!("Moonshine transcription failed: {e}"))?;
            Ok(transcript.text())
        }

        pub fn create_stream(&self) -> Result<OwnedTranscriberStream> {
            self.transcriber
                .clone()
                .create_owned_stream()
                .map_err(|e| anyhow::anyhow!("cannot start Moonshine stream: {e}"))
        }

        pub fn transcriber(&self) -> Arc<Transcriber> {
            self.transcriber.clone()
        }
    }
}

#[cfg(not(all(target_env = "msvc", feature = "stt")))]
mod imp {
    use super::*;

    pub struct SttService;

    impl SttService {
        pub fn load(_models_dir: &Path) -> Result<Self> {
            anyhow::bail!("Moonshine STT is only available in the MSVC build")
        }

        pub fn transcribe_file(&self, _path: &Path) -> Result<String> {
            anyhow::bail!("Moonshine STT is only available in the MSVC build")
        }

        pub fn transcribe_pcm(&self, _pcm: &[f32], _sample_rate: u32) -> Result<String> {
            anyhow::bail!("Moonshine STT is only available in the MSVC build")
        }

        pub fn model_name(&self) -> &'static str {
            "unavailable"
        }
    }
}

pub fn stt_model_dir(models_dir: &Path) -> PathBuf {
    models_dir.join("stt").join("medium-streaming-en")
}
