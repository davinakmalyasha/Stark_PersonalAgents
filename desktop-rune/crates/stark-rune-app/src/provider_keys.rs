//! Provider panel: OpenCode Go allowance + API-key management.
//!
//! The prompt-dock provider pill opens this sheet. Keys are global to the
//! harness, so the panel writes through the local server instead of keeping
//! anything in the UI. Only masked values ever come back; the entered secret
//! lives in a secure [`TextEdit`] and is cleared after a successful save.
//!
//! Contract:
//! - `GET /api/provider/keys` -> provider metadata + masked keys
//! - `POST /api/provider/keys {label,secret}` -> validate, save, activate
//! - `POST /api/provider/keys/active {id}` -> use `env` or a saved key
//! - `DELETE /api/provider/keys/{id}` -> remove a managed key
//! - `GET /api/provider/usage?key_id=...` -> that key's Go allowance

use std::sync::mpsc::Receiver;

use crate::keys::EnterPress;
use rune::{col, row, text, text_input, El, TextEdit};

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsageWindow {
    pub status: String,
    pub percent: f64,
    pub resets_at: String,
}

impl UsageWindow {
    fn from_json(value: &serde_json::Value) -> Option<Self> {
        if !value.is_object() {
            return None;
        }
        Some(Self {
            status: value
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string(),
            percent: value
                .get("percent")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0)
                .clamp(0.0, 100.0),
            resets_at: value
                .get("resets_at")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string(),
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProviderUsage {
    pub rolling: Option<UsageWindow>,
    pub weekly: Option<UsageWindow>,
    pub monthly: Option<UsageWindow>,
    pub error: String,
}

impl ProviderUsage {
    fn from_json(value: &serde_json::Value) -> Self {
        let usage = value.get("usage").filter(|usage| usage.is_object());
        Self {
            rolling: usage
                .and_then(|usage| usage.get("rolling"))
                .and_then(UsageWindow::from_json),
            weekly: usage
                .and_then(|usage| usage.get("weekly"))
                .and_then(UsageWindow::from_json),
            monthly: usage
                .and_then(|usage| usage.get("monthly"))
                .and_then(UsageWindow::from_json),
            error: value
                .get("error")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string(),
        }
    }

    fn empty(&self) -> bool {
        self.rolling.is_none() && self.weekly.is_none() && self.monthly.is_none()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProviderKeyRow {
    pub id: String,
    pub label: String,
    pub masked: String,
    pub source: String,
    pub active: bool,
}

impl ProviderKeyRow {
    fn from_json(value: &serde_json::Value) -> Self {
        let str_of = |key: &str| {
            value
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        Self {
            id: str_of("id"),
            label: str_of("label"),
            masked: str_of("masked"),
            source: str_of("source"),
            active: value
                .get("active")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        }
    }
}

/// One `/api/provider/keys` payload.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProviderKeysPayload {
    pub provider: String,
    pub label: String,
    pub base_url: String,
    pub model: String,
    pub active: String,
    pub has_key: bool,
    pub keys: Vec<ProviderKeyRow>,
    pub issues: Vec<String>,
    pub message: String,
}

impl ProviderKeysPayload {
    fn from_json(value: &serde_json::Value) -> Self {
        let str_of = |key: &str| {
            value
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        Self {
            provider: str_of("provider"),
            label: str_of("label"),
            base_url: str_of("base_url"),
            model: str_of("model"),
            active: str_of("active"),
            has_key: value
                .get("has_key")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            keys: value
                .get("keys")
                .and_then(serde_json::Value::as_array)
                .map(|keys| keys.iter().map(ProviderKeyRow::from_json).collect())
                .unwrap_or_default(),
            issues: value
                .get("issues")
                .and_then(serde_json::Value::as_array)
                .map(|issues| {
                    issues
                        .iter()
                        .filter_map(|issue| issue.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            message: str_of("message"),
        }
    }
}

pub struct ProviderKeysOp {
    pub(crate) status: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) payload: Option<ProviderKeysPayload>,
}

pub struct ProviderKeysState {
    pub host: String,
    pub port: u16,
    pub open: bool,
    pub loading: bool,
    pub busy: bool,
    pub provider: String,
    pub label: String,
    pub base_url: String,
    pub model: String,
    pub active: String,
    pub has_key: bool,
    pub keys: Vec<ProviderKeyRow>,
    pub issues: Vec<String>,
    pub usage: ProviderUsage,
    pub usage_loading: bool,
    /// Active key the current usage snapshot belongs to.
    pub usage_for: String,
    pub add_open: bool,
    pub label_edit: TextEdit,
    pub key_edit: TextEdit,
    pub reveal: bool,
    pub confirm_remove: Option<String>,
    pub status: String,
    pub error: Option<String>,
    /// A write succeeded; the shell refreshes the pill from `has_key`.
    pub dirty: bool,
    pub inbox: Option<Receiver<Result<ProviderKeysPayload, String>>>,
    pub usage_inbox: Option<Receiver<(String, Result<ProviderUsage, String>)>>,
    pub op_inbox: Option<Receiver<ProviderKeysOp>>,
}

impl std::fmt::Debug for ProviderKeysState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderKeysState")
            .field("open", &self.open)
            .field("provider", &self.provider)
            .field("keys", &self.keys.len())
            .field("active", &self.active)
            .field("busy", &self.busy)
            .field("usage_loading", &self.usage_loading)
            .finish()
    }
}

impl Clone for ProviderKeysState {
    fn clone(&self) -> Self {
        Self {
            host: self.host.clone(),
            port: self.port,
            open: self.open,
            loading: self.loading,
            busy: self.busy,
            provider: self.provider.clone(),
            label: self.label.clone(),
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            active: self.active.clone(),
            has_key: self.has_key,
            keys: self.keys.clone(),
            issues: self.issues.clone(),
            usage: self.usage.clone(),
            usage_loading: self.usage_loading,
            usage_for: self.usage_for.clone(),
            add_open: self.add_open,
            label_edit: self.label_edit.clone(),
            key_edit: self.key_edit.clone(),
            reveal: self.reveal,
            confirm_remove: self.confirm_remove.clone(),
            status: self.status.clone(),
            error: self.error.clone(),
            dirty: self.dirty,
            inbox: None,
            usage_inbox: None,
            op_inbox: None,
        }
    }
}

impl Default for ProviderKeysState {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8124,
            open: false,
            loading: false,
            busy: false,
            provider: String::new(),
            label: "OpenCode Go".to_string(),
            base_url: String::new(),
            model: String::new(),
            active: String::new(),
            has_key: false,
            keys: Vec::new(),
            issues: Vec::new(),
            usage: ProviderUsage::default(),
            usage_loading: false,
            usage_for: String::new(),
            add_open: false,
            label_edit: TextEdit::new(""),
            key_edit: TextEdit::new("").secure(),
            reveal: false,
            confirm_remove: None,
            status: String::new(),
            error: None,
            dirty: false,
            inbox: None,
            usage_inbox: None,
            op_inbox: None,
        }
    }
}

fn reset_form(state: &mut ProviderKeysState) {
    state.label_edit = TextEdit::new("");
    state.key_edit = TextEdit::new("").secure();
    state.reveal = false;
    state.add_open = false;
    state.confirm_remove = None;
}

pub fn open_provider_keys(state: &mut ProviderKeysState, host: String, port: u16) {
    state.host = host;
    state.port = port;
    state.open = true;
    state.status.clear();
    state.error = None;
    state.usage = ProviderUsage::default();
    state.usage_for.clear();
    reset_form(state);
    refresh_keys(state);
}

pub fn close_provider_keys(state: &mut ProviderKeysState) {
    state.open = false;
    state.status.clear();
    state.error = None;
    reset_form(state);
}

fn apply_payload(state: &mut ProviderKeysState, payload: ProviderKeysPayload) {
    let active_changed = state.active != payload.active;
    state.provider = payload.provider.clone();
    if !payload.label.is_empty() {
        state.label = payload.label;
    }
    state.base_url = payload.base_url;
    state.model = payload.model;
    state.active = payload.active;
    state.has_key = payload.has_key;
    state.keys = payload.keys;
    if active_changed {
        // Never show the previous key's meters under the new key's label.
        state.usage = ProviderUsage::default();
        state.usage_for.clear();
        state.usage_loading = false;
    }
    state.issues = payload.issues;
    state.error = None;
    state.status = payload.message;
    state.dirty = true;
    if state
        .confirm_remove
        .as_deref()
        .is_some_and(|id| !state.keys.iter().any(|key| key.id == id))
    {
        state.confirm_remove = None;
    }
}

/// Load the list, then that key's allowance.
pub fn refresh_keys(state: &mut ProviderKeysState) {
    if state.loading {
        return;
    }
    state.loading = true;
    state.error = None;
    let (tx, rx) = std::sync::mpsc::channel();
    state.inbox = Some(rx);
    let (host, port) = (state.host.clone(), state.port);
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let result = match crate::transport::fetch_json("/api/provider/keys").await {
            Ok(value) => Ok(ProviderKeysPayload::from_json(&value)),
            Err(error) => Err(error),
        };
        let _ = tx.send(result);
        let _ = (host, port);
    });
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let result = match crate::transport::get_json(&host, port, "/api/provider/keys") {
            Ok(value) => Ok(ProviderKeysPayload::from_json(&value)),
            Err(error) => Err(error),
        };
        let _ = tx.send(result);
    });
}

fn refresh_usage(state: &mut ProviderKeysState) {
    if state.usage_loading || state.active.is_empty() {
        if state.active.is_empty() {
            state.usage = ProviderUsage {
                error: "No API key is configured.".to_string(),
                ..ProviderUsage::default()
            };
            state.usage_for.clear();
        }
        return;
    }
    state.usage_loading = true;
    state.usage.error.clear();
    let key_id = state.active.clone();
    let provider = state.provider.clone();
    let path = format!(
        "/api/provider/usage?provider={}&key_id={}",
        provider, key_id
    );
    let (tx, rx) = std::sync::mpsc::channel();
    state.usage_inbox = Some(rx);
    let (host, port) = (state.host.clone(), state.port);
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let result = match crate::transport::fetch_json(&path).await {
            Ok(value) => Ok(ProviderUsage::from_json(&value)),
            Err(error) => Err(error),
        };
        let _ = tx.send((key_id, result));
        let _ = (host, port);
    });
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let result = match crate::transport::get_json(&host, port, &path) {
            Ok(value) => Ok(ProviderUsage::from_json(&value)),
            Err(error) => Err(error),
        };
        let _ = tx.send((key_id, result));
    });
}

fn op_sender(state: &mut ProviderKeysState) -> std::sync::mpsc::Sender<ProviderKeysOp> {
    if state.op_inbox.is_none() {
        let (tx, rx) = std::sync::mpsc::channel();
        state.op_inbox = Some(rx);
        return tx;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    if let Some(old) = state.op_inbox.replace(rx) {
        while let Ok(message) = old.try_recv() {
            let _ = tx.send(message);
        }
    }
    let (tx2, rx2) = std::sync::mpsc::channel();
    state.op_inbox = Some(rx2);
    tx2
}

/// Validate, save, and activate the key in the add form.
pub fn save_provider_key(state: &mut ProviderKeysState) {
    if state.busy {
        return;
    }
    let secret = state.key_edit.text.trim().to_string();
    if secret.is_empty() {
        state.error = Some("Enter an API key first.".to_string());
        return;
    }
    let label = state.label_edit.text.trim().to_string();
    state.busy = true;
    state.error = None;
    state.status = "Checking key…".to_string();
    let body = serde_json::json!({ "label": label, "secret": secret });
    let tx = op_sender(state);
    let (host, port) = (state.host.clone(), state.port);
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let result =
            match crate::transport::fetch_send("POST", "/api/provider/keys", Some(&body)).await {
                Ok(value) => Ok(ProviderKeysPayload::from_json(&value)),
                Err(error) => Err(error),
            };
        let _ = tx.send(op_result(result));
        let _ = (host, port);
    });
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let result =
            match crate::transport::send_json(&host, port, "POST", "/api/provider/keys", &body) {
                Ok(value) => Ok(ProviderKeysPayload::from_json(&value)),
                Err(error) => Err(error),
            };
        let _ = tx.send(op_result(result));
    });
}

/// Use a saved key (or `env`) immediately.
pub fn activate_provider_key(state: &mut ProviderKeysState, id: &str) {
    if state.busy || id.is_empty() {
        return;
    }
    state.busy = true;
    state.error = None;
    state.confirm_remove = None;
    let body = serde_json::json!({ "id": id });
    let tx = op_sender(state);
    let (host, port) = (state.host.clone(), state.port);
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let result =
            match crate::transport::fetch_send("POST", "/api/provider/keys/active", Some(&body))
                .await
            {
                Ok(value) => Ok(ProviderKeysPayload::from_json(&value)),
                Err(error) => Err(error),
            };
        let _ = tx.send(op_result(result));
        let _ = (host, port);
    });
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let result = match crate::transport::send_json(
            &host,
            port,
            "POST",
            "/api/provider/keys/active",
            &body,
        ) {
            Ok(value) => Ok(ProviderKeysPayload::from_json(&value)),
            Err(error) => Err(error),
        };
        let _ = tx.send(op_result(result));
    });
}

/// Remove a managed key. The `.env` fallback row has no remove action.
pub fn remove_provider_key(state: &mut ProviderKeysState, id: &str) {
    if state.busy || id.is_empty() || id == "env" {
        return;
    }
    state.busy = true;
    state.error = None;
    let path = format!("/api/provider/keys/{}?provider={}", id, state.provider);
    let tx = op_sender(state);
    let (host, port) = (state.host.clone(), state.port);
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let result = match crate::transport::fetch_send("DELETE", &path, None).await {
            Ok(value) => Ok(ProviderKeysPayload::from_json(&value)),
            Err(error) => Err(error),
        };
        let _ = tx.send(op_result(result));
        let _ = (host, port);
    });
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let result = match crate::transport::send_json(
            &host,
            port,
            "DELETE",
            &path,
            &serde_json::Value::Null,
        ) {
            Ok(value) => Ok(ProviderKeysPayload::from_json(&value)),
            Err(error) => Err(error),
        };
        let _ = tx.send(op_result(result));
    });
}

fn op_result(result: Result<ProviderKeysPayload, String>) -> ProviderKeysOp {
    match result {
        Ok(payload) => ProviderKeysOp {
            status: (!payload.message.is_empty()).then(|| payload.message.clone()),
            error: None,
            payload: Some(payload),
        },
        Err(error) => ProviderKeysOp {
            status: None,
            error: Some(error),
            payload: None,
        },
    }
}

/// Drain every request. Returns true when the pill or panel should redraw.
pub fn poll_provider_keys(state: &mut ProviderKeysState) -> bool {
    let mut changed = false;
    while let Some(result) = state.inbox.as_ref().and_then(|rx| rx.try_recv().ok()) {
        match result {
            Ok(payload) => {
                apply_payload(state, payload);
                state.usage_for.clear();
            }
            Err(error) => state.error = Some(error),
        }
        state.loading = false;
        changed = true;
    }
    while let Some(op) = state.op_inbox.as_ref().and_then(|rx| rx.try_recv().ok()) {
        state.busy = false;
        if let Some(payload) = op.payload {
            let success = op.error.is_none();
            apply_payload(state, payload);
            if success {
                reset_form(state);
            }
        }
        if let Some(error) = op.error {
            state.error = Some(error);
        } else if let Some(status) = op.status {
            state.status = status;
        }
        changed = true;
    }
    while let Some((key, result)) = state.usage_inbox.as_ref().and_then(|rx| rx.try_recv().ok()) {
        state.usage_loading = false;
        if key == state.active {
            match result {
                Ok(usage) => state.usage = usage,
                Err(error) => {
                    state.usage = ProviderUsage {
                        error,
                        ..ProviderUsage::default()
                    }
                }
            }
            state.usage_for = key;
        }
        changed = true;
    }
    if !state.loading
        && !state.active.is_empty()
        && state.usage_for != state.active
        && !state.usage_loading
    {
        refresh_usage(state);
        changed = true;
    }
    let dead = state.inbox.as_ref().is_some_and(|rx| {
        matches!(
            rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        )
    });
    if dead {
        state.inbox = None;
    }
    let usage_dead = state.usage_inbox.as_ref().is_some_and(|rx| {
        matches!(
            rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        )
    });
    if usage_dead {
        state.usage_inbox = None;
    }
    let op_dead = state.op_inbox.as_ref().is_some_and(|rx| {
        matches!(
            rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        )
    });
    if op_dead {
        state.op_inbox = None;
    }
    changed
}

fn field_label(label: &str) -> El {
    text(label.to_string())
        .font(rune::tok::TextStyle::new(10.0, 600.0))
        .color(ink("#71717a"))
}

fn reset_label(window: &UsageWindow) -> String {
    let Some(ms) = crate::clock::iso_millis(&window.resets_at) else {
        return window.status.clone();
    };
    let now = crate::clock::now_millis();
    if ms <= now {
        return "resetting…".to_string();
    }
    let seconds = (ms - now) / 1000;
    let label = if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3_600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86_400 {
        let hours = seconds / 3_600;
        let minutes = (seconds % 3_600) / 60;
        if minutes == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h {minutes}m")
        }
    } else {
        let days = seconds / 86_400;
        let hours = (seconds % 86_400) / 3_600;
        if hours == 0 {
            format!("{days}d")
        } else {
            format!("{days}d {hours}h")
        }
    };
    let reset = format!("resets in {label}");
    if window.status.is_empty() || window.status == "ok" {
        reset
    } else {
        format!("{} · {reset}", window.status)
    }
}

fn usage_color(percent: f64) -> rune::Color {
    if percent >= 85.0 {
        ink("#f87171")
    } else if percent >= 60.0 {
        ink("#fbbf24")
    } else {
        ink("#818cf8")
    }
}

fn usage_row(label: &str, window: &Option<UsageWindow>) -> El {
    let mut out = row().w_full().items_center().gap(10.0).child(
        text(label.to_string())
            .w(74.0)
            .font(rune::tok::TextStyle::new(11.5, 500.0))
            .color(ink("#a1a1aa")),
    );
    match window {
        Some(window) => {
            let percent = window.percent;
            let color = usage_color(percent);
            let grow = percent.clamp(0.0, 100.0).max(0.6) as f32;
            let rest = (100.0 - percent).clamp(0.0, 100.0).max(0.6) as f32;
            out = out
                .child(
                    row()
                        .grow(1.0)
                        .h(5.0)
                        .radius(3.0)
                        .fill(ink("#ffffff10"))
                        .child(rune::el().grow(grow).h(5.0).fill(color).radius(3.0))
                        .child(rune::el().grow(rest).h(5.0)),
                )
                .child(
                    text(format!("{}%", percent.round() as i64))
                        .w(38.0)
                        .font(rune::tok::TextStyle::new(11.0, 600.0).mono())
                        .color(color),
                )
                .child(
                    text(reset_label(window))
                        .w(130.0)
                        .font(rune::tok::TextStyle::new(9.5, 400.0))
                        .color(ink("#71717a")),
                );
        }
        None => {
            out = out
                .child(rune::el().grow(1.0))
                .child(
                    text("—".to_string())
                        .w(38.0)
                        .font(rune::tok::TextStyle::new(11.0, 500.0))
                        .color(ink("#52525b")),
                )
                .child(
                    text("no data".to_string())
                        .w(130.0)
                        .font(rune::tok::TextStyle::new(9.5, 400.0))
                        .color(ink("#52525b")),
                );
        }
    }
    out
}

/// Provider sheet. `which` resolves close/actions to the owning thread.
pub fn provider_keys_sheet<S: crate::rune_transcript::TranscriptHost + 'static>(
    state: &ProviderKeysState,
    which: crate::rune_transcript::Thread,
    width: f32,
    height: f32,
) -> El {
    let card_w = 560.0_f32.min((width - 24.0).max(0.0));
    let max_card_h = 640.0_f32.min((height - 36.0).max(0.0));
    // The sheet hugs its content (one key is a small panel) but grows for
    // the add form and extra keys, then scrolls at the window cap.
    let key_rows = state.keys.len().max(1) as f32;
    let form_h = if state.add_open { 196.0 } else { 0.0 };
    let notice_h = if state.error.is_some() {
        48.0
    } else if !state.status.is_empty() && !state.add_open {
        38.0
    } else {
        0.0
    };
    let issues_h = state.issues.len() as f32 * 14.0;
    let content_h = 62.0
        + 32.0
        + 34.0
        + 16.0
        + 150.0
        + 16.0
        + 40.0
        + key_rows * 62.0
        + 16.0
        + 38.0
        + form_h
        + notice_h
        + issues_h;
    let card_h = content_h.min(max_card_h).max(360.0);
    let close = move |s: &mut S| {
        close_provider_keys(&mut s.thread_mut(which).provider_keys);
    };
    let provider = state.provider.clone();
    let label = state.label.clone();
    let model = state.model.clone();
    let has_key = state.has_key;
    let active_label = state
        .keys
        .iter()
        .find(|key| key.active)
        .map(|key| key.label.clone())
        .unwrap_or_else(|| "No active key".to_string());
    let mut body = col()
        .key("providerkeys-body")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .gap(16.0)
        .px(20.0)
        .pt(14.0)
        .pb(18.0)
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        });

    body = body.child(
        row()
            .key("providerkeys-hero")
            .w_full()
            .items_center()
            .gap(12.0)
            .child(
                rune::el()
                    .size(34.0, 34.0)
                    .radius(10.0)
                    .fill(ink("#ffffff0a"))
                    .border(1.0, ink("#ffffff12"))
                    .center()
                    .child(
                        text("◎".to_string())
                            .font(rune::tok::TextStyle::new(17.0, 500.0))
                            .color(ink("#c7d2fe")),
                    ),
            )
            .child(
                col()
                    .grow(1.0)
                    .min_w(0.0)
                    .gap(2.0)
                    .child(
                        text(label.clone())
                            .key("providerkeys-title")
                            .font(rune::tok::TextStyle::new(15.0, 600.0))
                            .color(ink("#f4f4f5")),
                    )
                    .child(
                        text(if model.is_empty() {
                            provider.clone()
                        } else {
                            format!("{provider} · {model}")
                        })
                        .key("providerkeys-model")
                        .font(rune::tok::TextStyle::new(10.5, 400.0).mono())
                        .color(ink("#71717a")),
                    ),
            )
            .child(
                text(if has_key { "READY" } else { "NO KEY" }.to_string())
                    .key("providerkeys-ready")
                    .font(rune::tok::TextStyle::new(9.5, 700.0))
                    .px(8.0)
                    .py(3.0)
                    .radius(999.0)
                    .fill(ink(if has_key { "#6ee7b712" } else { "#fbbf2412" }))
                    .border(1.0, ink(if has_key { "#6ee7b740" } else { "#fbbf244d" }))
                    .color(ink(if has_key { "#6ee7b7" } else { "#fbbf24" })),
            ),
    );

    // Allowance card.
    let mut usage_card = col()
        .key("providerkeys-usage")
        .w_full()
        .gap(10.0)
        .px(14.0)
        .py(14.0)
        .radius(12.0)
        .fill(ink("#ffffff05"))
        .border(1.0, ink("#ffffff10"))
        .child(
            row()
                .w_full()
                .items_center()
                .gap(8.0)
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .child(
                            text("OpenCode Go allowance".to_string())
                                .font(rune::tok::TextStyle::new(11.5, 600.0))
                                .color(ink("#e4e4e7")),
                        )
                        .child(
                            text(format!(
                                "{active_label} · 5-hour, weekly and monthly limits"
                            ))
                            .font(rune::tok::TextStyle::new(9.5, 400.0))
                            .color(ink("#71717a")),
                        ),
                )
                .child(
                    text(
                        if state.usage_loading {
                            "checking…"
                        } else {
                            "refresh"
                        }
                        .to_string(),
                    )
                    .key("providerkeys-usage-refresh")
                    .role("button")
                    .focusable(true)
                    .label("Refresh OpenCode Go usage")
                    .font(rune::tok::TextStyle::new(10.0, 500.0))
                    .color(ink("#a1a1aa"))
                    .px(8.0)
                    .py(4.0)
                    .radius(6.0)
                    .fill(ink("#ffffff08"))
                    .on_press_enter(move |s: &mut S| {
                        let st = &mut s.thread_mut(which).provider_keys;
                        if st.usage_loading {
                            return;
                        }
                        st.usage_for.clear();
                        refresh_usage(st);
                    }),
                ),
        )
        .child(usage_row("5-hour", &state.usage.rolling))
        .child(usage_row("Weekly", &state.usage.weekly))
        .child(usage_row("Monthly", &state.usage.monthly));
    if !state.usage.error.is_empty() {
        usage_card = usage_card.child(
            text(state.usage.error.clone())
                .key("providerkeys-usage-error")
                .w_full()
                .wrap()
                .font(rune::tok::TextStyle::new(10.0, 400.0))
                .color(ink("#fca5a5")),
        );
    } else if state.usage.empty() && !state.usage_loading {
        usage_card = usage_card.child(
            text("No usage data for the active key.".to_string())
                .font(rune::tok::TextStyle::new(10.0, 400.0))
                .color(ink("#71717a")),
        );
    }
    body = body.child(usage_card);

    // Keys.
    let mut keys = col().key("providerkeys-keys").w_full().gap(8.0).child(
        row()
            .w_full()
            .items_center()
            .gap(8.0)
            .child(
                col()
                    .grow(1.0)
                    .min_w(0.0)
                    .child(
                        text("API keys".to_string())
                            .font(rune::tok::TextStyle::new(11.5, 600.0))
                            .color(ink("#e4e4e7")),
                    )
                    .child(
                        text("Click a key to use it. Its own usage loads above.".to_string())
                            .font(rune::tok::TextStyle::new(9.5, 400.0))
                            .color(ink("#71717a")),
                    ),
            )
            .child(
                text(
                    if state.add_open {
                        "Cancel"
                    } else {
                        "+ Add key"
                    }
                    .to_string(),
                )
                .key("providerkeys-add-toggle")
                .role("button")
                .focusable(true)
                .label(if state.add_open {
                    "Close the add-key form"
                } else {
                    "Add an API key"
                })
                .font(rune::tok::TextStyle::new(10.5, 600.0))
                .color(ink("#c7d2fe"))
                .px(9.0)
                .py(5.0)
                .radius(7.0)
                .fill(ink("#6366f11f"))
                .border(1.0, ink("#6366f14d"))
                .on_press_tx_enter(move |s: &mut S, tx: &mut rune::Tx| {
                    let st = &mut s.thread_mut(which).provider_keys;
                    st.add_open = !st.add_open;
                    st.error = None;
                    if st.add_open {
                        tx.scroll_into_view("providerkeys-form-actions".to_string());
                    } else {
                        reset_form(st);
                    }
                }),
            ),
    );
    if state.keys.is_empty() {
        keys = keys.child(
            text(
                if state.loading {
                    "Loading keys…"
                } else {
                    "No API key yet. Add one to start using OpenCode Go."
                }
                .to_string(),
            )
            .key("providerkeys-empty")
            .w_full()
            .px(12.0)
            .py(12.0)
            .radius(10.0)
            .fill(ink("#ffffff03"))
            .border(1.0, ink("#ffffff0d"))
            .font(rune::tok::TextStyle::new(11.0, 400.0))
            .color(ink("#71717a")),
        );
    }
    for key in &state.keys {
        let row_id = key.id.clone();
        let row_label = key.label.clone();
        let row_masked = key.masked.clone();
        let row_source = key.source.clone();
        let row_active = key.active;
        let mut key_row = row()
            .key(format!("providerkeys-key-{}", key.id))
            .w_full()
            .items_center()
            .gap(10.0)
            .px(12.0)
            .py(10.0)
            .radius(10.0)
            .fill(ink(if row_active { "#ffffff10" } else { "#00000000" }))
            .border(1.0, ink(if row_active { "#ffffff1f" } else { "#ffffff0d" }))
            .role("button")
            .focusable(true)
            .label(format!("Use {}", row_label))
            .on_press_enter(move |s: &mut S| {
                activate_provider_key(&mut s.thread_mut(which).provider_keys, &row_id);
            })
            .child(
                rune::el()
                    .size(8.0, 8.0)
                    .radius(4.0)
                    .fill(ink(if row_active { "#6ee7b7" } else { "#3f3f46" })),
            )
            .child(
                col()
                    .grow(1.0)
                    .min_w(0.0)
                    .gap(1.0)
                    .child(
                        text(row_label)
                            .font(rune::tok::TextStyle::new(11.5, 500.0))
                            .color(ink(if row_active { "#f4f4f5" } else { "#d4d4d8" })),
                    )
                    .child(
                        text(row_masked)
                            .font(rune::tok::TextStyle::new(10.0, 400.0).mono())
                            .color(ink("#71717a")),
                    ),
            )
            .child(
                text(row_source.to_ascii_uppercase())
                    .font(rune::tok::TextStyle::new(8.5, 700.0))
                    .color(ink("#71717a")),
            );
        if row_active {
            key_row = key_row.child(
                text("ACTIVE".to_string())
                    .font(rune::tok::TextStyle::new(8.5, 700.0))
                    .px(6.0)
                    .py(2.0)
                    .radius(999.0)
                    .fill(ink("#6366f11f"))
                    .color(ink("#c7d2fe")),
            );
        }
        if row_source == "managed" {
            let remove_id = key.id.clone();
            key_row = key_row.child(
                text("×".to_string())
                    .key(format!("providerkeys-remove-{}", key.id))
                    .role("button")
                    .focusable(true)
                    .label(format!("Remove {}", key.label))
                    .font(rune::tok::TextStyle::new(14.0, 400.0))
                    .color(ink("#8a8a93"))
                    .px(6.0)
                    .py(1.0)
                    .radius(6.0)
                    .on_press_enter(move |s: &mut S| {
                        s.thread_mut(which).provider_keys.confirm_remove = Some(remove_id.clone());
                    }),
            );
        }
        keys = keys.child(key_row);
    }

    if let Some(id) = &state.confirm_remove {
        let label = state
            .keys
            .iter()
            .find(|key| &key.id == id)
            .map(|key| key.label.clone())
            .unwrap_or_else(|| "this key".to_string());
        let cancel = {
            let close = id.clone();
            move |s: &mut S| {
                let _ = close;
                s.thread_mut(which).provider_keys.confirm_remove = None;
            }
        };
        let remove = {
            let target = id.clone();
            move |s: &mut S| {
                remove_provider_key(&mut s.thread_mut(which).provider_keys, &target);
            }
        };
        keys = keys.child(
            row()
                .key("providerkeys-confirm")
                .w_full()
                .items_center()
                .gap(8.0)
                .px(12.0)
                .py(8.0)
                .radius(9.0)
                .fill(ink("#ef44441a"))
                .border(1.0, ink("#ef444440"))
                .child(
                    text(format!("Remove {label}?"))
                        .grow(1.0)
                        .font(rune::tok::TextStyle::new(10.5, 500.0))
                        .color(ink("#fca5a5")),
                )
                .child(
                    text("Cancel".to_string())
                        .role("button")
                        .focusable(true)
                        .label("Cancel removing the key")
                        .font(rune::tok::TextStyle::new(10.0, 500.0))
                        .color(ink("#a1a1aa"))
                        .px(7.0)
                        .py(4.0)
                        .radius(6.0)
                        .on_press_enter(cancel),
                )
                .child(
                    text("Remove".to_string())
                        .key("providerkeys-confirm-remove")
                        .role("button")
                        .focusable(true)
                        .label("Confirm removing the key")
                        .font(rune::tok::TextStyle::new(10.0, 600.0))
                        .color(ink("#fecaca"))
                        .px(8.0)
                        .py(4.0)
                        .radius(6.0)
                        .fill(ink("#ef444433"))
                        .on_press_enter(remove),
                ),
        );
    }

    if state.add_open {
        let mut form = col()
            .key("providerkeys-add-form")
            .w_full()
            .gap(9.0)
            .px(12.0)
            .py(12.0)
            .radius(10.0)
            .fill(ink("#ffffff05"))
            .border(1.0, ink("#ffffff12"))
            .child(field_label("Label (optional)"))
            .child(
                text_input(&state.label_edit, rune::tok::TextStyle::new(11.5, 400.0))
                    .key("providerkeys-label-input")
                    .w_full()
                    .single_line()
                    .px(10.0)
                    .py(7.0)
                    .radius(7.0)
                    .fill(ink("#08080a"))
                    .border(1.0, ink("#ffffff14"))
                    .placeholder("Personal, work, backup…", ink("#52525b"))
                    .on_edit(move |s: &mut S| {
                        &mut s.thread_mut(which).provider_keys.label_edit
                    }),
            )
            .child(field_label("API key"))
            .child(
                row()
                    .w_full()
                    .items_center()
                    .gap(6.0)
                    .child(
                        text_input(&state.key_edit, rune::tok::TextStyle::new(11.5, 400.0))
                            .key("providerkeys-key-input")
                            .grow(1.0)
                            .min_w(0.0)
                            .single_line()
                            .secure()
                            .revealed(state.reveal)
                            .px(10.0)
                            .py(7.0)
                            .radius(7.0)
                            .fill(ink("#08080a"))
                            .border(1.0, ink("#ffffff14"))
                            .placeholder("oc_…", ink("#52525b"))
                            .on_edit(move |s: &mut S| {
                                &mut s.thread_mut(which).provider_keys.key_edit
                            })
                            .on_submit(move |s: &mut S, _tx| {
                                save_provider_key(&mut s.thread_mut(which).provider_keys);
                            }),
                    )
                    .child(
                        text(if state.reveal { "Hide" } else { "Show" }.to_string())
                            .key("providerkeys-reveal")
                            .role("button")
                            .focusable(true)
                            .label(if state.reveal {
                                "Hide the API key"
                            } else {
                                "Show the API key"
                            })
                            .font(rune::tok::TextStyle::new(9.5, 500.0))
                            .color(ink("#8a8a93"))
                            .px(7.0)
                            .py(6.0)
                            .radius(6.0)
                            .on_press_enter(move |s: &mut S| {
                                let st = &mut s.thread_mut(which).provider_keys;
                                st.reveal = !st.reveal;
                            }),
                    ),
            )
            .child(
                row()
                    .key("providerkeys-form-actions")
                    .w_full()
                    .items_center()
                    .gap(8.0)
                    .child(
                        text(
                            "Checked with OpenCode before saving. Only a masked value is stored for display."
                                .to_string(),
                        )
                        .grow(1.0)
                        .font(rune::tok::TextStyle::new(9.0, 400.0))
                        .color(ink("#71717a")),
                    )
                    .child(
                        text(if state.busy { "Checking…" } else { "Add & use" }.to_string())
                            .key("providerkeys-save")
                            .role("button")
                            .focusable(true)
                            .label("Save this API key and make it active")
                            .font(rune::tok::TextStyle::new(10.5, 600.0))
                            .color(ink("#ffffff"))
                            .px(10.0)
                            .py(5.0)
                            .radius(7.0)
                            .fill(ink("#6366f1"))
                            .on_press_enter(move |s: &mut S| {
                                save_provider_key(&mut s.thread_mut(which).provider_keys);
                            }),
                    ),
            );
        if state.busy {
            form = form.child(
                text("Checking the key with the provider…".to_string())
                    .font(rune::tok::TextStyle::new(9.5, 400.0))
                    .color(ink("#a1a1aa")),
            );
        }
        keys = keys.child(form);
    }
    body = body.child(keys);

    for issue in &state.issues {
        body = body.child(
            text(issue.clone())
                .w_full()
                .wrap()
                .font(rune::tok::TextStyle::new(9.5, 400.0).mono())
                .color(ink("#fbbf24")),
        );
    }
    if let Some(error) = &state.error {
        body = body.child(
            text(error.clone())
                .key("providerkeys-error")
                .w_full()
                .wrap()
                .px(9.0)
                .py(9.0)
                .radius(8.0)
                .fill(ink("#ef44441a"))
                .border(1.0, ink("#ef444440"))
                .font(rune::tok::TextStyle::new(10.0, 400.0))
                .color(ink("#fca5a5")),
        );
    } else if !state.status.is_empty() && !state.add_open {
        body = body.child(
            text(state.status.clone())
                .key("providerkeys-status")
                .w_full()
                .wrap()
                .px(9.0)
                .py(9.0)
                .radius(8.0)
                .fill(ink("#ffffff05"))
                .border(1.0, ink("#ffffff10"))
                .font(rune::tok::TextStyle::new(10.0, 400.0))
                .color(ink("#a1a1aa")),
        );
    }

    let footer = row()
        .key("providerkeys-foot")
        .w_full()
        .items_center()
        .gap(8.0)
        .px(20.0)
        .pt(6.0)
        .pb(14.0)
        .child(
            text("Agent settings".to_string())
                .role("button")
                .focusable(true)
                .label("Open agent settings")
                .font(rune::tok::TextStyle::new(10.0, 400.0))
                .color(ink("#71717a"))
                .on_press_enter(move |s: &mut S| {
                    let (host, port) = {
                        let st = &mut s.thread_mut(which).provider_keys;
                        st.open = false;
                        reset_form(st);
                        (st.host.clone(), st.port)
                    };
                    let agent = &mut s.thread_mut(which).agent_config;
                    agent.host = host;
                    agent.port = port;
                    crate::agent_config::open_agent_config(agent);
                }),
        )
        .child(rune::el().grow(1.0))
        .child(
            text(if state.busy { "Saving…" } else { "Done" }.to_string())
                .key("providerkeys-done")
                .role("button")
                .focusable(true)
                .label("Close provider settings")
                .font(rune::tok::TextStyle::new(10.5, 600.0))
                .color(ink("#e4e4e7"))
                .px(10.0)
                .py(5.0)
                .radius(7.0)
                .fill(ink("#ffffff0d"))
                .border(1.0, ink("#ffffff14"))
                .on_press_enter(close),
        );

    let mut card = col()
        .key("providerkeys-card")
        .transition(crate::motion::modal_enter())
        .w(card_w)
        .max_h(card_h)
        .radius(16.0)
        .fill(ink("#09090b"))
        .border(1.0, ink("#ffffff14"))
        .border_layout()
        .clip_rounded(16.0)
        .box_shadow(rune::Shadow {
            x: 0.0,
            y: 25.0,
            blur: 50.0,
            spread: -12.0,
            color: rune::Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.5,
            },
            inset: false,
        })
        .modal(true, close)
        .child(
            row()
                .key("providerkeys-head")
                .w_full()
                .items_center()
                .gap(10.0)
                .px(20.0)
                .pt(16.0)
                .pb(12.0)
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .child(
                            text("Provider".to_string())
                                .key("providerkeys-head-title")
                                .font(rune::tok::TextStyle::new(14.0, 600.0))
                                .color(ink("#f4f4f5")),
                        )
                        .child(
                            text("OpenCode Go keys and live allowance".to_string())
                                .font(rune::tok::TextStyle::new(10.0, 400.0))
                                .color(ink("#71717a")),
                        ),
                )
                .child(
                    rune::el()
                        .key("providerkeys-close")
                        .size(16.0, 16.0)
                        .role("button")
                        .focusable(true)
                        .label("Close provider settings")
                        .shape(crate::lucide_x(16.0, 2.0, ink("#8a8a93")))
                        .on_press_enter(close),
                ),
        )
        .child(body)
        .child(footer);
    if card_h > 0.0 {
        card = card.h(card_h);
    }
    rune::el()
        .key("providerkeys-overlay")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .center()
        .modal(true, move |s: &mut S| {
            close_provider_keys(&mut s.thread_mut(which).provider_keys);
        })
        .child(
            rune::el()
                .key("providerkeys-backdrop")
                .absolute(0.0, 0.0)
                .w_full()
                .h_full()
                .fill(rune::Color {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.7,
                })
                .backdrop_blur(12.0)
                .transition(crate::motion::backdrop_enter()),
        )
        .child(card)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_key_payload_and_usage() {
        let keys = serde_json::json!({
            "provider": "opencode",
            "label": "OpenCode Go",
            "base_url": "https://opencode.ai/zen/go/v1",
            "model": "deepseek-v4.1-flash",
            "active": "env",
            "has_key": true,
            "keys": [
                {"id":"env","label":"Environment (.env)","masked":"oc_…a1b2","source":"env","active":true},
                {"id":"key-1","label":"Backup","masked":"oc_…c3d4","source":"managed","active":false}
            ],
            "issues": [],
            "message": "ok"
        });
        let payload = ProviderKeysPayload::from_json(&keys);
        assert_eq!(payload.keys.len(), 2);
        assert_eq!(payload.keys[0].source, "env");
        assert_eq!(payload.keys[1].label, "Backup");

        let usage = serde_json::json!({
            "usage": {
                "rolling": {"status":"ok","percent":42.4,"resetsAt":"2099-01-01T00:00:00Z"},
                "weekly": {"status":"ok","percent":10,"resetsAt":"2099-01-01T00:00:00Z"},
                "monthly": {"status":"rate-limited","percent":100,"resetsAt":"2099-01-01T00:00:00Z"}
            },
            "error": null
        });
        let usage = ProviderUsage::from_json(&usage);
        assert_eq!(usage.rolling.as_ref().unwrap().percent, 42.4);
        assert_eq!(usage.monthly.as_ref().unwrap().status, "rate-limited");
        assert!(usage.error.is_empty());
    }

    #[test]
    fn reset_label_is_relative() {
        let window = UsageWindow {
            status: "ok".to_string(),
            percent: 1.0,
            resets_at: "2099-01-01T00:00:00Z".to_string(),
        };
        assert!(reset_label(&window).starts_with("resets in "));
    }

    #[test]
    fn provider_pill_opens_the_sheet() {
        use rune::{Env, Runtime};
        let mut init = crate::rune_transcript::RuneTranscriptState::screenshot();
        init.live = false;
        let mut rt = Runtime::new(
            init,
            crate::rune_transcript::transcript_view,
            (900.0, 700.0),
            Env::new(true, 1.0),
        );
        rt.frame(0.0);
        let (x, y) = rt.find("composer-provider").expect("provider pill");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..6 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.state.provider_keys.open, "pill opens the provider sheet");
        assert!(rt.find("providerkeys-overlay").is_some());
    }

    #[test]
    fn sheet_mounts_keys_usage_and_form() {
        use rune::{Env, Runtime};
        let mut init = crate::rune_transcript::RuneTranscriptState::screenshot();
        init.live = false;
        init.provider_keys.open = true;
        init.provider_keys.provider = "opencode".to_string();
        init.provider_keys.label = "OpenCode Go".to_string();
        init.provider_keys.model = "deepseek-v4.1-flash".to_string();
        init.provider_keys.active = "key-1".to_string();
        init.provider_keys.has_key = true;
        init.provider_keys.keys = vec![
            ProviderKeyRow {
                id: "env".to_string(),
                label: "Environment (.env)".to_string(),
                masked: "oc_…a1b2".to_string(),
                source: "env".to_string(),
                active: false,
            },
            ProviderKeyRow {
                id: "key-1".to_string(),
                label: "Backup".to_string(),
                masked: "oc_…c3d4".to_string(),
                source: "managed".to_string(),
                active: true,
            },
        ];
        init.provider_keys.usage = ProviderUsage {
            rolling: Some(UsageWindow {
                status: "ok".to_string(),
                percent: 12.0,
                resets_at: "2099-01-01T00:00:00Z".to_string(),
            }),
            ..ProviderUsage::default()
        };
        let mut rt = Runtime::new(
            init,
            crate::rune_transcript::transcript_view,
            (900.0, 700.0),
            Env::new(true, 1.0),
        );
        rt.frame(0.0);
        for key in [
            "providerkeys-overlay",
            "providerkeys-card",
            "providerkeys-title",
            "providerkeys-usage",
            "providerkeys-key-env",
            "providerkeys-key-key-1",
            "providerkeys-add-toggle",
            "providerkeys-close",
            "providerkeys-done",
        ] {
            assert!(rt.find(key).is_some(), "{key} mounted");
        }
        let (x, y) = rt.find("providerkeys-add-toggle").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.find("providerkeys-add-form").is_some());
    }
}
