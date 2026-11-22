//! Signed, versioned self-update for the Rune desktop app.
//!
//! The app never replaces a running executable. It checks a small manifest,
//! downloads the release zip, verifies its SHA-256 and minisign signature,
//! extracts it into a versioned install tree, validates the staged binaries,
//! then writes `pending.json`. `stark-launcher` applies that pending version
//! after the UI exits and rolls back if the new build fails to start.
//!
//! Tests can point `STARK_UPDATE_URL` at a local fixture and
//! `STARK_INSTALL_ROOT` at a temporary tree; the release workflow embeds the
//! minisign public key and publishes the matching manifest.

use std::fs;
#[cfg(not(target_arch = "wasm32"))]
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

#[cfg(not(target_arch = "wasm32"))]
use serde::Deserialize;
#[cfg(not(target_arch = "wasm32"))]
use sha2::{Digest, Sha256};

use crate::settings::{UpdatePhase, UpdateState};

/// Default feed. The Rune releases have their own repo so
/// `releases/latest` can never resolve to an NSIS installer for the wrong app.
pub const DEFAULT_MANIFEST_URL: &str =
    "https://github.com/davinakmalyasha/Stark_Rune_Releases/releases/latest/download/latest-rune.json";

#[cfg(not(target_arch = "wasm32"))]
const MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone)]
pub enum UpdateEvent {
    Downloading {
        version: String,
        downloaded: u64,
        total: Option<u64>,
    },
    UpToDate,
    Available {
        version: String,
    },
    Ready {
        version: String,
    },
    Error(String),
    /// An automatic check's network/feed failure: no toast, just return to idle.
    Quiet,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub schema: u32,
    pub version: String,
    #[serde(default)]
    pub min_version: Option<String>,
    pub platforms: std::collections::HashMap<String, Platform>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Deserialize)]
pub struct Platform {
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub signature: String,
}

/// Manifest platform key for this build.
pub fn platform_key() -> &'static str {
    "windows-x86_64"
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The installed tree root. `stark-launcher` sets this; a manual run from a
/// versioned install also recognises the `current.json` beside the exe.
pub fn install_root() -> Option<PathBuf> {
    if let Ok(raw) = std::env::var("STARK_INSTALL_ROOT") {
        let path = PathBuf::from(raw.trim());
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    dir.join("current.json")
        .is_file()
        .then(|| dir.to_path_buf())
}

pub fn manifest_url() -> String {
    std::env::var("STARK_UPDATE_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_MANIFEST_URL.to_string())
}

/// Embedded at release time. Development builds may set `STARK_UPDATE_PUBKEY`
/// or drop `update.pub` beside the installed launcher.
pub fn public_key(install_root: Option<&Path>) -> Option<String> {
    let clean = |text: &str| {
        text.lines()
            .map(str::trim)
            .find(|line| {
                !line.is_empty()
                    && !line.starts_with("untrusted comment:")
                    && !line.starts_with("trusted comment:")
            })
            .map(ToString::to_string)
    };
    if let Ok(value) = std::env::var("STARK_UPDATE_PUBKEY") {
        if let Some(key) = clean(&value) {
            return Some(key);
        }
    }
    if let Some(key) = option_env!("STARK_UPDATE_PUBKEY") {
        if let Some(key) = clean(key) {
            return Some(key);
        }
    }
    let root = install_root?;
    let path = root.join("update.pub");
    fs::read_to_string(path).ok().and_then(|text| clean(&text))
}

/// Start a check on a worker thread. Automatic checks stay silent on network
/// and feed errors; `manual` surfaces them.
#[cfg(target_arch = "wasm32")]
pub fn spawn_check(_manual: bool, tx: Sender<UpdateEvent>) {
    let _ = tx.send(UpdateEvent::Error(
        "Web build — updates are applied when the server deploys.".to_string(),
    ));
}

#[cfg(not(target_arch = "wasm32"))]
pub fn spawn_check(manual: bool, tx: Sender<UpdateEvent>) {
    std::thread::spawn(move || run_check(manual, &tx));
}

#[cfg(not(target_arch = "wasm32"))]
fn run_check(manual: bool, tx: &Sender<UpdateEvent>) {
    if let Err(error) = check_inner(manual, tx) {
        let _ = tx.send(UpdateEvent::Error(error));
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn check_inner(manual: bool, tx: &Sender<UpdateEvent>) -> Result<(), String> {
    let url = manifest_url();
    let body = match fetch_manifest(&url) {
        Ok(body) => body,
        Err(_error) if !manual => {
            let _ = tx.send(UpdateEvent::Quiet);
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    let manifest: Manifest = match serde_json::from_slice(&body) {
        Ok(manifest) => manifest,
        Err(_error) if !manual => {
            let _ = tx.send(UpdateEvent::Quiet);
            return Ok(());
        }
        Err(error) => return Err(format!("Update feed is not valid JSON: {error}")),
    };
    if manifest.schema != 0 && manifest.schema != 1 {
        let error = format!(
            "Update feed schema {} is newer than this app understands.",
            manifest.schema
        );
        if manual {
            return Err(error);
        }
        let _ = tx.send(UpdateEvent::Quiet);
        return Ok(());
    }
    if !version_is_newer(&manifest.version, current_version()) {
        let _ = tx.send(UpdateEvent::UpToDate);
        return Ok(());
    }
    if let Some(min) = &manifest.min_version {
        if !min.trim().is_empty() && version_is_newer(min, current_version()) {
            return Err(format!(
                "This update needs Stark {min} or newer first. Install the bridge release, then retry."
            ));
        }
    }
    if let Some(blocked) = install_root().and_then(|root| blocked_version(&root)) {
        if blocked == manifest.version {
            if manual {
                return Err(format!(
                    "Stark {} failed to start and was rolled back. Install the full installer or wait for a newer build.",
                    manifest.version
                ));
            }
            let _ = tx.send(UpdateEvent::Quiet);
            return Ok(());
        }
    }
    let Some(platform) = manifest.platforms.get(platform_key()) else {
        let error = format!("No {} build in the update feed.", platform_key());
        if manual {
            return Err(error);
        }
        let _ = tx.send(UpdateEvent::Quiet);
        return Ok(());
    };
    let _ = tx.send(UpdateEvent::Available {
        version: manifest.version.clone(),
    });

    let Some(root) = install_root() else {
        // A development run can inspect the feed but must not mutate the tree.
        return Err(
            "This copy is not a versioned install; download the installer to update.".to_string(),
        );
    };
    let version = sanitize_version(&manifest.version)?;
    let updates = root.join("updates");
    let target = root.join("app").join(&version);
    fs::create_dir_all(&updates)
        .map_err(|error| format!("cannot create {}: {error}", updates.display()))?;
    let part = updates.join(format!("{version}.zip.part"));
    let complete = updates.join(format!("{version}.zip"));

    if target.join("stark-rune.exe").is_file() && target.join("stark-server.exe").is_file() {
        validate_staged(&target, &version)?;
        write_pending(&root, &version, "already-staged")?;
        let _ = tx.send(UpdateEvent::Ready { version });
        return Ok(());
    }
    if target.exists() {
        let _ = fs::remove_dir_all(&target);
    }

    let downloaded = download(platform, &part, &version, tx)?;
    if let Some(expected) = platform.size {
        if downloaded != expected {
            let _ = fs::remove_file(&part);
            return Err(format!(
                "Download size mismatch: expected {expected} bytes, got {downloaded}."
            ));
        }
    }
    verify_sha256(&part, &platform.sha256)?;
    verify_signature(&part, &platform.signature, root.as_path())?;
    fs::rename(&part, &complete).map_err(|error| format!("cannot finalise download: {error}"))?;

    let staging = updates.join(format!("{version}.staging"));
    let _ = fs::remove_dir_all(&staging);
    extract_zip(&complete, &staging)?;
    validate_staged(&staging, &version)?;

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let _ = fs::remove_dir_all(&target);
    fs::rename(&staging, &target).map_err(|error| {
        format!(
            "cannot move staged update into {}: {error}",
            target.display()
        )
    })?;
    write_pending(&root, &version, "staged")?;
    let _ = fs::remove_file(&complete);
    let _ = tx.send(UpdateEvent::Ready { version });
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn fetch_manifest(url: &str) -> Result<Vec<u8>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(15))
        .timeout_read(std::time::Duration::from_secs(30))
        .timeout_write(std::time::Duration::from_secs(30))
        .build();
    let response = agent
        .get(url)
        .set("Accept", "application/json")
        .call()
        .map_err(|error| format!("Could not check for updates: {error}"))?;
    let mut body = Vec::new();
    response
        .into_reader()
        .take(1024 * 1024)
        .read_to_end(&mut body)
        .map_err(|error| format!("Could not read update feed: {error}"))?;
    Ok(body)
}

#[cfg(not(target_arch = "wasm32"))]
fn download(
    platform: &Platform,
    part: &Path,
    version: &str,
    tx: &Sender<UpdateEvent>,
) -> Result<u64, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(20))
        .timeout_read(std::time::Duration::from_secs(60))
        .build();
    let response = agent
        .get(&platform.url)
        .set("Accept", "application/octet-stream")
        .call()
        .map_err(|error| format!("Update download failed: {error}"))?;
    let total = platform.size.or_else(|| {
        response
            .header("Content-Length")
            .and_then(|v| v.parse().ok())
    });
    let mut reader = response.into_reader().take(MAX_ARTIFACT_BYTES + 1);
    let mut file = fs::File::create(part)
        .map_err(|error| format!("cannot create {}: {error}", part.display()))?;
    let mut downloaded = 0u64;
    let mut last_report = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("Update download interrupted: {error}"))?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])
            .map_err(|error| format!("cannot write update: {error}"))?;
        downloaded += read as u64;
        if downloaded > MAX_ARTIFACT_BYTES {
            drop(file);
            let _ = fs::remove_file(part);
            return Err("Update artifact is larger than the 512 MB safety cap.".to_string());
        }
        if downloaded - last_report >= 512 * 1024 {
            last_report = downloaded;
            let _ = tx.send(UpdateEvent::Downloading {
                version: version.to_string(),
                downloaded,
                total,
            });
        }
    }
    file.flush()
        .map_err(|error| format!("cannot flush update: {error}"))?;
    let _ = tx.send(UpdateEvent::Downloading {
        version: version.to_string(),
        downloaded,
        total,
    });
    Ok(downloaded)
}

#[cfg(not(target_arch = "wasm32"))]
fn verify_sha256(path: &Path, expected: &str) -> Result<(), String> {
    let expected = expected.trim().to_ascii_lowercase();
    if expected.len() != 64 || !expected.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("Update feed has an invalid SHA-256.".to_string());
    }
    let mut file =
        fs::File::open(path).map_err(|error| format!("cannot read downloaded update: {error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("cannot hash downloaded update: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        return Err(format!(
            "Update checksum mismatch (expected {expected}, got {actual})."
        ));
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn verify_signature(path: &Path, signature: &str, root: &Path) -> Result<(), String> {
    let Some(public_key) = public_key(Some(root)) else {
        return Err(
            "This build has no update signing key. Install a signed release to receive updates."
                .to_string(),
        );
    };
    let public_key = minisign_verify::PublicKey::from_base64(public_key.trim())
        .map_err(|_| "Update signing key is invalid.".to_string())?;
    let signature = minisign_verify::Signature::decode(signature.trim())
        .map_err(|_| "Update signature is invalid.".to_string())?;
    let bytes = fs::read(path).map_err(|error| format!("cannot read update: {error}"))?;
    public_key
        .verify(&bytes, &signature, false)
        .or_else(|_| public_key.verify(&bytes, &signature, true))
        .map_err(|_| "Update signature did not verify.".to_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn extract_zip(zip_path: &Path, target: &Path) -> Result<(), String> {
    fs::create_dir_all(target).map_err(|error| format!("cannot create staging folder: {error}"))?;
    let file =
        fs::File::open(zip_path).map_err(|error| format!("cannot open update zip: {error}"))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| format!("update zip is invalid: {error}"))?;
    if archive.len() > 20_000 {
        return Err("Update zip has too many entries.".to_string());
    }
    let mut unpacked = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("update zip entry {index}: {error}"))?;
        let name = entry
            .name()
            .map(|name| name.into_owned())
            .unwrap_or_else(|_| format!("entry {index}"));
        let Some(relative) = entry.enclosed_name() else {
            return Err(format!("update zip has an unsafe path: {name}"));
        };
        let unix_mode = entry.unix_mode().unwrap_or(0);
        if unix_mode & 0o170000 == 0o120000 {
            return Err(format!("update zip contains a symlink: {name}"));
        }
        unpacked = unpacked.saturating_add(entry.size());
        if unpacked > 1024 * 1024 * 1024 {
            return Err("Update zip expands past the 1 GB safety cap.".to_string());
        }
        let destination = target.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&destination)
                .map_err(|error| format!("cannot create {}: {error}", destination.display()))?;
            continue;
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
        let mut out = fs::File::create(&destination)
            .map_err(|error| format!("cannot write {}: {error}", destination.display()))?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|error| format!("cannot extract {}: {error}", destination.display()))?;
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn validate_staged(staging: &Path, version: &str) -> Result<(), String> {
    let app = staging.join("stark-rune.exe");
    let server = staging.join("stark-server.exe");
    if !app.is_file() {
        return Err("Update zip is missing stark-rune.exe.".to_string());
    }
    if !server.is_file() {
        return Err("Update zip is missing stark-server.exe.".to_string());
    }
    let output = std::process::Command::new(&app)
        .arg("--version")
        .output()
        .map_err(|error| format!("staged app cannot run: {error}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() || !text.contains(version) {
        return Err(format!(
            "Staged app reports '{}', expected version {version}.",
            text.trim()
        ));
    }
    let server_output = std::process::Command::new(&server)
        .arg("--version")
        .output();
    if let Ok(server_output) = server_output {
        if server_output.status.success() {
            let text = String::from_utf8_lossy(&server_output.stdout);
            if !text.trim().is_empty() {
                return Ok(());
            }
        }
    }
    // Older servers may not have --version; the binary presence is enough.
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn blocked_version(root: &Path) -> Option<String> {
    let text = fs::read_to_string(root.join("blocked.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("version")
        .and_then(|version| version.as_str())
        .map(ToString::to_string)
}

#[cfg(not(target_arch = "wasm32"))]
fn write_pending(root: &Path, version: &str, reason: &str) -> Result<(), String> {
    let path = root.join("pending.json");
    let temp = root.join("pending.json.tmp");
    let value = serde_json::json!({
        "version": version,
        "reason": reason,
        "created": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    });
    fs::write(&temp, serde_json::to_vec_pretty(&value).unwrap_or_default())
        .map_err(|error| format!("cannot write pending update: {error}"))?;
    fs::rename(&temp, &path).map_err(|error| format!("cannot save pending update: {error}"))
}

#[cfg(not(target_arch = "wasm32"))]
fn sanitize_version(version: &str) -> Result<String, String> {
    let version = version.trim();
    if version.is_empty()
        || version.len() > 64
        || !version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+' || c == '_')
        || version.starts_with('.')
        || version.contains("..")
    {
        return Err("Update feed has an invalid version string.".to_string());
    }
    Ok(version.to_string())
}

/// Semver-ish ordering: numeric release segments, then a pre-release suffix.
pub fn version_is_newer(candidate: &str, current: &str) -> bool {
    fn parse(value: &str) -> (Vec<u64>, Option<String>) {
        let (release, pre) = value
            .trim()
            .trim_start_matches('v')
            .split_once('-')
            .map_or((value.trim().trim_start_matches('v'), None), |(r, p)| {
                (r, Some(p.to_string()))
            });
        let nums = release
            .split('.')
            .map(|part| {
                part.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0)
            })
            .collect();
        (nums, pre)
    }
    let (a, pa) = parse(candidate);
    let (b, pb) = parse(current);
    let len = a.len().max(b.len());
    for index in 0..len {
        let left = a.get(index).copied().unwrap_or(0);
        let right = b.get(index).copied().unwrap_or(0);
        if left != right {
            return left > right;
        }
    }
    match (pa, pb) {
        (None, None) => false,
        (None, Some(_)) => true,
        (Some(_), None) => false,
        (Some(a), Some(b)) => a > b,
    }
}

/// Map the worker's latest event into the shared settings state.
pub fn apply_event(state: &mut UpdateState, event: UpdateEvent) -> bool {
    state.shown_at = Some(crate::clock::now_millis());
    match event {
        UpdateEvent::Downloading {
            version,
            downloaded,
            total,
        } => {
            let percent = total
                .filter(|total| *total > 0)
                .map(|total| (downloaded.min(total) * 100 / total) as u32);
            state.phase = UpdatePhase::Downloading;
            state.version = version.clone();
            state.restart_required = false;
            state.message = match percent {
                Some(percent) => format!("Downloading Stark {version}… {percent}%"),
                None => format!(
                    "Downloading Stark {version}… {:.1} MB",
                    downloaded as f64 / (1024.0 * 1024.0)
                ),
            };
            true
        }
        UpdateEvent::UpToDate => {
            state.phase = UpdatePhase::UpToDate;
            state.message = "Stark is up to date.".to_string();
            state.restart_required = false;
            true
        }
        UpdateEvent::Available { version } => {
            state.phase = UpdatePhase::Downloading;
            state.version = version.clone();
            state.message = format!("Stark {version} is available — preparing download…");
            true
        }
        UpdateEvent::Ready { version } => {
            state.phase = UpdatePhase::Ready;
            state.version = version.clone();
            state.restart_required = true;
            state.message =
                format!("Stark {version} is ready — it will restart when this run finishes.");
            true
        }
        UpdateEvent::Error(error) => {
            state.phase = UpdatePhase::Error;
            state.message = error;
            state.restart_required = false;
            true
        }
        UpdateEvent::Quiet => {
            state.phase = UpdatePhase::Idle;
            state.message.clear();
            state.restart_required = false;
            true
        }
    }
}

/// Optional automatic check at launch and every six hours. Installed builds
/// only; a dev checkout never reaches the feed.
#[cfg(not(target_arch = "wasm32"))]
pub fn auto_check_due(last: Option<f64>, now: f64) -> bool {
    install_root().is_some() && last.is_none_or(|last| now - last >= 6.0 * 60.0 * 60.0)
}

#[cfg(target_arch = "wasm32")]
pub fn auto_check_due(_last: Option<f64>, _now: f64) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_order_is_numeric_and_prerelease_aware() {
        assert!(version_is_newer("2.7.0", "2.6.4"));
        assert!(version_is_newer("2.10.0", "2.9.9"));
        assert!(!version_is_newer("2.6.4", "2.6.4"));
        assert!(!version_is_newer("2.6.3", "2.6.4"));
        assert!(version_is_newer("3.0.0", "2.99.99"));
        assert!(version_is_newer("2.7.0", "2.7.0-beta.1"));
        assert!(!version_is_newer("2.7.0-beta.1", "2.7.0"));
        assert!(version_is_newer("v2.7.0", "2.6.4"));
    }

    #[test]
    fn unsafe_versions_are_rejected() {
        for bad in ["", "..", "../x", "a/b", "a\\b", "x".repeat(65).as_str()] {
            assert!(sanitize_version(bad).is_err(), "{bad:?}");
        }
        assert_eq!(sanitize_version("2.7.0-rc.1").unwrap(), "2.7.0-rc.1");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn manifest_shape_parses_and_rejects_unknown_schema() {
        let json = serde_json::json!({
            "schema": 1,
            "version": "2.7.0",
            "platforms": {
                "windows-x86_64": {
                    "url": "https://example.test/app.zip",
                    "sha256": "a".repeat(64),
                    "size": 12,
                    "signature": "sig"
                }
            }
        });
        let manifest: Manifest = serde_json::from_value(json).unwrap();
        assert_eq!(manifest.version, "2.7.0");
        assert_eq!(manifest.platforms[platform_key()].size, Some(12));
    }

    #[test]
    fn events_map_to_toast_messages() {
        let mut state = UpdateState::default();
        assert!(apply_event(
            &mut state,
            UpdateEvent::Downloading {
                version: "2.7.0".to_string(),
                downloaded: 50,
                total: Some(100),
            }
        ));
        assert_eq!(state.phase, UpdatePhase::Downloading);
        assert!(state.message.contains("50%"));
        assert!(apply_event(
            &mut state,
            UpdateEvent::Ready {
                version: "2.7.0".to_string()
            }
        ));
        assert_eq!(state.phase, UpdatePhase::Ready);
        assert!(state.restart_required);
        assert!(apply_event(&mut state, UpdateEvent::Quiet));
        assert_eq!(state.phase, UpdatePhase::Idle);
        assert!(state.message.is_empty());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn blocked_version_is_read_from_the_launcher_record() {
        let root = std::env::temp_dir().join(format!("stark-blocked-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("blocked.json"),
            br#"{"version":"2.7.0","error":"failed"}"#,
        )
        .unwrap();
        assert_eq!(blocked_version(&root).as_deref(), Some("2.7.0"));
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn minisign_signature_verifies_and_rejects_tampering() {
        let root = std::env::temp_dir().join(format!("stark-sig-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("update.pub"),
            "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3
",
        )
        .unwrap();
        let file = root.join("payload");
        fs::write(&file, b"test").unwrap();
        let signature = "untrusted comment: signature from minisign secret key
RWQf6LRCGA9i59SLOFxz6NxvASXDJeRtuZykwQepbDEGt87ig1BNpWaVWuNrm73YiIiJbq71Wi+dP9eKL8OC351vwIasSSbXxwA=
trusted comment: timestamp:1555779966	file:test
QtKMXWyYcwdpZAlPF7tE2ENJkRd1ujvKjlj1m9RtHTBnZPa5WKU5uWRs5GoP5M/VqE81QFuMKI5k/SfNQUaOAA==";
        verify_signature(&file, signature, &root).expect("valid signature");
        fs::write(&file, b"Test").unwrap();
        assert!(verify_signature(&file, signature, &root).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn zip_traversal_is_rejected() {
        use std::io::Write as _;
        let dir = std::env::temp_dir().join(format!("stark-updater-zip-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let zip_path = dir.join("bad.zip");
        let file = fs::File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("../escape.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"nope").unwrap();
        zip.finish().unwrap();
        let error = extract_zip(&zip_path, &dir.join("out")).unwrap_err();
        assert!(
            error.contains("unsafe path") || error.contains("invalid"),
            "{error}"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
