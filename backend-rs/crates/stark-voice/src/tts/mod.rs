//! Vendored from the official `kyutai-labs/pocket-tts` Rust port (MIT), with the
//! model/config path handling made local-only for the packaged Stark app.
//!
//! Upstream: https://github.com/kyutai-labs/pocket-tts/tree/main/rust
//! Pinned ptts fork: gabrieldemarmiesse/xn-ptts @ 7efa26973f81bba42cf2ace8da2c1eb9b605d54d

mod audio;
mod config;
mod defaults;
mod model;
mod rng;
mod text;
mod tokenizer;
mod weights;

pub use config::{Config, FlowType};
pub use model::{CpuBackend, LoadOptions, TtsModel, VoiceState};
