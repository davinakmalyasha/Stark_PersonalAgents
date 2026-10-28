//! Single-process voice services for the Stark Rust backend.
//!
//! TTS is the official Pocket TTS Rust port, MIT-licensed, vendored under `tts/`.
//! See `POCKET_TTS_LICENSE`.

pub mod capture;
pub mod live;
pub mod stt;
pub mod tts;
pub mod wav;

mod service;

pub use capture::CaptureManager;
pub use live::{LiveEvent, LiveSession};
pub use service::{F32Backend, PocketModel, TtsEvent, TtsService, TtsStream, resolve_models_dir};
pub use stt::SttService;
