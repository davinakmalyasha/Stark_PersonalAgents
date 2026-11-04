use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedDevice {
    pub id: String,
    pub name: String,
    pub token: String,
    pub created_at: String,
    #[serde(default)]
    pub last_seen: String,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
struct PairingFile {
    /// Legacy single token (still honoured so already-paired phones keep
    /// working). New pairings should issue a device token instead.
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
        if let Ok(text) = std::fs::read_to_string(&self.path)
            && let Ok(parsed) = serde_json::from_str::<PairingFile>(&text)
            && let Ok(mut state) = self.state.lock()
        {
            *state = parsed;
        }
        *loaded = true;
        let legacy = self.state.lock().unwrap().token.clone();
        if legacy.is_empty() {
            let _ = self.regenerate_legacy();
        }
    }

    /// Legacy pairing token shown by the desktop QR dialog.
    pub fn token(&self) -> String {
        self.ensure_loaded();
        let mut state = self.state.lock().unwrap();
        if state.token.is_empty() {
            state.token = Uuid::new_v4().simple().to_string()[..12].to_string();
            let snapshot = state.clone();
            drop(state);
            self.save(&snapshot);
            return snapshot.token;
        }
        state.token.clone()
    }

    pub fn rotate(&self) -> String {
        self.ensure_loaded();
        let token = Uuid::new_v4().simple().to_string()[..12].to_string();
        let mut state = self.state.lock().unwrap();
        state.token = token.clone();
        let snapshot = state.clone();
        drop(state);
        self.save(&snapshot);
        token
    }

    /// Accept either the legacy token or any issued device token.
    pub fn authorize(&self, token: &str) -> bool {
        if token.trim().is_empty() {
            return false;
        }
        self.ensure_loaded();
        let mut state = self.state.lock().unwrap();
        if state.token == token {
            return true;
        }
        let mut touched = false;
        for device in state.devices.iter_mut() {
            if device.token == token {
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

    pub fn devices(&self) -> Vec<PairedDevice> {
        self.ensure_loaded();
        self.state.lock().unwrap().devices.clone()
    }

    /// Issue a new per-device token. The plaintext token is returned once;
    /// only the stored copy is used for auth afterwards.
    pub fn issue(&self, name: &str) -> PairedDevice {
        self.ensure_loaded();
        let name = name.trim();
        let device = PairedDevice {
            id: format!("dev-{}", &Uuid::new_v4().simple().to_string()[..8]),
            name: if name.is_empty() {
                "Unnamed device".to_string()
            } else {
                name.to_string()
            },
            token: format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
            created_at: chrono::Utc::now().to_rfc3339(),
            last_seen: String::new(),
        };
        let mut state = self.state.lock().unwrap();
        state.devices.push(device.clone());
        let snapshot = state.clone();
        drop(state);
        self.save(&snapshot);
        device
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
        let token = Uuid::new_v4().simple().to_string()[..12].to_string();
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
        let _ = std::fs::write(
            &self.path,
            serde_json::to_string_pretty(&json!({
                "token": state.token,
                "devices": state.devices,
            }))
            .unwrap_or_default(),
        );
    }
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

        assert!(store.revoke(&phone.id));
        assert!(!store.authorize(&phone.token));
        // Revoking one device leaves the other alone.
        assert!(store.authorize(&laptop.token));

        let reloaded = PairingStore::new(dir.path());
        assert!(!reloaded.authorize(&phone.token));
        assert!(reloaded.authorize(&laptop.token));
        assert_eq!(reloaded.devices().len(), 1);
    }
}
