use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedDevice {
    pub id: String,
    pub name: String,
    /// Legacy plaintext token for devices created before hashing. New devices
    /// leave this empty and store only `token_hash`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub token: String,
    /// `sha256:<hex>` digest of the issued token.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub token_hash: String,
    pub created_at: String,
    #[serde(default)]
    pub last_seen: String,
}

#[derive(Debug, Clone)]
pub struct IssuedDevice {
    pub device: PairedDevice,
    pub token: String,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
struct PairingFile {
    /// Legacy single token (still honoured so already-paired phones keep
    /// working). It is shown by the desktop QR dialog, so this one stays
    /// reversible; device tokens are stored as hashes.
    #[serde(default)]
    token: String,
    #[serde(default)]
    devices: Vec<PairedDevice>,
}

/// Pairing tokens for remote clients. One token per device means a lost phone
/// can be revoked without re-pairing the laptop.
pub struct PairingStore {
    path: PathBuf,
    state: Mutex<PairingFile>,
    loaded: Mutex<bool>,
}

impl PairingStore {
    pub fn new(harness_root: impl Into<PathBuf>) -> Self {
        Self {
            path: harness_root
                .into()
                .join(".deep_agents")
                .join("pairing.json"),
            state: Mutex::new(PairingFile::default()),
            loaded: Mutex::new(false),
        }
    }

    fn ensure_loaded(&self) {
        let mut loaded = self.loaded.lock().unwrap();
        if *loaded {
            return;
        }
        let mut changed = false;
        if let Ok(text) = std::fs::read_to_string(&self.path)
            && let Ok(mut parsed) = serde_json::from_str::<PairingFile>(&text)
        {
            // Migrate legacy per-device plaintext tokens in place. This is
            // idempotent: hashed devices have an empty `token`.
            for device in &mut parsed.devices {
                if !device.token.is_empty() {
                    if device.token_hash.is_empty() {
                        device.token_hash = hash_token(&device.token);
                    }
                    device.token.clear();
                    changed = true;
                }
            }
            if let Ok(mut state) = self.state.lock() {
                *state = parsed;
            }
        }
        *loaded = true;
        let legacy = self.state.lock().unwrap().token.clone();
        if legacy.is_empty() {
            let _ = self.regenerate_legacy();
        } else if changed {
            let snapshot = self.state.lock().unwrap().clone();
            self.save(&snapshot);
        }
    }

    /// Legacy pairing token shown by the desktop QR dialog.
    pub fn token(&self) -> String {
        self.ensure_loaded();
        let mut state = self.state.lock().unwrap();
        if state.token.is_empty() {
            state.token = new_legacy_token();
            let snapshot = state.clone();
            drop(state);
            self.save(&snapshot);
            return snapshot.token;
        }
        state.token.clone()
    }

    pub fn rotate(&self) -> String {
        self.ensure_loaded();
        let token = new_legacy_token();
        let mut state = self.state.lock().unwrap();
        state.token = token.clone();
        let snapshot = state.clone();
        drop(state);
        self.save(&snapshot);
        token
    }

    /// Accept either the legacy token or any issued device token. Device token
    /// comparison uses the stored SHA-256 digest and a constant-time compare.
    pub fn authorize(&self, token: &str) -> bool {
        let token = token.trim();
        if token.is_empty() {
            return false;
        }
        self.ensure_loaded();
        let mut state = self.state.lock().unwrap();
        if constant_time_eq(state.token.as_bytes(), token.as_bytes()) {
            return true;
        }
        let digest = hash_token(token);
        let mut touched = false;
        for device in state.devices.iter_mut() {
            let matches = if !device.token_hash.is_empty() {
                constant_time_eq(device.token_hash.as_bytes(), digest.as_bytes())
            } else if !device.token.is_empty() {
                // Compatibility for a device row saved before migration.
                constant_time_eq(device.token.as_bytes(), token.as_bytes())
            } else {
                false
            };
            if matches {
                device.last_seen = chrono::Utc::now().to_rfc3339();
                touched = true;
                break;
            }
        }
        if touched {
            let snapshot = state.clone();
            drop(state);
            self.save(&snapshot);
            true
        } else {
            false
        }
    }

    /// Device list for the settings UI. Token material is never returned.
    pub fn devices(&self) -> Vec<PairedDevice> {
        self.ensure_loaded();
        self.state
            .lock()
            .unwrap()
            .devices
            .iter()
            .map(|device| PairedDevice {
                id: device.id.clone(),
                name: device.name.clone(),
                token: String::new(),
                token_hash: String::new(),
                created_at: device.created_at.clone(),
                last_seen: device.last_seen.clone(),
            })
            .collect()
    }

    /// Issue a new per-device token. The plaintext token is returned once;
    /// only its digest is persisted.
    pub fn issue(&self, name: &str) -> IssuedDevice {
        self.ensure_loaded();
        let name = name.trim();
        let token = new_device_token();
        let device = PairedDevice {
            id: format!("dev-{}", &Uuid::new_v4().simple().to_string()[..8]),
            name: if name.is_empty() {
                "Unnamed device".to_string()
            } else {
                name.to_string()
            },
            token: String::new(),
            token_hash: hash_token(&token),
            created_at: chrono::Utc::now().to_rfc3339(),
            last_seen: String::new(),
        };
        let mut state = self.state.lock().unwrap();
        state.devices.push(device.clone());
        let snapshot = state.clone();
        drop(state);
        self.save(&snapshot);
        IssuedDevice { device, token }
    }

    pub fn revoke(&self, id: &str) -> bool {
        self.ensure_loaded();
        let mut state = self.state.lock().unwrap();
        let before = state.devices.len();
        state.devices.retain(|device| device.id != id);
        let removed = state.devices.len() != before;
        let snapshot = state.clone();
        drop(state);
        if removed {
            self.save(&snapshot);
        }
        removed
    }

    fn regenerate_legacy(&self) -> String {
        let token = new_legacy_token();
        let mut state = self.state.lock().unwrap();
        state.token = token.clone();
        let snapshot = state.clone();
        drop(state);
        self.save(&snapshot);
        token
    }

    fn save(&self, state: &PairingFile) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let text = serde_json::to_string_pretty(&json!({
            "token": state.token,
            "devices": state.devices,
        }))
        .unwrap_or_default();
        write_private(&self.path, text.as_bytes());
    }
}

fn new_legacy_token() -> String {
    Uuid::new_v4().simple().to_string()[..12].to_string()
}

fn new_device_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut out = String::with_capacity(7 + digest.len() * 2);
    out.push_str("sha256:");
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right) {
        diff |= a ^ b;
    }
    diff == 0
}

fn write_private(path: &std::path::Path, bytes: &[u8]) {
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
        {
            let _ = file.write_all(bytes);
            let _ = file.flush();
            let _ =
                std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600));
            return;
        }
    }
    let _ = std::fs::write(path, bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_and_rotates_token() {
        let dir = tempfile::tempdir().unwrap();
        let store = PairingStore::new(dir.path());
        let token = store.token();
        assert_eq!(token.len(), 12);

        let reloaded = PairingStore::new(dir.path());
        assert_eq!(reloaded.token(), token);
        assert_ne!(reloaded.rotate(), token);
        assert_eq!(PairingStore::new(dir.path()).token(), reloaded.token());
    }

    #[test]
    fn device_tokens_authorize_and_revoke_independently() {
        let dir = tempfile::tempdir().unwrap();
        let store = PairingStore::new(dir.path());
        let legacy = store.token();
        let phone = store.issue("phone");
        let laptop = store.issue("laptop");

        assert!(store.authorize(&legacy));
        assert!(store.authorize(&phone.token));
        assert!(store.authorize(&laptop.token));
        assert!(!store.authorize("nope"));

        assert!(store.revoke(&phone.device.id));
        assert!(!store.authorize(&phone.token));
        // Revoking one device leaves the other alone.
        assert!(store.authorize(&laptop.token));

        let reloaded = PairingStore::new(dir.path());
        assert!(!reloaded.authorize(&phone.token));
        assert!(reloaded.authorize(&laptop.token));
        assert_eq!(reloaded.devices().len(), 1);
    }

    #[test]
    fn device_tokens_are_hashed_on_disk_and_migrated() {
        let dir = tempfile::tempdir().unwrap();
        let store = PairingStore::new(dir.path());
        let issued = store.issue("phone");
        let path = dir.path().join(".deep_agents").join("pairing.json");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains(&issued.token),
            "plaintext token must not be persisted"
        );
        assert!(text.contains("sha256:"));

        // Simulate a pre-hash file and verify the loader migrates it.
        let legacy = format!(
            r#"{{"token":"legacytoken","devices":[{{"id":"dev-legacy","name":"old","token":"{}","created_at":"2026-01-01T00:00:00Z","last_seen":""}}]}}"#,
            issued.token
        );
        std::fs::write(&path, legacy).unwrap();
        let migrated = PairingStore::new(dir.path());
        assert!(migrated.authorize(&issued.token));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains(&issued.token));
        assert!(text.contains("sha256:"));
    }

    #[test]
    fn constant_time_compare_matches_only_identical_values() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}
