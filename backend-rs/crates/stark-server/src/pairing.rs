use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::{Value, json};
use uuid::Uuid;

pub struct PairingStore {
    path: PathBuf,
    token: Mutex<Option<String>>,
}

impl PairingStore {
    pub fn new(harness_root: impl Into<PathBuf>) -> Self {
        Self {
            path: harness_root
                .into()
                .join(".deep_agents")
                .join("pairing.json"),
            token: Mutex::new(None),
        }
    }

    pub fn token(&self) -> String {
        let mut token = self.token.lock().unwrap();
        if let Some(token) = token.as_ref() {
            return token.clone();
        }
        let loaded = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|value| {
                value
                    .get("token")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .filter(|token| !token.is_empty());
        let token_value =
            loaded.unwrap_or_else(|| Uuid::new_v4().simple().to_string()[..12].to_string());
        self.save(&token_value);
        *token = Some(token_value.clone());
        token_value
    }

    pub fn rotate(&self) -> String {
        let token = Uuid::new_v4().simple().to_string()[..12].to_string();
        self.save(&token);
        *self.token.lock().unwrap() = Some(token.clone());
        token
    }

    fn save(&self, token: &str) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&self.path, json!({"token": token}).to_string());
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
}
