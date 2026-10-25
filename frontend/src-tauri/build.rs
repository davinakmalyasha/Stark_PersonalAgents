fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "ping",
            "runtime_info",
            "select_folder",
            "reveal_path",
            "voice_toggle",
            "voice_set_chat",
            "voice_close",
            "voice_emit_event",
            "voice_resize",
            "voice_screen_sources",
            "voice_screen_select",
            "voice_screen_protect",
        ]),
    ))
    .expect("failed to run tauri-build");
}
