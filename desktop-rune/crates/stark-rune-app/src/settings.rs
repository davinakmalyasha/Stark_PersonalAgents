//! Settings — port of the original Settings modal.
//!
//! Contract:
//! - `GET /api/migration/status` → `{active, candidates:[{id,path,active,
//!   has_chats_db,has_sessions,has_config}]}`
//! - `POST /api/migration/import {source}` → `{ok,copied,message}`
//! - `GET /api/data/export` → zip (`Content-Disposition` filename)
//! - `POST /api/data/import` multipart `{file, projects_root?}` →
//!   `{ok,restart_required,from_root,to_root}`
//!
//! The sheet is global like the reference UI's modal: one state on the shell, opened by
//! The rail gear (and by the standalone transcript scene for goldens). The
//! updater is the signed, staged Rune feed in `updater.rs`; the web build is
//! updated with the server deploy.

use crate::keys::EnterPress;
use crate::updater::UpdateEvent;
use std::path::PathBuf;
use std::sync::mpsc;

use rune::{col, row, text, text_input, El, TextEdit};

/// Product version shown by Settings, distinct
/// from the Rune crate version stamped into the binary.
pub const STARK_VERSION: &str = env!("CARGO_PKG_VERSION");

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

fn rgba(r: f32, g: f32, b: f32, a: f32) -> rune::Color {
    rune::Color { r, g, b, a }
}

fn icon(paths: &[&str], size: f32, color: rune::Color) -> rune::Shape {
    crate::icons::lucide(paths, size, 2.0, color)
}

fn icon_settings(size: f32, color: rune::Color) -> rune::Shape {
    icon(
        &[
            "M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915",
            "M15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0z",
        ],
        size,
        color,
    )
}

fn icon_download(size: f32, color: rune::Color) -> rune::Shape {
    icon(
        &[
            "M12 15V3",
            "M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4",
            "m7 10 5 5 5-5",
        ],
        size,
        color,
    )
}

fn icon_folder_open(size: f32, color: rune::Color) -> rune::Shape {
    icon(
        &[
            "m6 14 1.5-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.54 6a2 2 0 0 1-1.95 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2",
        ],
        size,
        color,
    )
}

fn icon_hard_drive(size: f32, color: rune::Color) -> rune::Shape {
    icon(
        &[
            "M10 16h.01",
            "M2.212 11.577a2 2 0 0 0-.212.896V18a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-5.527a2 2 0 0 0-.212-.896L18.55 5.11A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z",
            "M21.946 12.013H2.054",
            "M6 16h.01",
        ],
        size,
        color,
    )
}

fn icon_refresh(size: f32, color: rune::Color) -> rune::Shape {
    icon(
        &[
            "M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8",
            "M21 3v5h-5",
            "M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16",
            "M8 16H3v5",
        ],
        size,
        color,
    )
}

fn icon_upload(size: f32, color: rune::Color) -> rune::Shape {
    icon(
        &[
            "M12 3v12",
            "m17 8-5-5-5 5",
            "M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4",
        ],
        size,
        color,
    )
}

/// One migration root from `/api/migration/status`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MigrationCandidate {
    pub id: String,
    pub path: String,
    pub active: bool,
    pub has_chats_db: bool,
    pub has_sessions: bool,
    pub has_config: bool,
}

impl MigrationCandidate {
    fn from_json(v: &serde_json::Value) -> Self {
        let str_of = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let bool_of = |k: &str| v.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
        Self {
            id: str_of("id"),
            path: str_of("path"),
            active: bool_of("active"),
            has_chats_db: bool_of("has_chats_db"),
            has_sessions: bool_of("has_sessions"),
            has_config: bool_of("has_config"),
        }
    }

    fn has_data(&self) -> bool {
        self.has_chats_db || self.has_sessions || self.has_config
    }

    fn summary(&self) -> String {
        let parts = [
            self.has_chats_db.then_some("chats.db"),
            self.has_sessions.then_some("sessions"),
            self.has_config.then_some("config"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        if parts.is_empty() {
            "empty".to_string()
        } else {
            parts.join(" · ")
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MigrationStatus {
    pub active: String,
    pub candidates: Vec<MigrationCandidate>,
}

impl MigrationStatus {
    fn from_json(v: &serde_json::Value) -> Self {
        Self {
            active: v
                .get("active")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            candidates: v
                .get("candidates")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().map(MigrationCandidate::from_json).collect())
                .unwrap_or_default(),
        }
    }
}

/// The updater phases `updateStore.ts` exposes: the shared toast and Settings
/// both read this one state so they never disagree about an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UpdatePhase {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Downloading,
    Ready,
    Error,
    Unsupported,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct UpdateState {
    pub phase: UpdatePhase,
    pub version: String,
    pub message: String,
    pub restart_required: bool,
    /// When the last event landed, so transient toasts hide after the reference UI's 6s.
    pub shown_at: Option<u64>,
}

enum OpKind {
    Migration,
    Backup,
}

struct OpResult {
    kind: OpKind,
    status: Option<String>,
    error: Option<String>,
    clear_file: bool,
}

struct LoadResult {
    generation: u64,
    result: Result<MigrationStatus, String>,
}

/// One settings modal's data. Global on the shell, and owned by the
/// standalone transcript state for the headless settings scene.
pub struct SettingsState {
    pub open: bool,
    pub host: String,
    pub port: u16,
    pub active: String,
    pub candidates: Vec<MigrationCandidate>,
    pub status: String,
    pub error: Option<String>,
    /// A migration import is running.
    pub busy: bool,
    /// An export or backup import is running.
    pub backup_busy: bool,
    pub backup_file: Option<PathBuf>,
    pub backup_root: TextEdit,
    pub update: UpdateState,
    /// Native Stark Island + voice preferences (local, not server config).
    pub live: crate::prefs::LivePrefs,
    /// The Settings vocabulary field, edited as `term | spelling, spelling`
    /// lines and saved to `live.vocabulary` on change.
    pub vocabulary_edit: TextEdit,
    pub loading: bool,
    generation: u64,
    load_inbox: Option<mpsc::Receiver<LoadResult>>,
    op_inbox: Option<mpsc::Receiver<OpResult>>,
    /// Update worker events; `None` when no check/download is in flight.
    update_inbox: Option<mpsc::Receiver<UpdateEvent>>,
    /// Last automatic check, on the shell clock. The 6-hour cadence survives
    /// errors and does not retry every frame.
    update_last_check: Option<f64>,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            open: false,
            host: crate::rune_transcript::DEFAULT_API_HOST.to_string(),
            port: crate::rune_transcript::api_port(),
            active: String::new(),
            candidates: Vec::new(),
            status: String::new(),
            error: None,
            busy: false,
            backup_busy: false,
            backup_file: None,
            backup_root: TextEdit::new(""),
            update: UpdateState::default(),
            live: crate::prefs::LivePrefs::default(),
            vocabulary_edit: TextEdit::new(""),
            loading: false,
            generation: 0,
            load_inbox: None,
            op_inbox: None,
            update_inbox: None,
            update_last_check: None,
        }
    }
}

impl Clone for SettingsState {
    fn clone(&self) -> Self {
        Self {
            open: self.open,
            host: self.host.clone(),
            port: self.port,
            active: self.active.clone(),
            candidates: self.candidates.clone(),
            status: self.status.clone(),
            error: self.error.clone(),
            busy: self.busy,
            backup_busy: self.backup_busy,
            backup_file: self.backup_file.clone(),
            backup_root: self.backup_root.clone(),
            update: self.update.clone(),
            live: self.live.clone(),
            vocabulary_edit: self.vocabulary_edit.clone(),
            loading: self.loading,
            generation: self.generation,
            load_inbox: None,
            op_inbox: None,
            update_inbox: None,
            update_last_check: self.update_last_check,
        }
    }
}

impl std::fmt::Debug for SettingsState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsState")
            .field("open", &self.open)
            .field("active", &self.active)
            .field("island", &self.live.island)
            .field("auto_send_voice", &self.live.auto_send_voice)
            .field("candidates", &self.candidates.len())
            .field("status", &self.status)
            .field("error", &self.error)
            .field("busy", &self.busy)
            .field("backup_busy", &self.backup_busy)
            .field("backup_file", &self.backup_file)
            .field("update", &self.update.phase)
            .field("loading", &self.loading)
            .field("update_inbox", &self.update_inbox.is_some())
            .finish()
    }
}

/// Views mutate settings through their host state (shell or standalone
/// transcript); the sheet renders from the borrowed snapshot.
pub trait SettingsHost {
    fn settings(&self) -> &SettingsState;
    fn settings_mut(&mut self) -> &mut SettingsState;
}

impl SettingsState {
    /// Toggle the native Stark Island and persist it immediately.
    pub fn set_live_island(&mut self, on: bool) {
        self.live.island = on;
        crate::prefs::save(&self.live);
    }

    /// Toggle hands-free voice and persist it immediately.
    pub fn set_auto_send_voice(&mut self, on: bool) {
        self.live.auto_send_voice = on;
        crate::prefs::save(&self.live);
    }

    /// Toggle screen-capture exclusion for the island.
    pub fn set_island_capture_exclusion(&mut self, on: bool) {
        self.live.exclude_from_capture = on;
        crate::prefs::save(&self.live);
    }

    /// Save the Settings vocabulary field. Parsing is cheap, and an unchanged
    /// field is not written again.
    pub fn save_vocabulary_text(&mut self) {
        if self.live.set_vocabulary_text(&self.vocabulary_edit.text) {
            crate::prefs::save(&self.live);
        }
    }

    /// Persist a boost choice; the server reads it with the next dictation.
    pub fn set_vocabulary_boost(&mut self, boost: crate::prefs::VocabularyBoost) {
        if self.live.set_vocabulary_boost(boost) {
            crate::prefs::save(&self.live);
        }
    }

    /// Register/unregister the installed launcher for sign-in. A missing
    /// launcher is reported in the settings error line instead of writing a
    /// broken startup command.
    pub fn set_start_with_windows(&mut self, on: bool) {
        match crate::startup::set_enabled(on) {
            Ok(()) => {
                self.live.start_with_windows = on;
                self.error = None;
                crate::prefs::save(&self.live);
            }
            Err(error) => self.error = Some(error),
        }
    }

    /// Open and reload migration status, like `useEffect(() => load(), [isOpen])`.
    pub fn open(&mut self, host: String, port: u16) {
        if !self.open {
            self.vocabulary_edit = TextEdit::new(self.live.vocabulary_text());
        }
        self.open = true;
        self.host = host;
        self.port = port;
        if !self.loading {
            self.load();
        }
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    /// Reload `/api/migration/status`; failures stay silent like the reference UI's
    /// `.catch(() => null)` so a cold backend never blocks the modal.
    pub fn load(&mut self) {
        if self.loading {
            return;
        }
        self.loading = true;
        self.error = None;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let (tx, rx) = mpsc::channel();
        self.load_inbox = Some(rx);
        let (host, port) = (self.host.clone(), self.port);
        #[cfg(target_arch = "wasm32")]
        wasm_bindgen_futures::spawn_local(async move {
            let result = match crate::transport::fetch_json("/api/migration/status").await {
                Ok(v) => Ok(MigrationStatus::from_json(&v)),
                Err(e) => Err(e),
            };
            let _ = (host, port);
            let _ = tx.send(LoadResult { generation, result });
        });
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::spawn(move || {
            let result = match crate::transport::get_json(&host, port, "/api/migration/status") {
                Ok(v) => Ok(MigrationStatus::from_json(&v)),
                Err(e) => Err(e),
            };
            let _ = tx.send(LoadResult { generation, result });
        });
    }

    /// `runImport(candidate.id)` / `importMigration(folder)`.
    pub fn run_import(&mut self, source: String) {
        if self.busy || source.trim().is_empty() {
            return;
        }
        self.busy = true;
        self.error = None;
        self.status.clear();
        let (tx, rx) = mpsc::channel();
        self.op_inbox = Some(rx);
        let (host, port) = (self.host.clone(), self.port);
        #[cfg(target_arch = "wasm32")]
        wasm_bindgen_futures::spawn_local(async move {
            let body = serde_json::json!({ "source": source });
            let result =
                crate::transport::fetch_send("POST", "/api/migration/import", Some(&body)).await;
            let _ = (host, port);
            let _ = tx.send(migration_op(result));
        });
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::spawn(move || {
            let body = serde_json::json!({ "source": source });
            let result =
                crate::transport::send_json(&host, port, "POST", "/api/migration/import", &body);
            let _ = tx.send(migration_op(result));
        });
    }

    /// `runBackupImport()`: stage the picked zip for the next server start.
    pub fn run_backup_import(&mut self) {
        if self.backup_busy {
            return;
        }
        let Some(file) = self.backup_file.clone() else {
            return;
        };
        self.backup_busy = true;
        self.error = None;
        self.status.clear();
        let root = self.backup_root.text.clone();
        let (tx, rx) = mpsc::channel();
        self.op_inbox = Some(rx);
        #[cfg(target_arch = "wasm32")]
        {
            let _ = file;
            wasm_bindgen_futures::spawn_local(async move {
                let result = crate::transport::upload_backup_web(&root).await;
                if result.is_ok() {
                    crate::transport::clear_backup_file();
                }
                let _ = tx.send(backup_op(result));
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (host, port) = (self.host.clone(), self.port);
            std::thread::spawn(move || {
                let result = crate::transport::upload_backup(&host, port, &file, &root);
                let _ = tx.send(backup_op(result));
            });
        }
    }

    /// `runExport()` on the native app: choose a destination, then download
    /// The server's consistent zip snapshot into it.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn export_to(&mut self, path: PathBuf) {
        if self.backup_busy {
            return;
        }
        self.backup_busy = true;
        self.error = None;
        self.status.clear();
        let (tx, rx) = mpsc::channel();
        self.op_inbox = Some(rx);
        let (host, port) = (self.host.clone(), self.port);
        std::thread::spawn(move || {
            let result = crate::transport::download_file(&host, port, "/api/data/export").and_then(
                |download| {
                    std::fs::write(&path, &download.bytes)
                        .map(|_| download.filename)
                        .map_err(|e| format!("cannot write export: {e}"))
                },
            );
            let _ = tx.send(export_op(result));
        });
    }

    /// Web build: fetch the backup with the pairing header, then hand the
    /// bytes to a browser download (`window.open` cannot send the token).
    #[cfg(target_arch = "wasm32")]
    pub fn export_web(&mut self) {
        if self.backup_busy {
            return;
        }
        self.backup_busy = true;
        self.error = None;
        self.status.clear();
        let (tx, rx) = mpsc::channel();
        self.op_inbox = Some(rx);
        wasm_bindgen_futures::spawn_local(async move {
            let result =
                crate::transport::download_web("/api/data/export", "stark-backup.zip").await;
            let _ = tx.send(export_op(result));
        });
    }

    /// One worker at a time. A ready update is not re-checked until it applies.
    pub fn start_update_check(&mut self, manual: bool) {
        if self.update_inbox.is_some()
            || matches!(
                self.update.phase,
                UpdatePhase::Checking | UpdatePhase::Downloading | UpdatePhase::Ready
            )
        {
            return;
        }
        self.update = UpdateState {
            phase: UpdatePhase::Checking,
            message: if manual {
                "Checking for updates…".to_string()
            } else {
                String::new()
            },
            ..UpdateState::default()
        };
        let (tx, rx) = mpsc::channel();
        self.update_inbox = Some(rx);
        crate::updater::spawn_check(manual, tx);
    }

    /// Launch/five-minute master-feed check. `clock` is the shell's wall clock
    /// in seconds; installed builds only, so headless scenes and dev runs stay
    /// offline.
    pub fn auto_update(&mut self, clock: f64) {
        if self.update_inbox.is_some()
            || matches!(
                self.update.phase,
                UpdatePhase::Checking | UpdatePhase::Downloading | UpdatePhase::Ready
            )
        {
            return;
        }
        if crate::updater::auto_check_due(self.update_last_check, clock) {
            self.update_last_check = Some(clock);
            self.start_update_check(false);
        }
    }

    /// Seconds until the updater needs the shell again. While a worker is in
    /// flight this is short so progress events are drained on an idle screen;
    /// otherwise it is the next master-feed check. `None` means no wake is
    /// needed (dev build, ready update, or wasm).
    pub fn next_auto_check_in(&self, clock: f64) -> Option<f64> {
        if !self.desktop() {
            return None;
        }
        if self.update_inbox.is_some()
            || matches!(
                self.update.phase,
                UpdatePhase::Checking | UpdatePhase::Downloading
            )
        {
            return Some(0.1);
        }
        if self.update.phase == UpdatePhase::Ready {
            return None;
        }
        crate::updater::install_root()?;
        Some(match self.update_last_check {
            None => 0.0,
            Some(last) => (last + crate::updater::AUTO_CHECK_SECONDS - clock).max(0.0),
        })
    }

    /// The staged build waits for the shell's busy/draft gate.
    pub fn update_restart_ready(&self) -> bool {
        self.update.phase == UpdatePhase::Ready && self.update.restart_required
    }

    pub fn desktop(&self) -> bool {
        !cfg!(target_arch = "wasm32")
    }

    fn inactive_candidates(&self) -> impl Iterator<Item = &MigrationCandidate> {
        self.candidates.iter().filter(|candidate| !candidate.active)
    }
}

fn migration_op(result: Result<serde_json::Value, String>) -> OpResult {
    match result {
        Ok(v) => {
            let message = v
                .get("message")
                .and_then(|x| x.as_str())
                .unwrap_or("Migration complete.");
            let copied = v
                .get("copied")
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            OpResult {
                kind: OpKind::Migration,
                status: Some(format!("{message} Copied: {copied}.")),
                error: None,
                clear_file: false,
            }
        }
        Err(error) => OpResult {
            kind: OpKind::Migration,
            status: None,
            error: Some(error),
            clear_file: false,
        },
    }
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn backup_op(result: Result<serde_json::Value, String>) -> OpResult {
    match result {
        Ok(v) => {
            let from = v
                .get("from_root")
                .and_then(|x| x.as_str())
                .unwrap_or("unknown");
            let to = v.get("to_root").and_then(|x| x.as_str()).unwrap_or("?");
            OpResult {
                kind: OpKind::Backup,
                status: Some(format!(
                    "Backup staged ({from} → {to}). Restart the app/server to apply it. A pre-import copy was kept."
                )),
                error: None,
                clear_file: true,
            }
        }
        Err(error) => OpResult {
            kind: OpKind::Backup,
            status: None,
            error: Some(error),
            clear_file: false,
        },
    }
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn export_op(result: Result<String, String>) -> OpResult {
    match result {
        Ok(name) => OpResult {
            kind: OpKind::Backup,
            status: Some(format!(
                "Exported {name}. Keep it safe — it contains chats and config, never API keys."
            )),
            error: None,
            clear_file: false,
        },
        Err(error) => OpResult {
            kind: OpKind::Backup,
            status: None,
            error: Some(format!("Export failed: {error}")),
            clear_file: false,
        },
    }
}

/// Drain background settings work. Returns whether the view must commit.
pub fn poll_settings(s: &mut SettingsState) -> bool {
    let mut changed = false;
    while let Some(event) = s.update_inbox.as_ref().and_then(|rx| rx.try_recv().ok()) {
        changed |= crate::updater::apply_event(&mut s.update, event);
    }
    if s.update_inbox
        .as_ref()
        .is_some_and(|rx| matches!(rx.try_recv(), Err(mpsc::TryRecvError::Disconnected)))
    {
        s.update_inbox = None;
    }
    #[cfg(target_arch = "wasm32")]
    if let Some(name) = crate::transport::backup_file_name() {
        let current = s
            .backup_file
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str());
        if current != Some(name.as_str()) {
            s.backup_file = Some(PathBuf::from(name));
            changed = true;
        }
    }
    while let Some(msg) = s.load_inbox.as_ref().and_then(|rx| rx.try_recv().ok()) {
        if msg.generation == s.generation {
            s.loading = false;
            if let Ok(status) = msg.result {
                s.active = status.active;
                s.candidates = status.candidates;
            }
            changed = true;
        }
    }
    while let Some(op) = s.op_inbox.as_ref().and_then(|rx| rx.try_recv().ok()) {
        match op.kind {
            OpKind::Migration => {
                s.busy = false;
                if op.error.is_none() {
                    s.load();
                }
            }
            OpKind::Backup => s.backup_busy = false,
        }
        if let Some(status) = op.status {
            s.status = status;
        }
        if let Some(error) = op.error {
            s.error = Some(error);
        }
        if op.clear_file {
            s.backup_file = None;
        }
        changed = true;
    }
    if s.load_inbox
        .as_ref()
        .is_some_and(|rx| matches!(rx.try_recv(), Err(mpsc::TryRecvError::Disconnected)))
    {
        s.load_inbox = None;
    }
    if s.op_inbox
        .as_ref()
        .is_some_and(|rx| matches!(rx.try_recv(), Err(mpsc::TryRecvError::Disconnected)))
    {
        s.op_inbox = None;
    }
    changed
}

fn label_style() -> rune::tok::TextStyle {
    let mut style = text_style(10.0, 600.0);
    style.tracking = 1.4;
    style
}

fn text_style(size: f32, weight: f32) -> rune::tok::TextStyle {
    // Reference arbitrary text sizes inherit the body's 1.5 line-height:
    // 12/18, 11/16.5, 10.5/15.75, 11.5/17.25, 10/15.
    rune::tok::TextStyle::new(size, weight).lh(size * 1.5)
}

fn section(key: &str) -> El {
    col()
        .key(key)
        .w_full()
        .gap(10.0)
        .px(14.0)
        .py(14.0)
        .radius(16.0)
        .fill(rgba(1.0, 1.0, 1.0, 0.015))
        .border(1.0, rgba(1.0, 1.0, 1.0, 0.06))
        .border_layout()
}

fn section_head(label: &str, glyph: rune::Shape) -> El {
    row()
        .w_full()
        .items_center()
        .gap(8.0)
        .child(rune::el().size(13.0, 13.0).inert().shape(glyph))
        .child(
            text(label.to_string())
                .font(label_style())
                .text_case(rune::TextCase::Uppercase)
                .color(ink("#71717a")),
        )
}

fn row_card(key: &str) -> El {
    row()
        .key(key)
        .w_full()
        .items_start()
        .justify_between()
        .gap(12.0)
        .px(14.0)
        .py(10.0)
        .radius(12.0)
        .fill(rgba(1.0, 1.0, 1.0, 0.02))
        .border(1.0, rgba(1.0, 1.0, 1.0, 0.06))
        .border_layout()
}

fn heading(value: &str) -> El {
    text(value.to_string())
        .w_full()
        .wrap()
        .font(text_style(12.0, 500.0))
        .color(ink("#e4e4e7"))
}

fn muted(value: &str) -> El {
    muted_flat(value).mt(2.0)
}

fn muted_flat(value: &str) -> El {
    text(value.to_string())
        .w_full()
        .wrap()
        .font(text_style(10.5, 400.0))
        .color(ink("#71717a"))
}

fn paragraph(value: impl Into<String>) -> El {
    text(value.into())
        .w_full()
        .wrap()
        .font(text_style(11.0, 400.0))
        .color(ink("#71717a"))
}

/// Action button (`actionClass`); `primary` is the white export/import CTA.
fn action_button<S: SettingsHost + 'static>(
    key: impl Into<String>,
    label: &'static str,
    glyph: Option<rune::Shape>,
    primary: bool,
    disabled: bool,
    spin: bool,
    press: impl Fn(&mut S, &mut rune::Tx) + 'static,
) -> El {
    let mut el = row()
        .key(key.into())
        .role("button")
        .focusable(true)
        .label(label)
        .items_center()
        .gap(6.0)
        .px(12.0)
        .py(6.0)
        .radius(8.0);
    if let Some(glyph) = glyph {
        let mut glyph_el = rune::el().size(14.0, 14.0).inert().shape(glyph);
        if spin {
            glyph_el = glyph_el.looping(rune::LoopSpec::rotation(1.0));
        }
        el = el.child(glyph_el);
    }
    el = el.child(
        text(label.to_string())
            .font(text_style(11.5, if primary { 600.0 } else { 500.0 }))
            .color(ink(if primary { "#0a0a0d" } else { "#d4d4d8" })),
    );
    if primary {
        el = el
            .fill(ink("#ffffff"))
            // `hover:bg-zinc-200`: black at 10.6% over white is #e4e4e7.
            .hover(rgba(0.0, 0.0, 0.0, 0.106));
    } else {
        el = el
            .border(1.0, rgba(1.0, 1.0, 1.0, 0.10))
            .border_layout()
            .hover(rgba(1.0, 1.0, 1.0, 0.06));
    }
    if disabled {
        el.disabled(true).opacity(0.5)
    } else {
        el.on_press_tx_enter(press)
    }
}

/// `file:...` control: Rune has no DOM file input, so the picker row draws
/// The browser's two pieces (styled button + filename) with reference values.
fn file_row(key: &'static str, file: Option<&PathBuf>) -> El {
    let name = file
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("No file chosen")
        .to_string();
    row()
        .key(key)
        .w_full()
        .items_center()
        .gap(8.0)
        .role("button")
        .focusable(true)
        .label("Choose a backup file")
        .child(
            text("Choose File".to_string())
                .font(text_style(11.0, 400.0))
                .px(8.0)
                .py(4.0)
                .radius(6.0)
                .fill(rgba(1.0, 1.0, 1.0, 0.10))
                .color(ink("#e4e4e7")),
        )
        .child(
            text(name)
                .grow(1.0)
                .min_w(0.0)
                .truncate()
                .font(text_style(11.0, 400.0))
                .color(ink("#a1a1aa")),
        )
}

#[cfg(not(target_arch = "wasm32"))]
fn export_dialog<S: SettingsHost + 'static>(tx: &mut rune::Tx) {
    let options = rune::FileDialogOptions {
        title: "Export Stark data".to_string(),
        file_name: Some("stark-backup.zip".to_string()),
        filters: vec![("Stark backup".to_string(), vec!["zip".to_string()])],
        ..Default::default()
    };
    tx.save_file(options, |s: &mut S, result, _tx| match result {
        Ok(Some(path)) => s.settings_mut().export_to(path),
        Ok(None) => {}
        Err(error) => s.settings_mut().error = Some(error.to_string()),
    });
}

fn pick_backup<S: SettingsHost + 'static>(s: &mut S, tx: &mut rune::Tx) {
    if s.settings().backup_busy {
        return;
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = tx;
        s.settings_mut().error = None;
        crate::transport::pick_backup_file();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let options = rune::FileDialogOptions {
            title: "Import a Stark backup".to_string(),
            filters: vec![("Stark backup".to_string(), vec!["zip".to_string()])],
            ..Default::default()
        };
        tx.open_file(options, |s: &mut S, result, _tx| match result {
            Ok(Some(path)) => {
                let settings = s.settings_mut();
                settings.backup_file = Some(path);
                settings.error = None;
            }
            Ok(None) => {}
            Err(error) => s.settings_mut().error = Some(error.to_string()),
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn pick_import_folder<S: SettingsHost + 'static>(tx: &mut rune::Tx) {
    let options = rune::FileDialogOptions {
        title: "Import from an old harness root".to_string(),
        ..Default::default()
    };
    tx.open_folder(options, |s: &mut S, result, _tx| match result {
        Ok(Some(path)) => {
            let source = path.to_string_lossy().into_owned();
            s.settings_mut().run_import(source);
        }
        Ok(None) => {}
        Err(error) => s.settings_mut().error = Some(error.to_string()),
    });
}

/// A labelled switch row. The whole card is the hit target; the switch is
/// inert so the card's press owns the toggle.
fn toggle_row<S: SettingsHost + 'static>(
    key: &'static str,
    title: &'static str,
    detail: &'static str,
    on: bool,
    press: impl Fn(&mut S, &mut rune::Tx) + 'static,
) -> El {
    let track = if on {
        ink("#3ddc97").alpha(0.20)
    } else {
        rgba(1.0, 1.0, 1.0, 0.09)
    };
    let knob = if on { ink("#3ddc97") } else { ink("#8a8a93") };
    row_card(key)
        .items_center()
        .on_press_tx_enter(press)
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .child(heading(title))
                .child(muted(detail)),
        )
        .child(
            rune::el()
                .key(format!("{key}-switch"))
                .size(34.0, 20.0)
                .radius(10.0)
                .fill(track)
                .border(1.0, rgba(1.0, 1.0, 1.0, 0.08))
                .border_layout()
                .motion(rune::Spring::smooth(0.18, 0.0))
                .inert()
                .child(
                    rune::el()
                        .size(14.0, 14.0)
                        .radius(7.0)
                        .fill(knob)
                        .offset(if on { 17.0 } else { 3.0 }, 3.0)
                        .motion(rune::Spring::smooth(0.18, 0.0))
                        .inert(),
                ),
        )
}

/// The shared vocabulary Parakeet biases toward, and its rescoring strength.
/// A term is one line; "sounds like" spellings follow `|` as a comma list.
fn vocabulary_card<S: SettingsHost + 'static>(s: &SettingsState) -> El {
    let mut boosts = row().w_full().items_center().gap(4.0);
    for boost in crate::prefs::VocabularyBoost::ALL {
        let selected = s.live.vocabulary_boost == boost;
        boosts = boosts.child(
            text(boost.label().to_string())
                .key(format!("settings-vocabulary-boost-{}", boost.label()))
                .role("button")
                .focusable(true)
                .label(format!("Vocabulary boost {}", boost.label()))
                .px(9.0)
                .py(4.0)
                .radius(7.0)
                .font(text_style(10.5, if selected { 600.0 } else { 500.0 }))
                .color(ink(if selected { "#0a0a0d" } else { "#a1a1aa" }))
                .fill(if selected {
                    ink("#ffffff")
                } else {
                    rgba(1.0, 1.0, 1.0, 0.04)
                })
                .hover(rgba(1.0, 1.0, 1.0, 0.09))
                .on_press_tx_enter(move |s: &mut S, _tx: &mut rune::Tx| {
                    s.settings_mut().set_vocabulary_boost(boost);
                }),
        );
    }
    let count = s.live.vocabulary.len();
    let read = count.min(64);
    let detail = if count == 0 {
        "One term per line. Names and jargon Parakeet should spell right; an optional `|` adds sounds-like spellings (comma-separated).".to_string()
    } else if s.live.vocabulary_boost == crate::prefs::VocabularyBoost::Off {
        format!(
            "{count} {} saved; the boost is off, so Parakeet ignores them.",
            if count == 1 { "term" } else { "terms" }
        )
    } else if count > read {
        format!(
            "{count} terms; Parakeet reads the first {read}. Add sounds-like spellings after `|`."
        )
    } else {
        format!(
            "{count} {} · applies to the next dictation. Add sounds-like spellings after `|`.",
            if count == 1 { "term" } else { "terms" }
        )
    };
    col()
        .key("settings-vocabulary")
        .w_full()
        .gap(8.0)
        .px(14.0)
        .py(12.0)
        .radius(12.0)
        .fill(rgba(1.0, 1.0, 1.0, 0.02))
        .border(1.0, rgba(1.0, 1.0, 1.0, 0.06))
        .border_layout()
        .child(heading("Vocabulary"))
        .child(muted_flat(&detail))
        .child(
            text_input(&s.vocabulary_edit, text_style(11.0, 400.0))
                .key("settings-vocabulary-input")
                .w_full()
                .min_h(88.0)
                .px(8.0)
                .py(6.0)
                .radius(6.0)
                .fill(rgba(0.0, 0.0, 0.0, 0.30))
                .border(1.0, rgba(1.0, 1.0, 1.0, 0.10))
                .border_layout()
                .placeholder(
                    "Stark\nOpenCode | open code\nKaffar | caffer",
                    ink("#52525b"),
                )
                .color(ink("#e4e4e7"))
                .caret(ink("#e4e4e7"), crate::motion::TEXT_SELECTION)
                .focus_within_ring(
                    1.0,
                    -1.0,
                    rgba(99.0 / 255.0, 102.0 / 255.0, 241.0 / 255.0, 0.60),
                )
                .on_edit(|s: &mut S| &mut s.settings_mut().vocabulary_edit)
                .on_input_change(|s: &mut S, _tx: &mut rune::Tx| {
                    s.settings_mut().save_vocabulary_text();
                }),
        )
        .child(
            row()
                .w_full()
                .items_center()
                .gap(8.0)
                .child(
                    text("Boost".to_string())
                        .font(text_style(10.5, 500.0))
                        .color(ink("#71717a")),
                )
                .child(boosts),
        )
}

/// Settings sheet. `width`/`height` are the window (or standalone view)
/// bounds, so the panel uses the reference UI's viewport-relative min() caps.
pub fn settings_sheet<S: SettingsHost + 'static>(s: &SettingsState, width: f32, height: f32) -> El {
    let card_w = 680.0_f32.min((width - 32.0).max(0.0));
    let card_h = 640.0_f32.min((height - 40.0).max(0.0));
    let desktop = s.desktop();

    let head = row()
        .key("settings-head")
        .w_full()
        .items_center()
        .justify_between()
        .px(20.0)
        .py(16.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], rgba(1.0, 1.0, 1.0, 0.08))
        .border_layout()
        .child(
            row()
                .items_center()
                .gap(12.0)
                .child(
                    rune::el()
                        .key("settings-head-icon")
                        .size(36.0, 36.0)
                        .center()
                        .radius(8.0)
                        .fill(rgba(1.0, 1.0, 1.0, 0.04))
                        .border(1.0, rgba(1.0, 1.0, 1.0, 0.09))
                        .border_layout()
                        .child(
                            rune::el()
                                .size(18.0, 18.0)
                                .inert()
                                .shape(icon_settings(18.0, ink("#d4d4d8"))),
                        ),
                )
                .child(
                    col()
                        .child(
                            text("Settings".to_string())
                                .key("settings-title")
                                .font({
                                    let mut style = text_style(14.0, 600.0);
                                    style.line_height = 20.0; // text-sm
                                    style.tracking = -0.35;
                                    style
                                })
                                .color(ink("#f4f4f5")),
                        )
                        .child(
                            text(format!("Stark v{STARK_VERSION}"))
                                .key("settings-version")
                                .mt(2.0)
                                .font(text_style(11.0, 400.0))
                                .color(ink("#71717a")),
                        ),
                ),
        )
        .child(
            rune::el()
                .key("settings-close")
                .role("button")
                .focusable(true)
                .label("Close")
                .size(28.0, 28.0)
                .center()
                .radius(6.0)
                .hover(rgba(1.0, 1.0, 1.0, 0.05))
                .hover_group()
                .on_press_enter(|s: &mut S| s.settings_mut().close())
                .child({
                    let mut stack = rune::el().size(16.0, 16.0).inert().child(
                        rune::el().size(16.0, 16.0).inert().shape(crate::lucide_x(
                            16.0,
                            2.0,
                            ink("#a1a1aa"),
                        )),
                    );
                    stack = stack.child(
                        rune::el()
                            .size(16.0, 16.0)
                            .absolute(0.0, 0.0)
                            .inert()
                            .group_opacity(0.0, 1.0)
                            .shape(crate::lucide_x(16.0, 2.0, ink("#e4e4e7"))),
                    );
                    stack
                }),
        );

    // Stark Island + voice. These are local desktop preferences, not server
    // config, so they persist in `.deep_agents/desktop.json`.
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut live = section("settings-section-live")
        .child(section_head(
            "Stark Island",
            crate::shell::icon_live(13.0, ink("#71717a")),
        ))
        .child(paragraph(
            "Live mode warms the speech models and shows the island. The dock mic arms live mode; the mic in the island (or Ctrl+Space while armed) starts listening. Work chats carry their own Live control: Dictate stages speech as a draft, Live sends and speaks the reply, and Share screen attaches display frames to each turn. Turning the dock off unloads the models and hides the island.",
        ))
        .child(toggle_row::<S>(
            "settings-island-toggle",
            "Show Stark Island in live mode",
            "Off keeps the island hidden even while armed.",
            s.live.island,
            |s: &mut S, _tx: &mut rune::Tx| {
                let next = !s.settings().live.island;
                s.settings_mut().set_live_island(next);
            },
        ))
        .child(toggle_row::<S>(
            "settings-auto-send",
            "Auto-send voice",
            "Send a finished transcript immediately. Off stages it as a draft.",
            s.live.auto_send_voice,
            |s: &mut S, _tx: &mut rune::Tx| {
                let next = !s.settings().live.auto_send_voice;
                s.settings_mut().set_auto_send_voice(next);
            },
        ))
        .child(toggle_row::<S>(
            "settings-capture-exclusion",
            "Hide island from screen capture",
            "Keep the notch out of recordings and screenshots where Windows allows it.",
            s.live.exclude_from_capture,
            |s: &mut S, _tx: &mut rune::Tx| {
                let next = !s.settings().live.exclude_from_capture;
                s.settings_mut().set_island_capture_exclusion(next);
            },
        ))
        .child(vocabulary_card::<S>(s))
        .child(
            text("Ctrl+Space arms live mode, then toggles listening. Ctrl+Alt+S opens Stark.".to_string())
                .w_full()
                .wrap()
                .font(text_style(10.5, 400.0))
                .color(ink("#71717a")),
        );
    #[cfg(windows)]
    {
        live = live.child(toggle_row::<S>(
            "settings-start-with-windows",
            "Start with Windows",
            "Open the tray and island in the background when you sign in.",
            s.live.start_with_windows,
            |s: &mut S, _tx: &mut rune::Tx| {
                let next = !s.settings().live.start_with_windows;
                s.settings_mut().set_start_with_windows(next);
            },
        ));
    }

    // App updates.
    let updates = section("settings-section-updates")
        .child(section_head(
            "App updates",
            icon_refresh(13.0, ink("#71717a")),
        ))
        .child(
            row_card("settings-update-row")
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .child(heading(&format!("Stark v{STARK_VERSION}")))
                        .child(muted(if desktop {
                            "Checked on launch and every 5 minutes; new master builds download and apply automatically — no manual step."
                        } else {
                            "Web build — updates arrive when the server deploys."
                        }))
                        .children(
                            (s.update.phase != UpdatePhase::Idle
                                && s.update.phase != UpdatePhase::Unsupported)
                                .then(|| {
                                    text(s.update.message.clone())
                                        .key("settings-update-status")
                                        .mt(6.0)
                                        .font(text_style(11.0, 400.0))
                                        .color(ink(if s.update.phase == UpdatePhase::Error {
                                            "#f87171"
                                        } else {
                                            "#a1a1aa"
                                        }))
                                        .role("status")
                                }),
                        ),
                )
                .children(
                    (desktop && s.update_restart_ready()).then(|| {
                        action_button::<S>(
                            "settings-restart",
                            "Restart",
                            Some(icon_refresh(14.0, ink("#d4d4d8"))),
                            true,
                            false,
                            false,
                            |_s: &mut S, tx: &mut rune::Tx| tx.quit(),
                        )
                    }),
                ),
        );
    // Backup & move.
    let mut backup = section("settings-section-backup")
        .child(section_head(
            "Backup & move",
            icon_download(13.0, ink("#71717a")),
        ))
        .child(paragraph(
            "One portable zip: chats, sessions, config, memory, workspaces (relative paths) and a consistent database snapshot. API keys are excluded.",
        ))
        .child(
            row_card("settings-export-row")
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .child(heading("Export this machine"))
                        .child(muted(
                            "Downloads a timestamped zip you can import on another machine.",
                        )),
                )
                .child(action_button::<S>(
                    "settings-export",
                    "Export",
                    Some(icon_download(14.0, ink("#0a0a0d"))),
                    true,
                    s.backup_busy || s.busy,
                    false,
                    |s: &mut S, tx: &mut rune::Tx| {
                        if s.settings().backup_busy || s.settings().busy {
                            return;
                        }
                        #[cfg(target_arch = "wasm32")]
                        {
                            let _ = tx;
                            s.settings_mut().export_web();
                        }
                        #[cfg(not(target_arch = "wasm32"))]
                        export_dialog::<S>(tx);
                    },
                )),
        );

    backup = backup.child(
        row_card("settings-import-row")
            .child(
                col()
                    .grow(1.0)
                    .min_w(0.0)
                    .gap(6.0)
                    .child(heading("Import a backup"))
                    .child(muted_flat(
                        "Applied on the next start, after backing up the current data.",
                    ))
                    .child(
                        file_row("settings-backup-file", s.backup_file.as_ref()).on_press_tx_enter(
                            |s: &mut S, tx: &mut rune::Tx| pick_backup::<S>(s, tx),
                        ),
                    )
                    .child(
                        rune::text_input(&s.backup_root, text_style(11.0, 400.0))
                            .key("settings-backup-root")
                            .w_full()
                            .px(8.0)
                            .py(4.0)
                            .radius(6.0)
                            .fill(rgba(0.0, 0.0, 0.0, 0.30))
                            .border(1.0, rgba(1.0, 1.0, 1.0, 0.10))
                            .border_layout()
                            .placeholder("Projects root here (optional)", ink("#52525b"))
                            .color(ink("#e4e4e7"))
                            .caret(ink("#e4e4e7"), crate::motion::TEXT_SELECTION)
                            .focus_within_ring(
                                1.0,
                                -1.0,
                                rgba(99.0 / 255.0, 102.0 / 255.0, 241.0 / 255.0, 0.60),
                            )
                            .on_edit(|s: &mut S| &mut s.settings_mut().backup_root),
                    ),
            )
            .child(action_button::<S>(
                "settings-backup-import",
                "Import",
                Some(icon_upload(14.0, ink("#d4d4d8"))),
                false,
                s.backup_busy || s.busy || s.backup_file.is_none(),
                false,
                |s: &mut S, _tx: &mut rune::Tx| s.settings_mut().run_backup_import(),
            )),
    );

    // One-time migration.
    let inactive: Vec<MigrationCandidate> = s.inactive_candidates().cloned().collect();
    let mut migration = section("settings-section-migration")
        .child(
            section_head(
                "One-time migration",
                icon_hard_drive(13.0, ink("#71717a")),
            )
            .child(rune::el().grow(1.0))
            .child(
                rune::el()
                    .key("settings-refresh")
                    .role("button")
                .focusable(true)
                    .label("Refresh")
                    .size(21.0, 21.0)
                    .center()
                    .hover_group()
                    .disabled(s.busy)
                    .opacity(if s.busy { 0.5 } else { 1.0 })
                    .on_press_enter(|s: &mut S| s.settings_mut().load())
                    .child(
                        rune::el()
                            .size(13.0, 13.0)
                            .inert()
                            .child(
                                rune::el()
                                    .size(13.0, 13.0)
                                    .inert()
                                    .shape(icon_refresh(13.0, ink("#71717a"))),
                            )
                            .child(
                                rune::el()
                                    .size(13.0, 13.0)
                                    .absolute(0.0, 0.0)
                                    .inert()
                                    .group_opacity(0.0, 1.0)
                                    .shape(icon_refresh(13.0, ink("#e4e4e7"))),
                            ),
                    ),
            ),
        )
        .child({
            let active = if s.active.is_empty() {
                "—"
            } else {
                s.active.as_str()
            };
            let body = format!(
                "Import chats, sessions, and config from an old harness root into the active one ({active}). Existing files are backed up to .bak first."
            );
            let mut p = text(body.clone())
                .w_full()
                .wrap()
                .font({
                    // The mono `.bak` span adds 2pt to Rune's line box;
                    // 16.0 keeps the two-line paragraph on the reference UI's 34pt.
                    let mut style = text_style(11.0, 400.0);
                    style.line_height = 16.0;
                    style
                })
                .color(ink("#71717a"));
            if let Some(start) = body.find(".bak") {
                p = p.spans([rune::Span::new(start, start + 4).mono()]);
            }
            p
        });

    for candidate in &inactive {
        let id = candidate.id.clone();
        let disabled = s.busy || !candidate.has_data();
        migration = migration.child(
            row_card(&format!("settings-candidate-{}", candidate.id))
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .child(heading(&candidate.id))
                        .child(
                            text(candidate.path.clone())
                                .w_full()
                                .wrap_anywhere()
                                .font(text_style(11.0, 400.0).mono())
                                .color(ink("#71717a")),
                        )
                        .child(muted(&candidate.summary())),
                )
                .child(action_button::<S>(
                    format!("settings-import-{}", candidate.id),
                    "Import",
                    None,
                    true,
                    disabled,
                    false,
                    move |s: &mut S, _tx: &mut rune::Tx| s.settings_mut().run_import(id.clone()),
                )),
        );
    }
    if inactive.is_empty() {
        migration = migration.child(
            text("No other harness roots with data were found.".to_string())
                .font(text_style(11.0, 400.0))
                .color(ink("#71717a")),
        );
    }
    migration = migration.child(
        action_button::<S>(
            "settings-import-folder",
            "Import from a folder…",
            Some(icon_folder_open(14.0, ink("#d4d4d8"))),
            false,
            s.busy,
            false,
            |s: &mut S, tx: &mut rune::Tx| {
                if s.settings().busy {
                    return;
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let _ = tx;
                    s.settings_mut().error =
                        Some("Folder import is available in the desktop app.".to_string());
                }
                #[cfg(not(target_arch = "wasm32"))]
                pick_import_folder::<S>(tx);
            },
        )
        // A CSS inline-flex button does not stretch inside the section's
        // block flow; a Rune column would stretch it to 608pt.
        .self_start(),
    );

    let mut body = col()
        .key("settings-body")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .gap(14.0)
        .px(20.0)
        .pt(16.0)
        .pb(20.0)
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        })
        .child(live)
        .child(updates)
        .child(backup)
        .child(migration);
    if let Some(error) = &s.error {
        body = body.child(
            text(error.clone())
                .key("settings-error")
                .w_full()
                .radius(12.0)
                .border(1.0, rgba(239.0 / 255.0, 68.0 / 255.0, 68.0 / 255.0, 0.25))
                .border_layout()
                .fill(rgba(239.0 / 255.0, 68.0 / 255.0, 68.0 / 255.0, 0.07))
                .px(12.0)
                .py(8.0)
                .wrap()
                .font(text_style(11.5, 400.0))
                .color(ink("#fca5a5"))
                .role("alert")
                .live_region(rune::LiveRegion::Assertive),
        );
    }
    if !s.status.is_empty() {
        body = body.child(
            text(s.status.clone())
                .key("settings-status")
                .w_full()
                .radius(12.0)
                .border(1.0, rgba(1.0, 1.0, 1.0, 0.10))
                .border_layout()
                .fill(rgba(1.0, 1.0, 1.0, 0.04))
                .px(12.0)
                .py(8.0)
                .wrap()
                .font(text_style(11.5, 400.0))
                .color(ink("#d4d4d8"))
                .role("status"),
        );
    }

    let mut card = col()
        .key("settings-card")
        .w(card_w)
        .max_h(card_h)
        .radius(16.0)
        .fill(ink("#09090b"))
        .border(1.0, ink("#262626"))
        .border_layout()
        .clip_rounded(16.0)
        .box_shadow(rune::Shadow {
            x: 0.0,
            y: 25.0,
            blur: 50.0,
            spread: -12.0,
            color: rgba(0.0, 0.0, 0.0, 0.5),
            inset: false,
        })
        .hover_motion_within(crate::motion::HOVER, crate::motion::HOVER)
        .modal(true, |s: &mut S| s.settings_mut().close())
        .child(head)
        .child(body);
    if card_h > 0.0 {
        // The body scrolls only inside a bounded card; the reference UI's max-height
        // clamps at the same cap once the three sections overflow it.
        card = card.h(card_h);
    }
    // `.modal-panel-enter`: 240 ms fade + 10 pt rise + 0.98 settle. The reference UI
    // unmounts the sheet on close, so the removal is instant.
    card = card.transition(crate::motion::modal_enter());

    // Backdrop and panel are siblings, like the reference UI's two animated elements:
    // fading the full-screen container would fade the panel twice.
    let backdrop = rune::el()
        .key("settings-backdrop")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(rgba(0.0, 0.0, 0.0, 0.70))
        .backdrop_blur(12.0)
        // `.modal-backdrop-enter`: 200 ms opacity, instant on close.
        .transition(crate::motion::backdrop_enter());
    rune::el()
        .key("settings-overlay")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .center()
        .px(16.0)
        .py(16.0)
        .child(backdrop)
        .child(card)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Cx, Env, Runtime};

    struct Host {
        settings: SettingsState,
        w: f32,
        h: f32,
        reduced: bool,
    }

    impl SettingsHost for Host {
        fn settings(&self) -> &SettingsState {
            &self.settings
        }
        fn settings_mut(&mut self) -> &mut SettingsState {
            &mut self.settings
        }
    }

    fn view(s: &Host, cx: &mut Cx<Host>) -> El {
        cx.reduced_motion(s.reduced);
        // The shell only mounts the sheet while it is open; keeping that here
        // lets the entrance test exercise a real insertion, not the initial
        // view (which Rune treats as already present).
        if s.settings.open {
            settings_sheet::<Host>(&s.settings, s.w, s.h)
        } else {
            col().key("settings-test-root").w_full().h_full()
        }
    }

    fn host(w: f32, h: f32) -> Host {
        let settings = SettingsState {
            open: true,
            active: r"C:\Stark\harness".to_string(),
            candidates: vec![
                MigrationCandidate {
                    id: "appdata".to_string(),
                    path: r"C:\Users\davin\AppData\Roaming\com.stark.workspace".to_string(),
                    active: false,
                    has_chats_db: true,
                    has_sessions: true,
                    has_config: true,
                },
                MigrationCandidate {
                    id: "repo".to_string(),
                    path: r"D:\MyProject\LangChain".to_string(),
                    active: false,
                    has_chats_db: true,
                    has_sessions: false,
                    has_config: true,
                },
            ],
            ..SettingsState::default()
        };
        Host {
            settings,
            w,
            h,
            reduced: false,
        }
    }

    #[test]
    fn panel_uses_reference_viewport_caps() {
        for (w, h, expected_w, expected_h) in [
            (1280.0, 900.0, 680.0, 640.0),
            (800.0, 573.0, 680.0, 533.0),
            (468.0, 720.0, 436.0, 640.0),
        ] {
            let env = Env::new(true, 1.0);
            let mut rt = Runtime::new(host(w, h), view, (w, h), env);
            rt.frame(0.0);
            let (card, _) = rt.presented("settings-card").expect("settings card");
            eprintln!("{w}x{h} card {card:?}");
            assert!((card[2] - expected_w).abs() < 0.51, "{w}x{h}: {}", card[2]);
            assert!((card[3] - expected_h).abs() < 0.51, "{w}x{h}: {}", card[3]);
            assert!((card[0] - (w - expected_w) / 2.0).abs() < 0.51);
            assert!((card[1] - (h - expected_h) / 2.0).abs() < 0.51);
        }
    }

    #[test]
    fn reduced_motion_snaps_the_panel_rise_and_keeps_the_fade() {
        let env = Env::new(true, 1.0);
        let mut reduced = host(1280.0, 900.0);
        reduced.reduced = true;
        reduced.settings.open = false;
        let mut rt = Runtime::new(reduced, view, (1280.0, 900.0), env);
        rt.frame(0.0);
        assert!(rt.find("settings-card").is_none());
        rt.state.settings.open = true;
        rt.invalidate();
        rt.frame(0.0);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        let card = rt
            .elements()
            .into_iter()
            .find(|element| element.key.as_deref() == Some("settings-card"))
            .expect("settings card");
        assert!(
            (card.rect[1] - 130.0).abs() < 0.51,
            "reduced motion removes the 10pt rise, got {:?}",
            card.rect
        );
        assert!(
            card.opacity > 0.0 && card.opacity < 1.0,
            "the presence fade still runs under reduced motion, got {}",
            card.opacity
        );
    }

    #[test]
    fn active_candidate_is_hidden_and_old_placeholder_rows_are_gone() {
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(host(1280.0, 900.0), view, (1280.0, 900.0), env);
        rt.frame(0.0);
        assert!(rt.find("settings-section-updates").is_some());
        assert!(rt.find("settings-section-backup").is_some());
        assert!(rt.find("settings-section-migration").is_some());
        assert!(rt.find("settings-candidate-appdata").is_some());
        assert!(rt.find("settings-candidate-repo").is_some());
        for stale in [
            "settings-row-model",
            "settings-row-mic",
            "settings-row-theme",
            "settings-row-backend",
        ] {
            assert!(rt.find(stale).is_none(), "{stale} must not render");
        }
        // Backdrop press closes.
        rt.pointer_down(100.0, 100.0);
        rt.pointer_up(100.0, 100.0);
        rt.frame(0.0);
        assert!(!rt.state.settings.open, "backdrop dismisses");
        assert!(
            rt.find("settings-card").is_none(),
            "the reference UI unmounts the sheet in the frame it closes"
        );
    }

    #[test]
    fn status_and_error_surfaces_match_reference_copy() {
        let settings = SettingsState {
            open: true,
            error: Some("Migration failed.".to_string()),
            status:
                "Backup staged (a → b). Restart the app/server to apply it. A pre-import copy was kept."
                    .to_string(),
            ..SettingsState::default()
        };
        let env = Env::new(true, 1.0);
        let mut rt = Runtime::new(
            Host {
                settings,
                w: 1280.0,
                h: 900.0,
                reduced: false,
            },
            view,
            (1280.0, 900.0),
            env,
        );
        rt.frame(0.0);
        assert!(rt.find("settings-error").is_some());
        assert!(rt.find("settings-status").is_some());
    }

    #[test]
    fn contract_messages_match_settings_modal() {
        let op = migration_op(Ok(serde_json::json!({
            "ok": true,
            "message": "Imported appdata.",
            "copied": ["chats.db", "sessions"]
        })));
        assert_eq!(
            op.status.as_deref(),
            Some("Imported appdata. Copied: chats.db, sessions.")
        );
        let backup = backup_op(Ok(serde_json::json!({
            "ok": true,
            "from_root": r"C:\old",
            "to_root": r"C:\new"
        })));
        assert_eq!(
            backup.status.as_deref(),
            Some(
                r"Backup staged (C:\old → C:\new). Restart the app/server to apply it. A pre-import copy was kept."
            )
        );
        assert!(backup.clear_file);
        let export = export_op(Ok("stark-backup-20260101-120000.zip".to_string()));
        assert_eq!(
            export.status.as_deref(),
            Some(
                "Exported stark-backup-20260101-120000.zip. Keep it safe — it contains chats and config, never API keys."
            )
        );
    }

    #[test]
    fn updater_schedules_short_wakes_while_a_worker_is_in_flight() {
        let mut s = SettingsState::default();
        let (_tx, rx) = std::sync::mpsc::channel();
        s.update_inbox = Some(rx);
        assert_eq!(s.next_auto_check_in(0.0), Some(0.1));
        s.update_inbox = None;
        s.update.phase = UpdatePhase::Ready;
        assert_eq!(s.next_auto_check_in(0.0), None);
    }

    #[test]
    fn poll_applies_status_candidates_and_op_messages() {
        let (ltx, lrx) = mpsc::channel();
        let (otx, orx) = mpsc::channel();
        let mut s = SettingsState {
            loading: true,
            generation: 7,
            load_inbox: Some(lrx),
            op_inbox: Some(orx),
            ..SettingsState::default()
        };
        ltx.send(LoadResult {
            generation: 7,
            result: Ok(MigrationStatus {
                active: r"C:\active".to_string(),
                candidates: vec![MigrationCandidate {
                    id: "old".to_string(),
                    path: r"C:\old".to_string(),
                    has_chats_db: true,
                    ..MigrationCandidate::default()
                }],
            }),
        })
        .unwrap();
        otx.send(OpResult {
            kind: OpKind::Backup,
            status: Some("Exported x.zip. Keep it safe".to_string()),
            error: None,
            clear_file: true,
        })
        .unwrap();
        assert!(poll_settings(&mut s));
        assert_eq!(s.active, r"C:\active");
        assert_eq!(s.candidates.len(), 1);
        assert!(!s.loading);
        assert_eq!(s.status, "Exported x.zip. Keep it safe");
        assert!(s.backup_file.is_none());
    }
}
