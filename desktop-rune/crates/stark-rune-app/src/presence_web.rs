//! Web stand-in for the native desktop presence module.
//!
//! The browser has no tray, native overlay or global shortcuts. The shell
//! still needs a shared state type, and the in-page voice card remains the
//! live surface there.

#[derive(Clone, Debug, Default)]
pub struct PresenceState {
    pub listen_requested: bool,
    pub open_requested: bool,
}

pub fn tick(_s: &mut crate::shell::ShellState, _clock: f64, _tx: &mut rune::Tx) -> bool {
    false
}

pub fn on_close(_s: &mut crate::shell::ShellState, _tx: &mut rune::Tx) {}

pub fn on_quit(_s: &mut crate::shell::ShellState, _tx: &mut rune::Tx) {}

pub fn shortcut_listen(s: &mut crate::shell::ShellState, _tx: &mut rune::Tx) {
    s.presence.listen_requested = true;
}

pub fn shortcut_open(s: &mut crate::shell::ShellState, _tx: &mut rune::Tx) {
    s.presence.open_requested = true;
}
