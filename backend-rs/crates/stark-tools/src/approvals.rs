use std::collections::HashSet;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct ApprovalCache {
    approved: Arc<Mutex<HashSet<Vec<String>>>>,
}

impl ApprovalCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_approved(&self, command: &str) -> bool {
        let tokens = tokens(command);
        if tokens.is_empty() {
            return false;
        }
        let approved = self.approved.lock().unwrap();
        approved.iter().any(|prefix| {
            !prefix.is_empty()
                && prefix.len() <= tokens.len()
                && prefix
                    .iter()
                    .zip(tokens.iter())
                    .all(|(left, right)| left == right)
        })
    }

    pub fn approve(&self, prefix: &[String]) {
        if prefix.is_empty() {
            return;
        }
        self.approved.lock().unwrap().insert(prefix.to_vec());
    }
}

fn tokens(command: &str) -> Vec<String> {
    command
        .split_whitespace()
        .map(|token| token.trim_matches(['"', '\'']).to_string())
        .filter(|token| !token.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caches_prefixes() {
        let cache = ApprovalCache::new();
        assert!(!cache.is_approved("git push origin main"));
        cache.approve(&["git".into(), "push".into()]);
        assert!(cache.is_approved("git push origin main"));
        assert!(!cache.is_approved("git status"));
    }
}
