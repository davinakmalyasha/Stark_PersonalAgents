use std::collections::BTreeMap;
use std::path::PathBuf;

use tauri::{AppHandle, Manager};

const SEED_FILE: &str = "stark-storage-seed.json";
const SEED_MARKER: &str = "stark.storage.migrated.v1";

fn seed_paths(app: &AppHandle) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(path) = std::env::var("STARK_STORAGE_SEED") {
        paths.push(PathBuf::from(path));
    }
    if let Ok(dir) = app.path().app_data_dir() {
        paths.push(dir.join(SEED_FILE));
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        paths.push(
            PathBuf::from(appdata)
                .join("com.stark.workspace")
                .join(SEED_FILE),
        );
    }
    paths.push(PathBuf::from("frontend").join(SEED_FILE));
    paths
}

pub(crate) fn seed_script(app: &AppHandle) -> String {
    let Some(path) = seed_paths(app).into_iter().find(|path| path.exists()) else {
        return String::new();
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return String::new();
    };
    let Ok(seed) = serde_json::from_str::<BTreeMap<String, String>>(&raw) else {
        log::warn!("ignoring invalid storage seed at {}", path.display());
        return String::new();
    };
    if seed.is_empty() {
        return String::new();
    }
    let Ok(seed_json) = serde_json::to_string(&seed) else {
        return String::new();
    };
    let imported = path.with_extension("json.imported");
    if let Err(error) = std::fs::rename(&path, &imported) {
        log::warn!("could not archive storage seed {}: {error}", path.display());
    }
    format!(
        r#"
(function seedStarkStorage() {{
  try {{
    if (localStorage.getItem("{marker}")) return;
    const seed = {seed};
    for (const [key, value] of Object.entries(seed)) {{
      if (localStorage.getItem(key) === null) localStorage.setItem(key, value);
    }}
    localStorage.setItem("{marker}", "1");
  }} catch (_e) {{}}
}})();
"#,
        marker = SEED_MARKER,
        seed = seed_json,
    )
}
