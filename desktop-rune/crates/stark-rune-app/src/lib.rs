//! Library surface of the native app, shared with the web shell crate
//! (`stark-rune-web`, the `cdylib`). The binary (`src/main.rs`) keeps the
//! native CLI; lib and bin compile the same modules below.

#[cfg(feature = "rune-real")]
#[path = "composer.rs"]
pub mod composer;
#[cfg(feature = "rune-real")]
#[path = "projects.rs"]
pub mod projects;
#[cfg(feature = "rune-real")]
#[path = "rooms.rs"]
pub mod rooms;
#[cfg(feature = "rune-real")]
#[path = "rune_canvas.rs"]
pub mod rune_canvas;
#[cfg(feature = "rune-real")]
#[path = "rune_transcript.rs"]
pub mod rune_transcript;
#[cfg(feature = "rune-real")]
#[path = "shell.rs"]
pub mod shell;
#[cfg(feature = "rune-real")]
#[path = "stream.rs"]
pub mod stream;
#[cfg(feature = "rune-real")]
#[path = "transport.rs"]
pub mod transport;
#[cfg(feature = "rune-real")]
#[path = "voice.rs"]
pub mod voice;
