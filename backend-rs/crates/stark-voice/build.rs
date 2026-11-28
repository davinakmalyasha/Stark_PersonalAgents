use std::path::Path;

fn main() {
    println!("cargo:rerun-if-env-changed=VULKAN_SDK");
    if std::env::var_os("CARGO_FEATURE_STT_VULKAN").is_none() {
        return;
    }
    // ggml emits `cargo:rustc-link-lib=vulkan-1`; the SDK's Lib directory is
    // not on rustc's search path automatically. Add it for the final server
    // link (MSVC uses Lib, Unix SDKs use lib).
    let Some(sdk) = std::env::var_os("VULKAN_SDK") else {
        return;
    };
    for directory in ["Lib", "lib"] {
        let path = Path::new(&sdk).join(directory);
        if path.is_dir() {
            println!("cargo:rustc-link-search=native={}", path.display());
        }
    }
}
