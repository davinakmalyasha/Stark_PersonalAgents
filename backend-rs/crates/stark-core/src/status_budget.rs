//! Per-turn budget for snapshot status tools.
//!
//! Delegation results and group progress are event-driven. The Orchestrator
//! may take one snapshot per turn when the user asks, but a second snapshot in
//! the same turn is almost always a polling loop and is blocked.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy)]
pub(crate) enum StatusKind {
    Delegation,
    Group,
}

#[derive(Default)]
struct Seen {
    delegation: bool,
    group: bool,
}

static SEEN: OnceLock<Mutex<HashMap<String, Seen>>> = OnceLock::new();

fn seen() -> &'static Mutex<HashMap<String, Seen>> {
    SEEN.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn begin_turn(chat_id: &str) -> TurnGuard {
    if let Ok(mut seen) = seen().lock() {
        seen.insert(chat_id.to_string(), Seen::default());
    }
    TurnGuard {
        chat_id: chat_id.to_string(),
    }
}

/// RAII removal: every early return, abort, and error path drops the guard,
/// so a cancelled or failed run never leaks its entry. Codex keeps per-turn
/// retry state on the turn object for the same reason.
pub(crate) struct TurnGuard {
    chat_id: String,
}

impl Drop for TurnGuard {
    fn drop(&mut self) {
        if let Ok(mut seen) = seen().lock() {
            seen.remove(&self.chat_id);
        }
    }
}

pub(crate) fn allow(chat_id: &str, kind: StatusKind) -> bool {
    let Ok(mut seen) = seen().lock() else {
        return true;
    };
    let entry = seen.entry(chat_id.to_string()).or_default();
    match kind {
        StatusKind::Delegation if entry.delegation => false,
        StatusKind::Delegation => {
            entry.delegation = true;
            true
        }
        StatusKind::Group if entry.group => false,
        StatusKind::Group => {
            entry.group = true;
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_snapshot_per_kind_per_turn() {
        let chat = "status-budget-test";
        let _guard = begin_turn(chat);
        assert!(allow(chat, StatusKind::Delegation));
        assert!(!allow(chat, StatusKind::Delegation));
        assert!(allow(chat, StatusKind::Group));
        assert!(!allow(chat, StatusKind::Group));
        let _guard = begin_turn(chat);
        assert!(allow(chat, StatusKind::Delegation));
        assert!(allow(chat, StatusKind::Group));
    }

    #[test]
    fn guard_drop_removes_the_entry() {
        let chat = "status-budget-guard-drop";
        {
            let _guard = begin_turn(chat);
            assert!(allow(chat, StatusKind::Delegation));
        }
        // Dropped: the next turn starts fresh instead of inheriting state.
        let _guard = begin_turn(chat);
        assert!(allow(chat, StatusKind::Delegation));
        assert!(seen().lock().unwrap().contains_key(chat));
    }
}
