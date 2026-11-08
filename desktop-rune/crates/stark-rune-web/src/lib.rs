//! Stark web shell (`cdylib`) — mirrors `examples/web-hello`.
//!
//! NOTE: this crate only ever links for `wasm32-unknown-unknown`. MinGW `ld`
//! cannot link a cdylib re-exporting Rune's monomorphized tree on Windows-GNU
//! (export ordinal overflow), so this crate is outside the workspace
//! `default-members`: host builds never touch it. Build explicitly:
//! `cargo build -p stark-rune-web --target wasm32-unknown-unknown`
//! (or `desktop-rune/web/build.sh`).

/// Browser entrypoint: no CLI on web — open the unified shell directly.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() {
    rune::run(stark_rune_app::shell::shell_spec());
}
