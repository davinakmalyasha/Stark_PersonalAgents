//! Parakeet Unified English speech-to-text.
//!
//! Feature `stt` compiles the real engine. Without it the server still builds
//! and reports an honest unavailable error; no old STT runtime is linked.
//! Vocabulary types compile in both builds so the API and app can carry a
//! list whether or not this binary includes the engine.

#[cfg(feature = "stt")]
mod decode;
#[cfg(feature = "stt")]
mod engine;
mod silence;
#[cfg(feature = "stt")]
mod speech;
#[cfg(feature = "stt")]
mod vad;
pub mod vocabulary;

#[cfg(feature = "stt")]
pub use engine::{
    MODEL_DIR, MODEL_FILE, MODEL_NAME, SttBackend, SttService, TRAILING_SILENCE_SAMPLES, TurnHandle,
};
pub use silence::{LevelGate, level_gate};
#[cfg(feature = "stt")]
pub use speech::{FRAME_SAMPLES, SAMPLE_RATE, SpeechFrames};
#[cfg(feature = "stt")]
pub use vad::{SpeechDetector, VAD_FILE};
pub use vocabulary::{
    BiasParts, MAX_BIASED_TERMS, MAX_ENTRIES, MAX_SOUNDS_LIKE, MAX_TERM_CHARS, Vocabulary,
    VocabularyBoost, VocabularyEntry,
};

#[cfg(not(feature = "stt"))]
mod stub {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use anyhow::{Result, bail};
    use tokio::sync::mpsc::UnboundedSender;

    use crate::LiveEvent;

    pub const MODEL_DIR: &str = "parakeet-unified-en-0.6b";
    pub const MODEL_FILE: &str = "parakeet-unified-en-0.6b-Q8_0.gguf";
    pub const MODEL_NAME: &str = "parakeet-unified-en-0.6b";
    pub const SAMPLE_RATE: usize = 16_000;
    pub const FRAME_SAMPLES: usize = 320;
    pub const TRAILING_SILENCE_SAMPLES: usize = 6_400;

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

    pub struct SttService;

    impl SttService {
        pub fn load(_models_dir: &Path) -> Result<Self> {
            bail!("Parakeet STT is not available in this build")
        }

        pub fn model_name(&self) -> &str {
            "unavailable"
        }

        pub fn backend(&self) -> SttBackend {
            SttBackend::Cpu
        }

        pub fn backend_label(&self) -> &'static str {
            "unavailable"
        }

        pub fn device(&self) -> &str {
            "unavailable"
        }

        pub fn can_stream(&self) -> bool {
            false
        }

        pub fn vad_installed(&self) -> bool {
            false
        }

        pub fn is_loaded(&self) -> bool {
            false
        }

        pub fn warm(&self) -> Result<()> {
            bail!("Parakeet STT is not available in this build")
        }

        pub fn release(&self) -> bool {
            false
        }

        pub fn transcribe_file(
            &self,
            _path: &Path,
            _vocabulary: &super::Vocabulary,
        ) -> Result<Option<String>> {
            bail!("Parakeet STT is not available in this build")
        }

        pub fn transcribe_pcm(
            &self,
            _samples: &[f32],
            _vocabulary: &super::Vocabulary,
        ) -> Result<String> {
            bail!("Parakeet STT is not available in this build")
        }

        pub fn start_turn(
            self: &Arc<Self>,
            _events: UnboundedSender<LiveEvent>,
            _turn_id: String,
            _vocabulary: super::Vocabulary,
        ) -> Result<TurnHandle> {
            bail!("Parakeet STT is not available in this build")
        }
    }

    pub struct TurnHandle {
        _path: PathBuf,
    }

    impl TurnHandle {
        pub fn feed(&self, _samples: Vec<f32>) -> bool {
            false
        }

        pub fn finish(self, _clip: Vec<f32>) {}

        pub fn cancel(self) {}
    }
}

#[cfg(not(feature = "stt"))]
pub use stub::{
    FRAME_SAMPLES, MODEL_DIR, MODEL_FILE, MODEL_NAME, SAMPLE_RATE, SttBackend, SttService,
    TRAILING_SILENCE_SAMPLES,
};
