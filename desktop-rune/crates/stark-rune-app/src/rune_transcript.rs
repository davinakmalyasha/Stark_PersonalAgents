//! Real Rune transcript view — MessageStream port with virtualized rows.
//!
//! Only compiled with `--features rune-real` (needs the pinned `rune` checkout
//! + GPU at snap time; unit tests below the `cfg` stay GPU-free).
//!
//! Rune mapping, 1:1 with `crates/rune/README.md`:
//! - scroll viewport: `.scroll(ScrollSpec { stick_bottom: true })` on a
//!   height-bounded column (`.h_full().min_h(0.0)` — flex `.grow` collapses
//!   unbounded viewports to 0pt).
//! - growth stack: `.growth(GrowthSpec::default())` + lazy
//!   `.virtual_rows(count, EST, key, build)` as the viewport's first child;
//!   rows keyed by message id with internal padding (never stack gaps/margins).
//!   Eager `.virtualize` + prebuilt children is a perf bug Rune warns on;
//!   handlers find items by key, never by index.
//! - headless PNG: `stark-rune --rune-snap out.png [--rune-scene name]`
//!   runs `rune::App` headless (`--snap`); tree via `--tree`.

use std::borrow::Cow;

use rune::{col, row, text, AppSpec, Cx, El, GrowthSpec, ScrollSpec, TextEdit};
use stark_kit::transcript::{TranscriptRow, ESTIMATED_ROW_H};

#[path = "rune_transcript/md_render.rs"]
mod md_render;

/// Row content fed to both the pure window math and the Rune view.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RowData {
    pub key: String,
    pub role: &'static str,
    pub body: String,
    /// ISO-8601 creation stamp (`CanvasChatBubble clock()` renders it as
    /// `hh:MM AM/PM` in the bubble foot). Empty for scene fixtures.
    pub at: String,
    /// `metadata.duration` for the "Worked for …" trace divider.
    pub duration: String,
    pub show_trace: bool,
    pub hide_body: bool,
    /// `metadata.kind` for `SessionEventDivider`: `"aborted"` or `"compaction"`.
    pub event: String,
}

/// Accumulated usage for one thread (`combinedTokenUsage` in `useChat.ts`):
/// totals grow from `delta kind:"usage"` frames (`input/output`,
/// `cached_tokens?`, `cost?`); `active` is the last call's input and drives
/// the context orb (`activeContextTokens = lastCall?.input || 0`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UsageTotals {
    pub input: u64,
    pub output: u64,
    pub cache_hit: u64,
    pub cost: f64,
    pub active: u64,
    /// Per-call rows (`TokenCall`) for the stats modal. `agent` is always
    /// `"chat"` — usage deltas carry no agent name (React merges server-side
    /// rows that do); model/timestamp stamp at record time.
    pub calls: Vec<UsageCall>,
}

/// One request row (`TokenCall`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UsageCall {
    pub agent: String,
    pub model: String,
    pub input: u64,
    pub output: u64,
    pub hits: u64,
    pub misses: u64,
    pub cost: f64,
    pub at: String,
}

impl UsageTotals {
    /// Context percent of 1M (`contextPercent`).
    pub fn context_percent(&self) -> f64 {
        ((self.active as f64 / 1_000_000.0) * 100.0).min(100.0)
    }
}

/// Live provider label (`fetchProvider` → `ProviderInfo`): `OpenCode Go`
/// when the gateway is opencode, else `provider_label || provider`. `warn`
/// mirrors the trigger's `is-warning` (no key or a provider health error).
/// The agent-config modal it opens is unported, so the pill is a display.
#[derive(Debug)]
pub struct ProviderState {
    pub label: String,
    pub warn: bool,
    pub loading: bool,
    pub last_ok: f64,
    /// Start of the last attempt (success or failure) — failure must not
    /// retry every frame.
    pub last_try: f64,
    pub inbox: Option<std::sync::mpsc::Receiver<Result<ProviderState, String>>>,
}

impl Clone for ProviderState {
    fn clone(&self) -> Self {
        Self {
            label: self.label.clone(),
            warn: self.warn,
            loading: self.loading,
            last_ok: self.last_ok,
            last_try: self.last_try,
            inbox: None,
        }
    }
}

impl Default for ProviderState {
    fn default() -> Self {
        Self {
            label: "Provider".to_string(),
            warn: false,
            loading: false,
            last_ok: 0.0,
            last_try: f64::NEG_INFINITY,
            inbox: None,
        }
    }
}

impl ProviderState {
    pub fn from_json(v: &serde_json::Value) -> Self {
        let str_of = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let provider = str_of("provider");
        let label = if provider == "opencode" {
            "OpenCode Go".to_string()
        } else {
            let named = str_of("provider_label");
            if named.is_empty() {
                if provider.is_empty() {
                    "Provider".to_string()
                } else {
                    provider.clone()
                }
            } else {
                named
            }
        };
        let health = v
            .get("health")
            .and_then(|x| x.as_object())
            .and_then(|o| o.get(&provider))
            .and_then(|x| x.as_str())
            .unwrap_or("");
        Self {
            label,
            warn: !v.get("has_key").and_then(|x| x.as_bool()).unwrap_or(false)
                || !health.is_empty(),
            loading: false,
            last_ok: 0.0,
            last_try: 0.0,
            inbox: None,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn fetch(host: &str, port: u16) -> Result<Self, String> {
        let v = crate::transport::get_json(host, port, "/api/provider")?;
        Ok(Self::from_json(&v))
    }
}

/// One model entry (`GET /api/models` → `{models:[{id,name,provider}]}`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModelOption {
    pub id: String,
    pub name: String,
}

impl ModelOption {
    pub fn from_json(v: &serde_json::Value) -> Self {
        Self {
            id: v
                .get("id")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            name: v
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
        }
    }
}

/// Fetch the model list once per thread (60s retry on failure). Returns
/// whether a result landed (so the shell commits the new dropup).
pub fn poll_models_state(s: &mut RuneTranscriptState, clock: f64) -> bool {
    let mut changed = false;
    let (msgs, _) = crate::rooms::drain_results(&mut s.models_inbox);
    for msg in msgs {
        if let Ok(models) = msg {
            if s.models != models {
                s.models = models;
                changed = true;
            }
            s.models_ok = true;
        }
        s.models_loading = false;
        s.models_last_try = clock;
    }
    if s.services_auto && !s.models_loading && !s.models_ok && clock - s.models_last_try > 60.0 {
        s.models_loading = true;
        s.models_last_try = clock;
        let (tx, rx) = std::sync::mpsc::channel();
        s.models_inbox = Some(rx);
        let (host, port) = (s.api_host.clone(), s.api_port);
        #[cfg(target_arch = "wasm32")]
        wasm_bindgen_futures::spawn_local(async move {
            let result = match crate::transport::fetch_json("/api/models").await {
                Ok(v) => Ok(v
                    .get("models")
                    .and_then(|x| x.as_array())
                    .map(|a| a.iter().map(ModelOption::from_json).collect())
                    .unwrap_or_default()),
                Err(e) => Err(e),
            };
            let _ = tx.send(result);
            let _ = (host, port);
        });
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::spawn(move || {
            let result = match crate::transport::get_json(&host, port, "/api/models") {
                Ok(v) => Ok(v
                    .get("models")
                    .and_then(|x| x.as_array())
                    .map(|a| a.iter().map(ModelOption::from_json).collect())
                    .unwrap_or_default()),
                Err(e) => Err(e),
            };
            let _ = tx.send(result);
        });
    }
    changed
}

/// Refresh the provider label at most every 30s (`PromptDock` interval).
/// Failures keep the last label (the effect swallows errors).
pub fn poll_provider_state(s: &mut RuneTranscriptState, clock: f64) -> bool {
    let mut changed = false;
    let (msgs, _) = crate::rooms::drain_results(&mut s.provider.inbox);
    for msg in msgs {
        if let Ok(next) = msg {
            if s.provider.label != next.label || s.provider.warn != next.warn {
                changed = true;
            }
            s.provider.label = next.label;
            s.provider.warn = next.warn;
            s.provider.last_ok = clock;
        }
        s.provider.loading = false;
        s.provider.last_try = clock;
    }
    if s.services_auto && !s.provider.loading && clock - s.provider.last_try > 30.0 {
        s.provider.loading = true;
        s.provider.last_try = clock;
        let (tx, rx) = std::sync::mpsc::channel();
        s.provider.inbox = Some(rx);
        let (host, port) = (s.api_host.clone(), s.api_port);
        #[cfg(target_arch = "wasm32")]
        wasm_bindgen_futures::spawn_local(async move {
            let result = match crate::transport::fetch_json("/api/provider").await {
                Ok(v) => Ok(ProviderState::from_json(&v)),
                Err(e) => Err(e),
            };
            let _ = tx.send(result);
            let _ = (host, port);
        });
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::spawn(move || {
            let _ = tx.send(ProviderState::fetch(&host, port));
        });
    }
    changed
}
/// Now as ISO-8601 UTC (`YYYY-MM-DDTHH:MM:SSZ`), matching the server wire
/// shape the other time helpers parse.
pub fn now_iso() -> String {
    let secs = (crate::notes::now_millis() / 1000) as i64;
    // Days → civil date (Howard Hinnant's algorithm, inverted).
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        if m <= 2 { y + 1 } else { y },
        m,
        d,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// `02:44 PM` (`CanvasChatBubble clock()`): the browser's local 12h clock.
/// Empty in, empty out.
pub fn clock(iso: &str) -> String {
    let Some((_, _, _, h, mi)) = local_parts(iso) else {
        return String::new();
    };
    let (h12, ap) = match h {
        0 => (12, "AM"),
        1..=11 => (h, "AM"),
        12 => (12, "PM"),
        _ => (h - 12, "PM"),
    };
    format!("{h12:02}:{mi:02} {ap}")
}

/// Local calendar day key (`YYYY-MM-DD`) — the transcript's divider grouping
/// (`MessageStream.dayKey`, `Date.toDateString` in local time).
pub fn day_key(iso: &str) -> String {
    local_parts(iso)
        .map(|(y, m, d, _, _)| format!("{y:04}-{m:02}-{d:02}"))
        .unwrap_or_default()
}

/// Date divider label (`MessageStream.dayLabel`): `Today`, `Yesterday`, or
/// `Mon D` (adds the year only when it differs from today's).
pub fn day_label(iso: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let Some((y, m, d, _, _)) = local_parts(iso) else {
        return String::new();
    };
    let today = local_parts_from_secs((crate::notes::now_millis() / 1000) as i64);
    if let Some((ty, tm, td, _, _)) = today {
        if (y, m, d) == (ty, tm, td) {
            return "Today".to_string();
        }
        let yesterday = days_from_civil(ty as i64, tm as i64, td as i64) - 1;
        if days_from_civil(y as i64, m as i64, d as i64) == yesterday {
            return "Yesterday".to_string();
        }
        let month = MONTHS
            .get(m.saturating_sub(1) as usize)
            .copied()
            .unwrap_or("");
        return if y == ty {
            format!("{month} {d}")
        } else {
            format!("{month} {d}, {y}")
        };
    }
    let month = MONTHS
        .get(m.saturating_sub(1) as usize)
        .copied()
        .unwrap_or("");
    format!("{month} {d}")
}

/// Parse the server's ISO stamp to UTC epoch seconds.
/// Calendar fields from a server timestamp plus whether it carried a zone.
/// Server chat stamps come in both shapes: `...Z` (UTC) and a naive
/// `2026-10-25T11:37:32.113796` that `new Date()` reads as local time.
fn iso_fields(iso: &str) -> Option<(i64, i64, i64, i64, i64, i64, bool)> {
    let digits: String = iso.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() < 12 {
        return None;
    }
    let part = |from: usize, len: usize| digits.get(from..from + len)?.parse::<i64>().ok();
    let (y, mo, d) = (part(0, 4)?, part(4, 2)?, part(6, 2)?);
    let (h, mi) = (part(8, 2)?, part(10, 2)?);
    let s = if digits.len() >= 14 { part(12, 2)? } else { 0 };
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    let has_zone = iso.trim_end().ends_with('Z')
        || iso
            .chars()
            .skip(10)
            .any(|c| c == '+' || (c == '-' && iso.chars().filter(|x| *x == '-').count() > 2));
    Some((y, mo, d, h, mi, s, !has_zone))
}

/// Parse a server stamp to UTC epoch seconds. Naive stamps are read as the
/// browser reads them: local wall time.
pub(crate) fn iso_epoch_secs(iso: &str) -> Option<i64> {
    let (y, mo, d, h, mi, s, naive) = iso_fields(iso)?;
    if naive {
        return naive_local_epoch(y, mo, d, h, mi, s);
    }
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s)
}

/// Local wall-clock fields → UTC epoch. `None`/fallback stays UTC when the
/// platform has no zone API.
#[cfg(target_arch = "wasm32")]
fn naive_local_epoch(y: i64, mo: i64, d: i64, h: i64, mi: i64, s: i64) -> Option<i64> {
    let date = js_sys::Date::new_with_year_month_day_hr_min_sec(
        y as u32,
        (mo - 1) as i32,
        d as i32,
        h as i32,
        mi as i32,
        s as i32,
    );
    let ms = date.get_time();
    if ms.is_nan() {
        return None;
    }
    Some((ms / 1000.0) as i64)
}

#[cfg(all(not(target_arch = "wasm32"), windows))]
fn naive_local_epoch(y: i64, mo: i64, d: i64, h: i64, mi: i64, s: i64) -> Option<i64> {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn TzSpecificLocalTimeToSystemTime(
            tz: *const core::ffi::c_void,
            local: *const SystemTime,
            utc: *mut SystemTime,
        ) -> i32;
        fn SystemTimeToFileTime(st: *const SystemTime, ft: *mut u64) -> i32;
    }
    const EPOCH_AS_FILETIME: i64 = 11_644_473_600;
    let local = SystemTime {
        year: y as u16,
        month: mo as u16,
        day: d as u16,
        hour: h as u16,
        minute: mi as u16,
        second: s as u16,
        ..SystemTime::default()
    };
    let mut utc = SystemTime::default();
    // SAFETY: plain Win32 structs with valid pointers for the call duration.
    if unsafe { TzSpecificLocalTimeToSystemTime(core::ptr::null(), &local, &mut utc) } == 0 {
        return None;
    }
    let mut ft = 0u64;
    if unsafe { SystemTimeToFileTime(&utc, &mut ft) } == 0 {
        return None;
    }
    Some((ft / 10_000_000) as i64 - EPOCH_AS_FILETIME)
}

#[cfg(all(not(target_arch = "wasm32"), not(windows)))]
fn naive_local_epoch(y: i64, mo: i64, d: i64, h: i64, mi: i64, s: i64) -> Option<i64> {
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s)
}

/// Days since 1970-01-01 (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

#[cfg(all(not(target_arch = "wasm32"), not(windows)))]
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    ((y + i64::from(m <= 2)) as i32, m as u32, d as u32)
}

/// Local calendar fields for a UTC epoch (year, month, day, hour, minute).
#[cfg(target_arch = "wasm32")]
fn local_parts_from_secs(secs: i64) -> Option<(i32, u32, u32, u32, u32)> {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64((secs * 1000) as f64));
    Some((
        date.get_full_year() as i32,
        date.get_month() + 1,
        date.get_date(),
        date.get_hours(),
        date.get_minutes(),
    ))
}

/// Windows local time (`SystemTimeToTzSpecificLocalTime`), DST-correct.
#[cfg(all(not(target_arch = "wasm32"), windows))]
fn local_parts_from_secs(secs: i64) -> Option<(i32, u32, u32, u32, u32)> {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn FileTimeToSystemTime(ft: *const u64, st: *mut SystemTime) -> i32;
        fn SystemTimeToTzSpecificLocalTime(
            tz: *const core::ffi::c_void,
            utc: *const SystemTime,
            local: *mut SystemTime,
        ) -> i32;
    }
    const EPOCH_AS_FILETIME: i64 = 11_644_473_600;
    let ft = ((secs + EPOCH_AS_FILETIME) as u128 * 10_000_000) as u64;
    let mut utc = SystemTime::default();
    let mut local = SystemTime::default();
    // SAFETY: plain Win32 structs with valid pointers for the call duration.
    let ok = unsafe { FileTimeToSystemTime(&ft, &mut utc) };
    if ok == 0 {
        return None;
    }
    let ok = unsafe { SystemTimeToTzSpecificLocalTime(core::ptr::null(), &utc, &mut local) };
    if ok == 0 {
        return None;
    }
    Some((
        local.year as i32,
        local.month as u32,
        local.day as u32,
        local.hour as u32,
        local.minute as u32,
    ))
}

/// Non-Windows native fallback: UTC (the browser and Windows builds carry
/// the real local zone).
#[cfg(all(not(target_arch = "wasm32"), not(windows)))]
fn local_parts_from_secs(secs: i64) -> Option<(i32, u32, u32, u32, u32)> {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    Some((y, m, d, (rem / 3600) as u32, ((rem % 3600) / 60) as u32))
}

fn local_parts(iso: &str) -> Option<(i32, u32, u32, u32, u32)> {
    let (y, mo, d, h, mi, _, naive) = iso_fields(iso)?;
    if naive {
        return Some((y as i32, mo as u32, d as u32, h as u32, mi as u32));
    }
    local_parts_from_secs(iso_epoch_secs(iso)?)
}

pub struct RuneTranscriptState {
    pub title: String,
    pub rows: Vec<RowData>,
    pub edit: TextEdit,
    pub model: String,
    pub live: bool,
    pub api_host: String,
    pub api_port: u16,
    /// Chat id sent with `POST /api/run` and used by `POST /api/abort` —
    /// mirrors the React `chat.id` both `streamPipeline` and `abortRun`
    /// carry. Empty until the first send stages one.
    pub chat_id: String,
    /// Sticky notes for this thread (chat scope, like `ChatView.tsx`).
    pub notes: crate::notes::NotesState,
    /// Run target (`PromptDock runTarget`, persisted per chat — here per
    /// thread, the session's lifetime). `"local"` default; `"cloud"` only
    /// changes where the NEXT prompt is marked to go — the transfer itself
    /// lands next, same as React, so sends behave identically either way.
    pub run_target: String,
    /// Usage totals for the orb + popover (`combinedTokenUsage`).
    pub usage: UsageTotals,
    /// Usage popover open (tap the orb; React hovers — tap is the honest
    /// native equivalent, no hover targeting headlessly).
    pub usage_open: bool,
    /// Usage stats modal open (`TokenStatsModal`, per thread).
    pub stats_open: bool,
    /// Agents & providers modal state (per thread — the pill lives in every
    /// dock, so every thread owns its modal).
    pub agent_config: crate::agent_config::AgentConfigState,
    /// Model options (`GET /api/models` → `{models:[{id,name}]}`), fetched
    /// once per thread lifetime (React fetches on mount; 60s retry keeps a
    /// cold start honest).
    pub models: Vec<ModelOption>,
    pub models_loading: bool,
    pub models_ok: bool,
    pub models_last_try: f64,
    /// Model dropup open (`provider-menu`).
    pub model_open: bool,
    pub models_inbox: Option<std::sync::mpsc::Receiver<Result<Vec<ModelOption>, String>>>,
    /// Live provider pill (`fetchProvider`, 30s refresh).
    pub provider: ProviderState,
    /// Background status fetches (`/api/provider`, `/api/models`) run only in
    /// the real app entry; unit tests and headless scenes stay offline.
    pub services_auto: bool,
    /// Settings sheet visibility.
    pub settings_open: bool,
    /// Mic capture bus (levels, notes, stop flag, inbox).
    pub voice: crate::voice::VoiceBus,
    /// Frames streamed from the run thread, drained by the `poll` hook.
    /// `None` on clone: a copied state never inherits a live socket.
    pub inbox: Option<std::sync::mpsc::Receiver<crate::stream::SseFrame>>,
    /// The key of the code block whose copy button shows the Check state.
    pub copied_code: Option<String>,
    /// When that copy happened; cleared after React's 1.8s.
    pub copied_at: Option<std::time::Instant>,
    /// Diff blocks whose "Show all N lines" button has been pressed.
    pub expanded_diffs: std::collections::HashSet<String>,
}

impl std::fmt::Debug for RuneTranscriptState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuneTranscriptState")
            .field("title", &self.title)
            .field("rows", &self.rows)
            .field("edit", &self.edit.text)
            .field("model", &self.model)
            .field("live", &self.live)
            .field("api_host", &self.api_host)
            .field("api_port", &self.api_port)
            .field("chat_id", &self.chat_id)
            .field("notes", &self.notes.notes.len())
            .field("run_target", &self.run_target)
            .field("usage", &self.usage)
            .field("models", &self.models.len())
            .field("settings_open", &self.settings_open)
            .field("voice", &self.voice)
            .field("inbox", &self.inbox.is_some())
            .field("copied_code", &self.copied_code)
            .field("expanded_diffs", &self.expanded_diffs.len())
            .finish()
    }
}

impl Clone for RuneTranscriptState {
    fn clone(&self) -> Self {
        Self {
            title: self.title.clone(),
            rows: self.rows.clone(),
            edit: self.edit.clone(),
            model: self.model.clone(),
            live: self.live,
            api_host: self.api_host.clone(),
            api_port: self.api_port,
            chat_id: self.chat_id.clone(),
            notes: self.notes.clone(),
            run_target: self.run_target.clone(),
            usage: self.usage.clone(),
            usage_open: self.usage_open,
            stats_open: self.stats_open,
            agent_config: self.agent_config.clone(),
            models: self.models.clone(),
            models_loading: self.models_loading,
            models_ok: self.models_ok,
            models_last_try: self.models_last_try,
            model_open: self.model_open,
            models_inbox: None,
            provider: ProviderState {
                label: self.provider.label.clone(),
                warn: self.provider.warn,
                loading: self.provider.loading,
                last_ok: self.provider.last_ok,
                last_try: self.provider.last_try,
                inbox: None,
            },
            services_auto: self.services_auto,
            settings_open: self.settings_open,
            voice: self.voice.clone(),
            inbox: None,
            copied_code: self.copied_code.clone(),
            copied_at: self.copied_at,
            expanded_diffs: self.expanded_diffs.clone(),
        }
    }
}

pub const DEFAULT_API_HOST: &str = "127.0.0.1";
pub const DEFAULT_API_PORT: u16 = 8124;

/// The port every client uses. `STARK_API_PORT` lets the parity rig point
/// both frontends at a frozen fixture server without stopping the live one.
pub fn api_port() -> u16 {
    std::env::var("STARK_API_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_API_PORT)
}

impl RuneTranscriptState {
    /// Screenshot scene: mirrors `AppState::default` card-4 (the running chat).
    pub fn screenshot() -> Self {
        Self {
            title: "New Chat".to_string(),
            rows: vec![
                RowData {
                    key: "msg-0".to_string(),
                    role: "user",
                    body: "can you use a music api or smth, makes green green real, playing now in your tab, something like that, and play that in current youtube".to_string(),
                    at: "2026-11-09T14:44:00Z".to_string(),
                    ..RowData::default()
                },
                RowData {
                    key: "msg-1".to_string(),
                    role: "agent",
                    body: "Playing now in your YouTube tab: **massive green day** — `Cinnamon Girl` (8yr-old).\n\n```text\ntrack is running (04:48)\n```".to_string(),
                    at: "2026-11-09T14:45:00Z".to_string(),
                    ..RowData::default()
                },
            ],
            edit: TextEdit::new(""),
            model: "Automatic Fallback".to_string(),
            live: true,
            api_host: DEFAULT_API_HOST.to_string(),
            api_port: api_port(),
            chat_id: crate::notes::mint_id("rune"),
            notes: crate::notes::NotesState::default(),
            run_target: "local".to_string(),
            usage: UsageTotals::default(),
            usage_open: false,
            stats_open: false,
            agent_config: crate::agent_config::AgentConfigState::default(),
            models: Vec::new(),
            models_loading: false,
            models_ok: false,
            models_last_try: f64::NEG_INFINITY,
            model_open: false,
            models_inbox: None,
            provider: ProviderState::default(),
            services_auto: false,
            settings_open: false,
            voice: crate::voice::VoiceBus::default(),
            inbox: None,
            copied_code: None,
            copied_at: None,
            expanded_diffs: std::collections::HashSet::new(),
        }
    }

    /// Stress scene: 10k rows for the virtualization parity check.
    pub fn stress() -> Self {
        let mut rows = Vec::with_capacity(10_000);
        for i in 0..10_000 {
            let (role, body) = if i % 3 == 0 {
                (
                    "user",
                    format!("prompt {i}: please summarize the workspace state"),
                )
            } else {
                (
                    "agent",
                    format!("answer {i}: applied 3 edits across 2 files, tests passing"),
                )
            };
            rows.push(RowData {
                key: format!("msg-{i}"),
                role,
                body,
                at: String::new(),
                ..RowData::default()
            });
        }
        Self {
            title: "Stress — 10k rows".to_string(),
            rows,
            edit: TextEdit::new(""),
            model: "Automatic Fallback".to_string(),
            live: false,
            api_host: DEFAULT_API_HOST.to_string(),
            api_port: api_port(),
            chat_id: crate::notes::mint_id("rune"),
            notes: crate::notes::NotesState::default(),
            run_target: "local".to_string(),
            usage: UsageTotals::default(),
            usage_open: false,
            stats_open: false,
            agent_config: crate::agent_config::AgentConfigState::default(),
            models: Vec::new(),
            models_loading: false,
            models_ok: false,
            models_last_try: f64::NEG_INFINITY,
            model_open: false,
            models_inbox: None,
            provider: ProviderState::default(),
            services_auto: false,
            settings_open: false,
            voice: crate::voice::VoiceBus::default(),
            inbox: None,
            copied_code: None,
            copied_at: None,
            expanded_diffs: std::collections::HashSet::new(),
        }
    }

    pub fn transcript_rows(&self) -> Vec<TranscriptRow> {
        use stark_kit::Role;
        self.rows
            .iter()
            .map(|r| {
                let role = match r.role {
                    "user" => Role::User,
                    "error" => Role::Error,
                    "system" => Role::System,
                    _ => Role::Agent,
                };
                TranscriptRow::new(&r.key, role, r.body.len())
            })
            .collect()
    }
}

fn bg() -> rune::Color {
    rune::color::hex("#0a0a0a").unwrap_or(rune::Color::BLACK)
}

/// Markdown block rendering lives in `md_render` (a direct `Markdown.tsx` port).
fn render_body<S: TranscriptHost + 'static>(
    s: &S,
    which: Thread,
    key: &str,
    body: &str,
    body_ink: &'static str,
) -> El {
    md_render::render_body(s, which, key, body, body_ink)
}

/// Markdown at a caller-chosen type scale (home rows are 15.5pt, the sheet
/// keeps 12.5pt).
fn render_body_styled<S: TranscriptHost + 'static>(
    s: &S,
    which: Thread,
    key: &str,
    body: &str,
    body_ink: &'static str,
    body_style: rune::tok::TextStyle,
) -> El {
    md_render::render_body_styled(s, which, key, body, body_ink, body_style)
}

/// Shared send path (button + Enter): stage the field as a user row, mark the
/// run live, and spawn the blocking `POST /api/run` thread into `inbox`.
pub fn stage_send(s: &mut RuneTranscriptState) {
    let prompt = s.edit.text.trim().to_string();
    if prompt.is_empty() {
        return;
    }
    s.edit.text.clear();
    s.edit.anchor = 0;
    s.edit.focus = 0;
    stage_send_prompt(s, prompt);
}

/// Stage an explicit prompt (map dock sends land here with the dock's text).
pub fn stage_send_prompt(s: &mut RuneTranscriptState, prompt: String) {
    let prompt = prompt.trim().to_string();
    if prompt.is_empty() {
        return;
    }
    let n = s.rows.len();
    s.rows.push(RowData {
        key: format!("user-{n}"),
        role: "user",
        body: prompt.clone(),
        at: now_iso(),
        ..RowData::default()
    });
    s.live = true;
    // The abort path needs the same id the run started with: mint one per
    // thread on first send (React reuses the persistent `chat.id`).
    if s.chat_id.trim().is_empty() {
        s.chat_id = crate::notes::mint_id("rune");
    }
    let req = crate::stream::RunRequest {
        prompt,
        model: s.model.clone(),
        temperature: 0.7,
        workspace_path: String::new(),
        chat_id: s.chat_id.clone(),
        workspace_kind: "general".to_string(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    s.inbox = Some(rx);
    let (host, port) = (s.api_host.clone(), s.api_port);
    // Browsers have no TCP sockets: drive the run over fetch + ReadableStream
    // into the same inbox the poll hook drains. Same rows, same framing.
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let mut emit = |frame: crate::stream::SseFrame| {
            let _ = tx.send(frame);
        };
        crate::transport::post_run_stream(&req, &mut emit).await;
        let _ = (host, port);
    });
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let mut emit = |frame: crate::stream::SseFrame| {
            let _ = tx.send(frame);
        };
        if let Err(message) = crate::transport::post_run(&host, port, &req, &mut emit) {
            let _ = tx.send(crate::stream::SseFrame {
                event: "error".to_string(),
                data: serde_json::json!({"message": message}),
            });
        }
    });
}

/// Shared stop path (composer Stop button): `POST /api/abort {chat_id}` on a
/// worker thread — fire-and-forget like React's `abortRun` from Stop handlers
/// (fetch-abort alone only detaches this client while the run keeps burning
/// tokens) — then mark the run dead locally. The inbox stays open so the
/// backend's `turn_aborted` / `done` frames still land instead of stranding
/// the live row.
pub fn stage_abort(s: &mut RuneTranscriptState) {
    if !s.live {
        return;
    }
    s.live = false;
    let (host, port, chat_id) = (s.api_host.clone(), s.api_port, s.chat_id.clone());
    // Browsers have no TCP sockets: abort over fetch, same as the run itself.
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let _ = crate::transport::fetch_abort(&chat_id).await;
        let _ = (host, port);
    });
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let _ = crate::transport::post_abort(&host, port, &chat_id);
    });
}

/// Host access for views composed into a larger shell: handlers stay typed to
/// the shell state while reading transcript data. The standalone transcript
/// view hosts itself. `Main` is the fallback thread; `Open` is the transcript
/// sheet's card thread (resolved through the sessions map — one page, chat
/// types only, no workspace routes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thread {
    Main,
    Open,
}

pub trait TranscriptHost {
    fn thread(&self, which: Thread) -> &RuneTranscriptState;
    fn thread_mut(&mut self, which: Thread) -> &mut RuneTranscriptState;
}

impl TranscriptHost for RuneTranscriptState {
    fn thread(&self, _which: Thread) -> &RuneTranscriptState {
        self
    }
    fn thread_mut(&mut self, _which: Thread) -> &mut RuneTranscriptState {
        self
    }
}

pub fn transcript_view(s: &RuneTranscriptState, _: &mut Cx<RuneTranscriptState>) -> El {
    transcript_body::<RuneTranscriptState>(s, Thread::Main)
}

/// Transcript body shared by the standalone view and the shell: rows read
/// `s`, handlers resolve through `S: TranscriptHost` for `which` thread.
pub fn transcript_body<S: TranscriptHost + 'static>(s: &RuneTranscriptState, which: Thread) -> El {
    // Thread-local running banner lives in `transcript_body` (the shell's
    // sheet drops its own when the thread is live — see `sheet_running`).
    if s.live {
        return col()
            .key(format!("transcript-body-live-{}", s.chat_id))
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .fill(bg())
            .child(transcript_view_body::<S>(s, which));
    }
    transcript_view_body::<S>(s, which)
}

/// The body without the live banner (sheet uses this + its own banner).
pub fn transcript_view_body<S: TranscriptHost + 'static>(
    s: &RuneTranscriptState,
    which: Thread,
) -> El {
    transcript_body_with::<S>(s, which, true, false, true, 0.0, 0.0)
}

/// Deduped home transcript state (`MessageStream`): agent rows that merely
/// repeat an error row disappear, and rows group under local-day dividers.
struct HomeRows {
    visible: Vec<bool>,
    dividers: Vec<Option<String>>,
}

impl HomeRows {
    fn new(rows: &[RowData]) -> Self {
        // `normalizeError`: trim, collapse whitespace, lowercase.
        fn normalize(text: &str) -> String {
            text.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        }
        let errors: Vec<String> = rows
            .iter()
            .filter(|r| r.role == "error" && !r.body.trim().is_empty())
            .map(|r| normalize(&r.body))
            .collect();
        let mut visible = vec![true; rows.len()];
        for (i, r) in rows.iter().enumerate() {
            if r.role != "agent" || r.body.trim().is_empty() || errors.is_empty() {
                continue;
            }
            let text = normalize(&r.body);
            if errors.iter().any(|e| e == &text) {
                visible[i] = false;
                continue;
            }
            if let Some(rest) = text.strip_prefix("error:") {
                let rest = rest.trim();
                if errors.iter().any(|e| {
                    e.contains(rest) || text.contains(e.trim_start_matches("error:").trim())
                }) {
                    visible[i] = false;
                }
            }
        }
        let mut dividers = vec![None; rows.len()];
        let mut previous_day = String::new();
        for i in 0..rows.len() {
            if !visible[i] {
                continue;
            }
            let day = day_key(&rows[i].at);
            if !day.is_empty() && day != previous_day {
                dividers[i] = Some(day_label(&rows[i].at));
            }
            previous_day = day;
        }
        Self { visible, dividers }
    }
}

fn home_ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

fn home_rgba(r: f32, g: f32, b: f32, a: f32) -> rune::Color {
    // Rune `Color` is linear RGB; CSS/React values here are sRGB.
    fn linear(c: f32) -> f32 {
        let c = c / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    rune::Color {
        r: linear(r),
        g: linear(g),
        b: linear(b),
        a,
    }
}

/// Home message type scale: 15.5pt at React's `leading-[1.85]`.
fn home_body_style() -> rune::tok::TextStyle {
    let mut style = rune::tok::TextStyle::new(15.5, 400.0);
    style.line_height = 15.5 * 1.85;
    style
}

/// Lucide `Copy` at a 24-unit box.
fn home_icon_copy(size: f32, color: rune::Color) -> rune::Shape {
    let p = rune::Path::new()
        .move_to([10.0, 8.0])
        .line_to([20.0, 8.0])
        .cubic_to([21.1, 8.0], [22.0, 8.9], [22.0, 10.0])
        .line_to([22.0, 20.0])
        .cubic_to([22.0, 21.1], [21.1, 22.0], [20.0, 22.0])
        .line_to([10.0, 22.0])
        .cubic_to([8.9, 22.0], [8.0, 21.1], [8.0, 20.0])
        .line_to([8.0, 10.0])
        .cubic_to([8.0, 8.9], [8.9, 8.0], [10.0, 8.0])
        .close()
        .move_to([4.0, 16.0])
        .cubic_to([2.9, 16.0], [2.0, 15.1], [2.0, 14.0])
        .line_to([2.0, 4.0])
        .cubic_to([2.0, 2.9], [2.9, 2.0], [4.0, 2.0])
        .line_to([14.0, 2.0])
        .cubic_to([15.1, 2.0], [16.0, 2.9], [16.0, 4.0]);
    rune::Shape::new()
        .stroke_path(&p, crate::lucide_stroke(size), rune::LineCap::Round, color)
        .view_box(24.0, 24.0)
}

/// Lucide `RotateCcw` at a 24-unit box (rewind / retry).
fn home_icon_rotate(size: f32, color: rune::Color) -> rune::Shape {
    const K: f32 = 4.971;
    let p = rune::Path::new()
        .move_to([12.0, 3.0])
        .cubic_to([12.0 + K, 3.0], [21.0, 12.0 - K], [21.0, 12.0])
        .cubic_to([21.0, 12.0 + K], [12.0 + K, 21.0], [12.0, 21.0])
        .cubic_to([12.0 - K, 21.0], [3.0, 12.0 + K], [3.0, 12.0])
        .cubic_to([3.0, 9.9], [3.8, 7.8], [5.2, 6.2])
        .move_to([3.0, 3.0])
        .line_to([3.0, 8.0])
        .line_to([8.0, 8.0]);
    rune::Shape::new()
        .stroke_path(&p, crate::lucide_stroke(size), rune::LineCap::Round, color)
        .view_box(24.0, 24.0)
}

/// Lucide `CircleAlert` at a 24-unit box.
fn home_icon_alert(size: f32, color: rune::Color) -> rune::Shape {
    let circle = rune::Path::new()
        .move_to([12.0, 2.0])
        .cubic_to([17.5, 2.0], [22.0, 6.5], [22.0, 12.0])
        .cubic_to([22.0, 17.5], [17.5, 22.0], [12.0, 22.0])
        .cubic_to([6.5, 22.0], [2.0, 17.5], [2.0, 12.0])
        .cubic_to([2.0, 6.5], [6.5, 2.0], [12.0, 2.0])
        .close();
    rune::Shape::new()
        .stroke_path(
            &circle,
            crate::lucide_stroke(size),
            rune::LineCap::Round,
            color,
        )
        .line(
            [12.0, 8.0],
            [12.0, 12.0],
            crate::lucide_stroke(size),
            rune::LineCap::Round,
            color,
        )
        .line(
            [12.0, 16.0],
            [12.01, 16.0],
            crate::lucide_stroke(size),
            rune::LineCap::Round,
            color,
        )
        .view_box(24.0, 24.0)
}

fn home_icon_button(key: String, label: &'static str, size: f32, icon: rune::Shape) -> El {
    rune::el()
        .key(key)
        .role("button")
        .label(label)
        .size(size, size)
        .radius(6.0)
        .center()
        .child(rune::el().size(12.0, 12.0).inert().shape(icon))
}

/// `chat-date-divider`: hairlines fading from the edges + uppercase pill.
fn home_divider(label: &str) -> El {
    let line = || {
        rune::el().grow(1.0).h(1.0).linear_gradient(
            std::f32::consts::FRAC_PI_2,
            [
                (0.0, home_ink("#ffffff00")),
                (0.5, home_ink("#ffffff12")),
                (1.0, home_ink("#ffffff00")),
            ],
        )
    };
    row()
        .w_full()
        .items_center()
        .gap(10.0)
        .mt(6.0)
        .mb(18.0)
        .child(line())
        .child(
            text(label.to_ascii_uppercase())
                .font({
                    let mut style = rune::tok::TextStyle::new(9.0, 600.0);
                    style.line_height = 12.8;
                    style
                })
                .color(home_ink("#5f5f68"))
                .px(8.0)
                .py(2.0)
                .radius(999.0)
                .border(1.0, home_ink("#ffffff0f"))
                .fill(home_ink("#ffffff05")),
        )
        .child(line())
}

fn home_user_row<S: TranscriptHost + 'static>(s: &S, which: Thread, r: &RowData, max_w: f32) -> El {
    let stamp = clock(&r.at);
    let bubble = col()
        .key(format!("{}-bubble", r.key))
        .radius(14.0)
        .linear_gradient(
            145.0_f32.to_radians(),
            [
                (0.0, home_rgba(48.0, 48.0, 51.0, 0.92)),
                (1.0, home_rgba(35.0, 35.0, 38.0, 0.95)),
            ],
        )
        .border(1.0, home_ink("#ffffff17"))
        .border_layout()
        .px(16.0)
        .py(10.0)
        .max_w(max_w)
        .child(
            render_body_styled::<S>(s, which, &r.key, &r.body, "#e4e4e7", home_body_style())
                .pt(12.0)
                .pb(12.0),
        );
    let mut actions = row()
        .key(format!("{}-actions", r.key))
        .w_full()
        .justify_end()
        .items_center()
        .gap(4.0)
        .mt(4.0)
        .pr(2.0);
    if !stamp.is_empty() {
        actions = actions.child(
            text(stamp)
                .key(format!("{}-time", r.key))
                .font({
                    let mut style = rune::tok::TextStyle::new(10.0, 400.0);
                    style.line_height = 15.0;
                    style
                })
                .color(home_ink("#52525b")),
        );
    }
    actions = actions
        .child(home_icon_button(
            format!("{}-copy", r.key),
            "Copy message",
            24.0,
            home_icon_copy(12.0, home_ink("#52525b")),
        ))
        .child(home_icon_button(
            format!("{}-rewind", r.key),
            "Rewind to this message",
            24.0,
            home_icon_rotate(12.0, home_ink("#52525b")),
        ));
    col()
        .key(format!("{}-group", r.key))
        .w_full()
        .mb(24.0)
        .child(row().w_full().justify_end().child(bubble))
        .child(actions)
}

/// Collapsed run divider (`TraceAccordion` complete header): full-width
/// "Worked for 8s ›" with a bottom hairline, then the 9pt collapsed-body
/// spacer. The dropdown contents stay closed, exactly like the default UI.
fn home_trace_header(r: &RowData) -> El {
    let label = if r.duration.is_empty() {
        "Worked".to_string()
    } else {
        format!("Worked for {}", r.duration)
    };
    col()
        .key(format!("{}-trace", r.key))
        .w_full()
        .mt(8.0)
        .mb(22.0)
        .child(
            row()
                .key(format!("{}-trace-head", r.key))
                .w_full()
                .h(42.0)
                .items_center()
                .gap(7.0)
                .border_sides([0.0, 0.0, 1.0, 0.0], home_ink("#ffffff16"))
                .border_layout()
                .child(
                    text(label)
                        .font({
                            let mut style = rune::tok::TextStyle::new(13.0, 500.0);
                            style.line_height = 19.5;
                            style
                        })
                        .color(home_ink("#a4a4ad")),
                )
                .child(
                    rune::el()
                        .key(format!("{}-trace-chevron", r.key))
                        .size(14.0, 14.0)
                        .inert()
                        .shape({
                            let p = rune::Path::new()
                                .move_to([9.0, 6.0])
                                .line_to([15.0, 12.0])
                                .line_to([9.0, 18.0]);
                            rune::Shape::new()
                                .stroke_path(
                                    &p,
                                    crate::lucide_stroke(14.0),
                                    rune::LineCap::Round,
                                    home_ink("#71717a"),
                                )
                                .view_box(24.0, 24.0)
                        }),
                ),
        )
        .child(rune::el().key(format!("{}-trace-gap", r.key)).h(9.0))
}

fn home_agent_row<S: TranscriptHost + 'static>(
    s: &S,
    which: Thread,
    r: &RowData,
    live: bool,
) -> El {
    let mut response =
        render_body_styled::<S>(s, which, &r.key, &r.body, "#e4e4e7", home_body_style())
            .pt(12.0)
            .pb(12.0);
    if live {
        let mut dots = row()
            .key(format!("{}-typing", r.key))
            .items_center()
            .gap(3.0)
            .pt(4.0);
        for (n, delay) in [0.0, 0.14, 0.28].into_iter().enumerate() {
            dots = dots.child(
                rune::el()
                    .key(format!("{}-typing-{n}", r.key))
                    .size(3.0, 3.0)
                    .radius(1.5)
                    .fill(home_ink("#9b9ba5"))
                    .looping(rune::LoopSpec::opacity(1.1, delay, 0.35, 1.0)),
            );
        }
        response = response.child(dots);
    }
    let mono = {
        let mut style = rune::tok::TextStyle::new(10.5, 400.0).mono();
        // `.agent-message-footer`: text-[10.5px] at leading 1.5.
        style.line_height = 15.75;
        style
    };
    let mut footer = row()
        .key(format!("{}-footer", r.key))
        .items_center()
        .gap(12.0)
        .mt(8.0)
        .child(
            text("↑0".to_string())
                .key(format!("{}-in", r.key))
                .font(mono)
                .color(home_ink("#52525b")),
        )
        .child(
            text("↓0".to_string())
                .key(format!("{}-out", r.key))
                .font(mono)
                .color(home_ink("#52525b")),
        );
    let stamp = clock(&r.at);
    if !stamp.is_empty() {
        footer = footer.child(
            text(stamp)
                .key(format!("{}-time", r.key))
                .font(mono)
                .color(home_ink("#52525b")),
        );
    }
    let mut group = col()
        .key(format!("{}-group", r.key))
        .w_full()
        .mb(24.0)
        .hover_group();
    if r.show_trace {
        group = group.child(home_trace_header(r));
    }
    if !r.hide_body {
        group = group.child(response);
    }
    group.child(footer).child(home_agent_actions(r))
}

/// Hover-revealed agent actions (React reserves the 25pt row at rest).
fn home_agent_actions(r: &RowData) -> El {
    row()
        .key(format!("{}-actions", r.key))
        .w_full()
        .h(25.0)
        .justify_end()
        .items_center()
        .gap(2.0)
        .mt(8.0)
        .group_opacity(0.0, 1.0)
        .child(home_icon_button(
            format!("{}-retry", r.key),
            "Retry — re-run the last request",
            25.0,
            home_icon_rotate(12.0, home_ink("#71717a")),
        ))
        .child(home_icon_button(
            format!("{}-copy", r.key),
            "Copy to clipboard",
            25.0,
            home_icon_copy(12.0, home_ink("#71717a")),
        ))
}

fn home_error_row(r: &RowData) -> El {
    let mut body_style = rune::tok::TextStyle::new(13.0, 400.0);
    body_style.line_height = 13.0 * 1.625;
    let body = text(r.body.clone())
        .key(format!("{}-error-text", r.key))
        .font(body_style)
        .wrap_anywhere()
        .color(home_rgba(252.0, 165.0, 165.0, 0.9));
    let retry = row()
        .key(format!("{}-retry", r.key))
        .role("button")
        .label("Retry")
        .items_center()
        .gap(4.0)
        .px(8.0)
        .py(4.0)
        .radius(6.0)
        .border(1.0, home_rgba(248.0, 113.0, 113.0, 0.15))
        .border_layout()
        .fill(home_rgba(248.0, 113.0, 113.0, 0.06))
        .child(
            rune::el()
                .size(10.0, 10.0)
                .inert()
                .shape(home_icon_rotate(10.0, home_rgba(254.0, 202.0, 202.0, 0.9))),
        )
        .child(
            text("Retry".to_string())
                .font({
                    let mut style = rune::tok::TextStyle::new(10.0, 500.0);
                    style.line_height = 15.0;
                    style
                })
                .color(home_rgba(254.0, 202.0, 202.0, 0.9)),
        );
    let copy = row()
        .key(format!("{}-copy", r.key))
        .role("button")
        .label("Copy")
        .items_center()
        .gap(4.0)
        .px(8.0)
        .py(4.0)
        .radius(6.0)
        .child(
            rune::el()
                .size(10.0, 10.0)
                .inert()
                .shape(home_icon_copy(10.0, home_ink("#71717a"))),
        )
        .child(
            text("Copy".to_string())
                .font({
                    let mut style = rune::tok::TextStyle::new(10.0, 400.0);
                    style.line_height = 15.0;
                    style
                })
                .color(home_ink("#71717a")),
        );
    col()
        .key(format!("{}-group", r.key))
        .w_full()
        .mb(24.0)
        .child(
            row()
                .w_full()
                .gap(10.0)
                .py(4.0)
                .child(
                    rune::el()
                        .key(format!("{}-alert", r.key))
                        .size(15.0, 15.0)
                        .mt(3.0)
                        .inert()
                        .shape(home_icon_alert(15.0, home_rgba(248.0, 113.0, 113.0, 0.9))),
                )
                .child(
                    col().grow(1.0).min_w(0.0).child(body).child(
                        row()
                            .items_center()
                            .gap(4.0)
                            .mt(6.0)
                            .child(retry)
                            .child(copy),
                    ),
                ),
        )
}

/// Lucide `Square` at a 24-unit box.
fn home_icon_square(size: f32, color: rune::Color) -> rune::Shape {
    let p = rune::Path::new()
        .move_to([5.0, 3.0])
        .line_to([19.0, 3.0])
        .cubic_to([20.1, 3.0], [21.0, 3.9], [21.0, 5.0])
        .line_to([21.0, 19.0])
        .cubic_to([21.0, 20.1], [20.1, 21.0], [19.0, 21.0])
        .line_to([5.0, 21.0])
        .cubic_to([3.9, 21.0], [3.0, 20.1], [3.0, 19.0])
        .line_to([3.0, 5.0])
        .cubic_to([3.0, 3.9], [3.9, 3.0], [5.0, 3.0])
        .close();
    rune::Shape::new()
        .stroke_path(&p, crate::lucide_stroke(size), rune::LineCap::Round, color)
        .view_box(24.0, 24.0)
}

/// Lucide `Archive` at a 24-unit box.
fn home_icon_archive(size: f32, color: rune::Color) -> rune::Shape {
    let p = rune::Path::new()
        .move_to([3.0, 3.0])
        .line_to([21.0, 3.0])
        .line_to([21.0, 8.0])
        .line_to([3.0, 8.0])
        .close()
        .move_to([5.0, 8.0])
        .line_to([5.0, 19.0])
        .cubic_to([5.0, 20.1], [5.9, 21.0], [7.0, 21.0])
        .line_to([17.0, 21.0])
        .cubic_to([18.1, 21.0], [19.0, 20.1], [19.0, 19.0])
        .line_to([19.0, 8.0])
        .move_to([10.0, 12.0])
        .line_to([14.0, 12.0]);
    rune::Shape::new()
        .stroke_path(&p, crate::lucide_stroke(size), rune::LineCap::Round, color)
        .view_box(24.0, 24.0)
}

/// `SessionEventDivider` (`MessageBubble.tsx` + `.session-event*` CSS): the
/// hairline-framed "Session aborted / compacted" status row.
fn home_session_event(r: &RowData) -> El {
    let aborted = r.event == "aborted";
    let line = || {
        rune::el().grow(1.0).h(1.0).linear_gradient(
            std::f32::consts::FRAC_PI_2,
            [
                (0.0, home_rgba(255.0, 255.0, 255.0, 0.0)),
                (0.5, home_rgba(255.0, 255.0, 255.0, 0.09)),
                (1.0, home_rgba(255.0, 255.0, 255.0, 0.0)),
            ],
        )
    };
    let icon = if aborted {
        home_icon_square(10.0, home_ink("#fcd34d"))
    } else {
        home_icon_archive(11.0, home_ink("#8b8b94"))
    };
    let icon_box = rune::el()
        .key(format!("{}-icon", r.key))
        .size(18.0, 18.0)
        .radius(6.0)
        .border(
            1.0,
            if aborted {
                home_rgba(252.0, 211.0, 77.0, 0.2)
            } else {
                home_rgba(255.0, 255.0, 255.0, 0.08)
            },
        )
        .fill(if aborted {
            home_rgba(252.0, 211.0, 77.0, 0.06)
        } else {
            home_rgba(255.0, 255.0, 255.0, 0.035)
        })
        .center()
        .inert()
        .child(rune::el().size(10.0, 10.0).inert().shape(icon));
    let mut main = row()
        .key(format!("{}-main", r.key))
        .items_center()
        .gap(6.0)
        .shrink()
        .child(icon_box)
        .child(
            text(if aborted {
                "Session aborted".to_string()
            } else {
                "Session compacted".to_string()
            })
            .font({
                let mut style = rune::tok::TextStyle::new(10.5, 600.0);
                style.line_height = 14.0;
                style.tracking = 0.01;
                style
            })
            .color(if aborted {
                home_ink("#e4c46a")
            } else {
                home_ink("#a1a1a9")
            }),
        );
    if aborted {
        main = main.child(
            text("partial response kept".to_string())
                .font({
                    let mut style = rune::tok::TextStyle::new(9.5, 400.0).mono();
                    style.line_height = 9.5;
                    style
                })
                .color(home_ink("#5f5f68")),
        );
    }
    if aborted {
        main = main.child(
            row()
                .key(format!("{}-retry", r.key))
                .role("button")
                .label("Retry")
                .items_center()
                .gap(3.0)
                .ml(3.0)
                .px(6.0)
                .py(3.0)
                .radius(6.0)
                .border(1.0, home_rgba(255.0, 255.0, 255.0, 0.1))
                .border_layout()
                .fill(home_rgba(255.0, 255.0, 255.0, 0.04))
                .hover(home_rgba(255.0, 255.0, 255.0, 0.1))
                .child(
                    rune::el()
                        .size(10.0, 10.0)
                        .inert()
                        .shape(home_icon_rotate(10.0, home_ink("#cfcfd6"))),
                )
                .child(
                    text("Retry".to_string())
                        .font({
                            let mut style = rune::tok::TextStyle::new(9.0, 400.0);
                            style.line_height = 12.0;
                            style
                        })
                        .color(home_ink("#cfcfd6")),
                ),
        );
    }
    row()
        .key(format!("{}-session-event", r.key))
        .w_full()
        .items_center()
        .gap(10.0)
        .mt(10.0)
        .mb(24.0)
        .child(line())
        .child(main)
        .child(line())
}

/// One home row (`MessageStream` + `MessageBubble`): local-day divider, then
/// the role's flat layout — no avatars, no bubble names, 24pt group spacing.
fn home_row<S: TranscriptHost + 'static>(
    s: &S,
    which: Thread,
    i: usize,
    meta: &HomeRows,
    max_w: f32,
) -> El {
    let t = s.thread(which);
    let r = &t.rows[i];
    let mut root = col().key(r.key.clone()).w_full();
    if let Some(label) = &meta.dividers[i] {
        root = root.child(home_divider(label));
    }
    if !meta.visible[i] {
        return root;
    }
    let live = t.live && i + 1 == t.rows.len() && r.role != "user" && r.role != "error";
    let group = if !r.event.is_empty() {
        home_session_event(r)
    } else {
        match r.role {
            "user" => home_user_row::<S>(s, which, r, max_w),
            "error" => home_error_row(r),
            _ => home_agent_row::<S>(s, which, r, live),
        }
    };
    root.child(group)
}

/// `ConversationRail`: up to six 7×2 turn dashes centered in a 24pt column
/// at the scroll area's left edge (React hides it below two turns).
fn conversation_rail(rows: &[RowData]) -> Option<El> {
    let mut turns: Vec<Vec<&RowData>> = Vec::new();
    let mut current: Vec<&RowData> = Vec::new();
    for r in rows.iter().filter(|r| !r.body.trim().is_empty()) {
        if r.role == "user" && !current.is_empty() {
            turns.push(std::mem::take(&mut current));
        }
        current.push(r);
    }
    if !current.is_empty() {
        turns.push(current);
    }
    if turns.len() < 2 {
        return None;
    }
    let role_of = |turn: &[&RowData]| {
        if turn.iter().any(|r| r.role == "user") {
            "user"
        } else if turn.iter().any(|r| r.role == "error") {
            "error"
        } else {
            "agent"
        }
    };
    let mut markers: Vec<(&str, String)> = turns
        .iter()
        .enumerate()
        .map(|(i, turn)| (role_of(turn), format!("mark-{i}")))
        .collect();
    if markers.len() > 6 {
        // Fold everything older than the newest five into one quiet dash.
        let earlier = markers.len() - 5;
        let mut folded = vec![("system", "mark-earlier".to_string())];
        folded.extend(markers.drain(earlier..));
        markers = folded;
    }
    let mut rail = col()
        .key("conversation-rail")
        .w(24.0)
        .pin(Some(4.0), Some(16.0), None, Some(16.0))
        .justify_center()
        .inert();
    for (role, id) in markers {
        let color = match role {
            "user" => rune::Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.46,
            },
            "error" => home_rgba(251.0, 113.0, 133.0, 0.72),
            "system" => rune::Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.18,
            },
            _ => rune::Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.28,
            },
        };
        rail = rail.child(
            row()
                .key(format!("conversation-{id}"))
                .w(24.0)
                .h(14.0)
                .items_center()
                .child(rune::el().size(7.0, 2.0).radius(1.0).fill(color)),
        );
    }
    Some(rail)
}

/// Home chat card body (`HouseChat` `variant="home"`): no header row — the
/// card is the floating conversation panel, just messages + prompt dock.
pub fn home_transcript_body<S: TranscriptHost + 'static>(
    s: &RuneTranscriptState,
    which: Thread,
    card_w: f32,
    window_h: f32,
) -> El {
    // React caps the user bubble at 85% of the message column: card width
    // minus its 8pt side gutters, hairlines, and the scroll area's pl-8/pr-4.
    let max_w = ((card_w - 18.0 - 48.0) * 0.85).max(120.0);
    transcript_body_with::<S>(s, which, false, true, false, max_w, window_h)
}

fn transcript_body_with<S: TranscriptHost + 'static>(
    s: &RuneTranscriptState,
    which: Thread,
    show_head: bool,
    dock_compact: bool,
    show_sheets: bool,
    home_max_w: f32,
    window_h: f32,
) -> El {
    // Index-built rows: layout builds only the mounted window (viewport + up to
    // two viewports either side), so a 10k transcript commits in window time.
    // Handlers must find items by key, never by index (see `RowSource` docs).
    // Thread padding lives on the stack (outside rows, estimates stay valid);
    // user rows `justify_end` with content-sized bubbles like
    // `.canvas-bubble-row.is-user`; agent bubbles `grow` full width.
    let home = !show_head;
    // React's home scroll area is `pl-8 pr-4 py-3`; the sheet keeps 12pt.
    let mut stack = col().key("transcript-stack").growth(GrowthSpec::default());
    stack = if home {
        stack.pl(32.0).pr(16.0).pt(12.0).pb(12.0)
    } else {
        stack.px(12.0)
    };
    let home_rows = if home {
        Some(HomeRows::new(&s.rows))
    } else {
        None
    };
    // Empty home thread (`MessageStream` isEmpty): a 60vh drop surface with
    // the centered "What to do Boss?" prompt. It lives inside the scroll
    // area, so the card caps at 74vh and the prompt sits at the bottom
    // (stick-to-bottom), exactly like React.
    let stack = if home && s.rows.is_empty() {
        stack.child(
            col()
                .key("transcript-empty")
                .w_full()
                .h((window_h * 0.6).max(80.0))
                .center()
                .child(
                    text("What to do Boss?".to_string())
                        .font(rune::tok::TextStyle::new(19.0, 500.0))
                        .color(home_ink("#ffffff")),
                ),
        )
    } else {
        stack.virtual_rows(
            s.rows.len(),
            ESTIMATED_ROW_H,
            // Typed to the window state `S` (Rune checks the registration):
            // read rows through the host, never by closing over `s`.
            move |s: &S, i| Cow::Borrowed(s.thread(which).rows[i].key.as_str()),
            move |s: &S, i| {
                let t = s.thread(which);
                if let Some(meta) = &home_rows {
                    return home_row::<S>(s, which, i, meta, home_max_w);
                }
                // Live row: the streaming agent row (last, non-user, non-error)
                // while a run is in flight. Mirrors
                // `.canvas-bubble-row.is-live .canvas-bubble` (`#1b1b20`, .10
                // hairline) plus the 3-dot `.canvas-bubble-typing` bounce
                // (1.1s period, .14s stagger).
                let live_row = t.live
                    && i + 1 == t.rows.len()
                    && t.rows[i].role != "user"
                    && t.rows[i].role != "error";
                let r = &s.thread(which).rows[i];
                // Role palette from `canvas.css` `.canvas-bubble*`: per-role
                // fills, hairlines (agent .05 / user .075 / live .10), and
                // the translucent rose error system; 22px avatars, 11px/650
                // `#f0f0f3` name headers, role-tinted 12.5px bodies
                // (`#dedee5` user, `#d2d2da` agent).
                let (fill, hair_a, avatar_bg, initial, initial_ink, name, body_ink) = match r.role {
                    "user" => (
                        "#28282f", 0.075, "#303038", "Y", "#e6e6ec", "You", "#dedee5",
                    ),
                    "error" => (
                        "#fb718511",
                        0.24,
                        "#3a2226",
                        "!",
                        "#f0f0f3",
                        "Error",
                        "#d2d2da",
                    ),
                    "system" => (
                        "#101013", 0.05, "#24242a", "S", "#c9c9d1", "System", "#d2d2da",
                    ),
                    _ => (
                        "#19191e", 0.05, "#24242a", "A", "#c9c9d1", "Agent", "#d2d2da",
                    ),
                };
                let ink = |hex: &'static str| rune::color::hex(hex).unwrap_or(rune::Color::BLACK);
                let hairline = if live_row {
                    rune::Color {
                        r: 1.0,
                        g: 1.0,
                        b: 1.0,
                        a: 0.10,
                    }
                } else if r.role == "error" {
                    rune::color::hex("#fb71853d").unwrap_or(rune::Color::BLACK)
                } else {
                    rune::Color {
                        r: 1.0,
                        g: 1.0,
                        b: 1.0,
                        a: hair_a,
                    }
                };
                let name_style = rune::tok::TextStyle::new(11.0, 650.0);
                let initial_style = rune::tok::TextStyle::new(9.5, 700.0);
                let bubble = col()
                    .key(format!("{}-bubble", r.key))
                    .radius(14.0)
                    .fill(ink(if live_row { "#1b1b20" } else { fill }))
                    .border(1.0, hairline)
                    .px(13.0)
                    .py(10.0)
                    .child(
                        text(name.to_string())
                            .key(format!("{}-name", r.key))
                            .font(name_style)
                            .color(ink("#f0f0f3")),
                    )
                    .child(render_body::<S>(s, which, &r.key, &r.body, body_ink));
                // Bubble foot (`canvas-bubble-foot`): `hh:MM AM/PM`, nothing
                // else — copy/rewind need clipboard + server endpoints Rune
                // has no target for, so they stay out (documented, not faked).
                let bubble = match clock(&r.at) {
                    t if t.is_empty() => bubble,
                    t => bubble.child(
                        text(t)
                            .key(format!("{}-time", r.key))
                            .font(rune::tok::TextStyle::new(10.0, 400.0))
                            .color(ink("#71717a")),
                    ),
                };
                // Typing bounce on the live row only (`.canvas-bubble-typing`).
                let bubble = if live_row {
                    let mut dots = row()
                        .key(format!("{}-typing", r.key))
                        .items_center()
                        .gap(3.0)
                        .pt(4.0);
                    for (n, delay) in [0.0, 0.14, 0.28].into_iter().enumerate() {
                        dots = dots.child(
                            rune::el()
                                .key(format!("{}-typing-{n}", r.key))
                                .size(3.0, 3.0)
                                .radius(1.5)
                                .fill(ink("#9b9ba5"))
                                .looping(rune::LoopSpec::opacity(1.1, delay, 0.35, 1.0)),
                        );
                    }
                    bubble.child(dots)
                } else {
                    bubble
                };
                let bubble = if r.role == "user" {
                    // Content-sized (no `grow`) + `justify_end` row hugs right
                    // like `.canvas-bubble-row.is-user`. `max_w` bounds the
                    // wrap: 468 viewport − 24 stack px − 22 avatar − 8 gap.
                    // Without it the bubble measures max-content and overflows.
                    bubble.max_w(414.0)
                } else {
                    bubble.grow(1.0).min_h(0.0)
                };
                let message = row()
                    .key(r.key.clone())
                    .role("bubble")
                    .label(r.role.to_string())
                    .w_full()
                    .gap(8.0)
                    .child(
                        rune::el()
                            .key(format!("{}-avatar", r.key))
                            .size(22.0, 22.0)
                            .radius(11.0)
                            .fill(ink(avatar_bg))
                            .center()
                            .child(
                                text(initial.to_string())
                                    .font(initial_style)
                                    .color(ink(initial_ink)),
                            ),
                    )
                    .child(bubble);
                if r.role == "user" {
                    message.justify_end()
                } else {
                    message
                }
            },
        )
    };
    // Bounded root (window height) + growing viewport: the viewport takes the
    // free space between title and composer instead of pushing the composer
    // off-screen (an `h_full` viewport in an auto-height column does that).
    let viewport = col()
        .key("transcript-viewport")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: true })
        .child(stack);
    let mut root = col()
        .key("stark-transcript")
        .w_full()
        // `grow`, not `h_full`: fills a standalone window and shares a shell
        // column with the tab bar without overflowing it.
        .grow(1.0)
        .min_h(0.0);
    // The home card supplies the surface (rgba 13,13,16/.94); the standalone
    // sheet still paints its own opaque page background.
    if show_head {
        root = root.fill(bg());
    }
    if show_head {
        root = root.child(
            row()
                .key("transcript-head")
                .w_full()
                .items_center()
                .gap(8.0)
                .px(16.0)
                .pt(14.0)
                .pb(10.0)
                .child(
                    text(s.title.clone())
                        .key("transcript-title")
                        .grow(1.0)
                        .color(rune::color::hex("#f2f2f5").unwrap_or(rune::Color::BLACK)),
                )
                // Sticky notes (chat scope, like `ChatView.tsx`'s NotesButton).
                .child(super::notes::notes_button::<S>(
                    &s.notes,
                    which,
                    s.chat_id.clone(),
                ))
                .child(
                    text("Settings".to_string())
                        .key("settings-open")
                        .role("button")
                        .label("Open settings")
                        .font(rune::tok::TextStyle::new(11.0, 500.0))
                        .color(rune::color::hex("#8a8a93").unwrap_or(rune::Color::BLACK))
                        .on_press(move |s: &mut S| {
                            s.thread_mut(which).settings_open = true;
                        }),
                ),
        );
    }
    // `MessageStream` wrapper: scroll area plus the turn-mark rail pinned to
    // its left edge (React mounts the rail above the scroller).
    let mut view = col()
        .key("transcript-body")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(viewport);
    if let Some(rail) = conversation_rail(&s.rows) {
        view = view.child(rail);
    }
    root = root.child(view);
    // Thread-local running banner: the standalone view carries it here; the
    // shell's sheet drops it (sheet keeps its own).
    if s.live {
        root = root.child(
            text("Running".to_string())
                .key("transcript-live")
                .px(16.0)
                .color(rune::color::hex("#ededf0").unwrap_or(rune::Color::BLACK)),
        );
    }
    // Composer card: shared `.premium-surface` dock (see `composer.rs`).
    // Send (button or Enter) stages the field as a user row, marks the run
    // live, and spawns the blocking `POST /api/run` thread; frames land in
    // `inbox`, drained by the `poll` hook before every frame.
    let composer_card = super::composer::composer_card(
        &s.edit,
        move |s: &mut S| &mut s.thread_mut(which).edit,
        s.voice.meter(),
        s.live,
        super::composer::ComposerMeta {
            target: &s.run_target,
            usage: &s.usage,
            usage_open: s.usage_open,
            provider_label: &s.provider.label,
            provider_warn: s.provider.warn,
            model: &s.model,
            // HouseChat's PromptDock never passes `onModelChange`, so the
            // home card has no model dropup (React renders provider only).
            models: if dock_compact { &[] } else { &s.models },
            model_open: s.model_open,
        },
        super::composer::ComposerButtons {
            voice: std::rc::Rc::new(move |s: &mut S| {
                let on = !s.thread_mut(which).voice.live;
                s.thread_mut(which).voice.set_live(on);
            }),
            cancel: std::rc::Rc::new(move |s: &mut S| stage_abort(s.thread_mut(which))),
            local: std::rc::Rc::new(move |s: &mut S| {
                s.thread_mut(which).run_target = "local".to_string();
            }),
            cloud: std::rc::Rc::new(move |s: &mut S| {
                s.thread_mut(which).run_target = "cloud".to_string();
            }),
            usage: std::rc::Rc::new(move |s: &mut S| {
                let open = s.thread_mut(which).usage_open;
                s.thread_mut(which).usage_open = !open;
            }),
            send: std::rc::Rc::new(move |s: &mut S| stage_send(s.thread_mut(which))),
            model_menu: std::rc::Rc::new(move |s: &mut S| {
                let open = s.thread_mut(which).model_open;
                s.thread_mut(which).model_open = !open;
            }),
            model_pick: std::rc::Rc::new(move |s: &mut S, id: String| {
                s.thread_mut(which).model = id;
                s.thread_mut(which).model_open = false;
            }),
            stats: std::rc::Rc::new(move |s: &mut S| {
                s.thread_mut(which).stats_open = true;
                s.thread_mut(which).usage_open = false;
            }),
            config: std::rc::Rc::new(move |s: &mut S| {
                crate::agent_config::open_agent_config(&mut s.thread_mut(which).agent_config);
            }),
        },
    );
    let dock = if dock_compact {
        // Inside the home chat card the outer card owns the 8pt gutter and
        // React nests two wrappers here: HouseChat `pt-1` then PromptDock's
        // `px-2 pt-1.5 pb-1.5` (safe-area inline override = 6pt bottom).
        rune::el()
            .key("composer-dock")
            .w_full()
            .px(8.0)
            .pt(10.0)
            .pb(6.0)
    } else {
        rune::el()
            .key("composer-dock")
            .w_full()
            .px(16.0)
            .pt(6.0)
            .pb(14.0)
    };
    root = root.child(dock.child(composer_card));
    if show_sheets {
        if s.settings_open {
            root = root.child(settings_sheet::<S>(s, which));
        }
        if s.notes.open {
            root = root.child(super::notes::notes_sheet::<S>(&s.notes, which));
        }
        if s.stats_open {
            root = root.child(super::token_stats::stats_sheet::<S>(
                &s.usage,
                &s.model,
                std::rc::Rc::new(move |s: &mut S| {
                    s.thread_mut(which).stats_open = false;
                }),
            ));
        }
        if s.agent_config.open {
            root = root.child(super::agent_config::agent_config_sheet::<S>(
                &s.agent_config,
                which,
            ));
        }
    }
    root
}

/// Settings sheet: dim backdrop + centred card, focus-trapped while mounted.
/// Backdrop press and Escape both dismiss (Rune `modal` machinery).
/// Settings sheet (public for the shell's global gear; the transcript dock
/// uses it too).
pub fn settings_sheet<S: TranscriptHost + 'static>(s: &RuneTranscriptState, which: Thread) -> El {
    let ink = |hex: &'static str| rune::color::hex(hex).unwrap_or(rune::Color::BLACK);
    let label = rune::tok::TextStyle::new(12.0, 500.0);
    let value = rune::tok::TextStyle::new(12.0, 400.0);
    let setting_row = |key: &str, name: &str, val: String| {
        row()
            .key(format!("settings-row-{key}"))
            .w_full()
            .items_center()
            .py(9.0)
            .child(
                text(name.to_string())
                    .key(format!("settings-{key}-label"))
                    .grow(1.0)
                    .font(label)
                    .color(ink("#a1a1aa")),
            )
            .child(
                text(val)
                    .key(format!("settings-{key}-value"))
                    .font(value)
                    .color(ink("#e4e4e7")),
            )
    };
    rune::el()
        .key("settings-overlay")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(rune::Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.6,
        })
        .center()
        .modal(true, move |s: &mut S| {
            s.thread_mut(which).settings_open = false;
        })
        .child(
            col()
                .key("settings-card")
                .w(380.0)
                .radius(16.0)
                .fill(ink("#141417"))
                .border(1.0, ink("#ffffff14"))
                .px(20.0)
                .pt(16.0)
                .pb(16.0)
                .gap(4.0)
                .child(
                    row()
                        .key("settings-head")
                        .w_full()
                        .items_center()
                        .child(
                            text("Settings".to_string())
                                .key("settings-title")
                                .grow(1.0)
                                .font(rune::tok::TextStyle::new(13.0, 700.0))
                                .color(ink("#f4f4f5")),
                        )
                        .child(
                            text("×".to_string())
                                .key("settings-close")
                                .role("button")
                                .label("Close settings")
                                .font(rune::tok::TextStyle::new(14.0, 400.0))
                                .color(ink("#8a8a93"))
                                .on_press(move |s: &mut S| {
                                    s.thread_mut(which).settings_open = false;
                                }),
                        ),
                )
                .child(setting_row("model", "Model", s.model.clone()))
                .child(setting_row("theme", "Theme", "Dark".to_string()))
                .child(setting_row(
                    "version",
                    "Version",
                    format!("stark-rune {}", env!("CARGO_PKG_VERSION")),
                ))
                .child(setting_row(
                    "backend",
                    "Backend",
                    format!("{}:{}", s.api_host, s.api_port),
                )),
        )
}

fn init_screenshot() -> RuneTranscriptState {
    RuneTranscriptState::screenshot()
}

fn apply_screenshot(s: &mut RuneTranscriptState) {
    *s = RuneTranscriptState::screenshot();
}

fn apply_stress(s: &mut RuneTranscriptState) {
    *s = RuneTranscriptState::stress();
}

fn apply_settings(s: &mut RuneTranscriptState) {
    *s = RuneTranscriptState::screenshot();
    s.settings_open = true;
}

fn apply_voice(s: &mut RuneTranscriptState) {
    *s = RuneTranscriptState::screenshot();
    // Deterministic meter proof: preset levels directly (no mic thread in a
    // headless snap; live capture is exercised by the toggle/drain tests).
    s.voice.live = true;
    s.voice.level = 0.6;
}

/// `poll` hook: drain run frames queued by the transport thread into rows.
/// Returns true when state changed (requests another frame).
pub fn drain_run_inbox(s: &mut RuneTranscriptState, _clock: f64, _tx: &mut rune::Tx) -> bool {
    let mut changed = false;
    // `Markdown.tsx`'s copy button shows the Check for 1.8s.
    if s.copied_at
        .is_some_and(|at| at.elapsed() >= std::time::Duration::from_millis(1800))
    {
        s.copied_code = None;
        s.copied_at = None;
        changed = true;
    }
    while let Some(frame) = s.inbox.as_ref().and_then(|rx| rx.try_recv().ok()) {
        crate::stream::apply_frame(s, &frame);
        changed = true;
    }
    if changed {
        // Sender gone and queue empty: run over, drop the inbox so idle frames
        // stop polling a dead channel.
        let dead = s.inbox.as_ref().is_some_and(|rx| {
            matches!(
                rx.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Disconnected)
            )
        });
        if dead {
            s.inbox = None;
        }
    }
    // Voice levels ride the same poll at 60fps steps.
    changed |= s.voice.drain(1.0 / 60.0);
    changed
}

/// AppSpec for headless `--snap` / windowed runs.
pub fn spec() -> AppSpec<RuneTranscriptState> {
    // Parity self-check (debug only): the pure window math must cover the whole
    // screenshot scene in its 720pt viewport — the Rune `.virtualize` stack and
    // `stark-kit::transcript::realize` may never disagree on the shipped scene.
    debug_assert!({
        use stark_kit::transcript::{realize, OVERSCAN};
        let rows = RuneTranscriptState::screenshot().transcript_rows();
        let w = realize(&rows, 0.0, 720.0, OVERSCAN);
        w.start == 0 && w.end == rows.len()
    });
    AppSpec {
        scenes: &[
            ("screenshot", apply_screenshot),
            ("stress", apply_stress),
            ("settings", apply_settings),
            ("voice", apply_voice),
        ],
        poll: Some(drain_run_inbox),
        fonts: &[include_bytes!("../assets/InterVariable.ttf")],
        families: ("Inter", "ui-monospace"),
        ..AppSpec::new(
            "Stark Transcript",
            (468.0, 720.0),
            init_screenshot,
            transcript_view,
        )
    }
}

/// Run headless (`--snap` renders PNG with the real wgpu renderer) or windowed
/// when no snap flag is present.
pub fn run_headless_or_windowed(extra_args: Vec<String>) {
    let app = rune::App::new(spec()).args(extra_args);
    app.run();
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn clock_shapes() {
        // The clock is local time (React `toLocaleTimeString`), so only the
        // shape is timezone-independent; the date math itself is covered by
        // `day_label_groups_a_fresh_stamp_as_today`.
        for iso in [
            "2026-11-09T14:44:00Z",
            "2026-11-09T00:05:00Z",
            "2026-11-09T12:00:00Z",
        ] {
            let value = clock(iso);
            assert_eq!(value.len(), 8, "{iso} → {value}");
            assert!(
                value.ends_with(" AM") || value.ends_with(" PM"),
                "{iso} → {value}"
            );
        }
        assert_eq!(clock(""), "");
        assert_eq!(clock("garbage"), "");
        // Naive stamps are local wall time (React `new Date()`), not UTC.
        assert_eq!(clock("2026-11-09T14:44:00"), "02:44 PM");
        assert_eq!(day_label("2026-11-01T14:44:00"), "Nov 1");
    }

    #[test]
    fn day_label_groups_a_fresh_stamp_as_today() {
        let iso = now_iso();
        assert_eq!(day_key(&iso).len(), 10);
        assert_eq!(day_label(&iso), "Today");
    }

    #[test]
    fn now_iso_roundtrips_through_agents_relative_time() {
        // Same wire shape the server emits; the agents helper parses it.
        let iso = now_iso();
        assert!(iso.len() >= 20 && iso.ends_with('Z'));
        assert_eq!(crate::agents::relative_time(&iso), "just now");
        assert_eq!(clock(&iso).len(), 8);
    }

    #[test]
    fn transcript_commits_and_rows_are_findable() {
        let env = Env::new(true, 2.0);
        let mut rt = Runtime::new(
            RuneTranscriptState::screenshot(),
            transcript_view,
            (468.0, 720.0),
            env,
        );
        rt.frame(0.0);
        for row in ["msg-0", "msg-1"] {
            assert!(rt.find(row).is_some(), "row {row} laid out");
            let presented = rt.presented(row).unwrap();
            assert!(
                presented.0[2] > 0.0 && presented.0[3] > 0.0,
                "row {row} has size"
            );
        }
        assert!(rt.find("composer-card").is_some());
        assert!(rt.find("composer-input").is_some());
        // Scene rows carry stamps, so bubble feet render the clock.
        assert!(rt.find("msg-0-time").is_some());
        // The screenshot scene is mid-run (`live: true`), so the dock shows
        // Stop, not Send (`.send-button.stop`).
        assert!(rt.find("composer-stop").is_some());
        assert!(rt.find("composer-send").is_none());
        // PromptDock toolbar: run target, usage orb, provider pill.
        for key in [
            "composer-target",
            "composer-target-local",
            "composer-target-cloud",
            "composer-usage",
            "composer-provider",
            "composer-div",
        ] {
            assert!(rt.find(key).is_some(), "{key} mounted");
        }
    }

    #[test]
    fn provider_parses_opencode_label_and_warns_without_key() {
        let v = serde_json::json!({
            "provider": "opencode", "provider_label": "x",
            "base_url": "http://x", "model_flash": "m",
            "has_key": false, "health": {},
        });
        let p = ProviderState::from_json(&v);
        // Gateway id wins over the label; missing key warns.
        assert_eq!(p.label, "OpenCode Go");
        assert!(p.warn);
        let ok = ProviderState::from_json(&serde_json::json!({
            "provider": "deepseek", "provider_label": "DeepSeek",
            "has_key": true, "health": {},
        }));
        assert_eq!(ok.label, "DeepSeek");
        assert!(!ok.warn);
        let sick = ProviderState::from_json(&serde_json::json!({
            "provider": "deepseek", "has_key": true,
            "health": {"deepseek": "boom"},
        }));
        assert!(sick.warn);
    }

    #[test]
    fn model_menu_lists_and_picks() {
        let env = Env::new(true, 1.0);
        let mut init = RuneTranscriptState::screenshot();
        init.live = false;
        init.models = vec![
            ModelOption {
                id: "m1".to_string(),
                name: "Model One".to_string(),
            },
            ModelOption {
                id: "m2".to_string(),
                name: "Model Two".to_string(),
            },
        ];
        init.models_ok = true;
        init.model = "m1".to_string();
        let mut rt = Runtime::new(init, transcript_view, (468.0, 800.0), env);
        rt.frame(0.0);
        // Provider pill + model trigger both mount (ChatView twin).
        assert!(rt.find("composer-provider").is_some());
        assert!(rt.find("composer-model-trigger").is_some());
        assert!(rt.find("composer-model-menu").is_none());
        let (x, y) = rt.find("composer-model-trigger").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.state.model_open, "trigger opens menu");
        for key in [
            "composer-model-menu",
            "composer-model-Automatic Fallback",
            "composer-model-m1",
            "composer-model-m2",
        ] {
            assert!(rt.find(key).is_some(), "{key} mounted");
        }
        // Pick switches the model and closes the menu (onModelChange).
        let (x, y) = rt.find("composer-model-m2").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(rt.state.model, "m2");
        assert!(!rt.state.model_open);
    }

    #[test]
    fn dock_target_flips_and_usage_opens() {
        let env = Env::new(true, 1.0);
        let mut init = RuneTranscriptState::screenshot();
        init.live = false;
        let mut rt = Runtime::new(init, transcript_view, (468.0, 800.0), env);
        rt.frame(0.0);
        assert_eq!(rt.state.run_target, "local");
        let (x, y) = rt.find("composer-target-cloud").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(rt.state.run_target, "cloud", "toggle flips target");
        // Usage popover opens on orb tap with zeroed rows.
        assert!(rt.find("composer-usage-popover").is_none());
        let (x, y) = rt.find("composer-usage").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.state.usage_open, "orb toggles popover");
        for key in [
            "composer-usage-popover",
            "composer-usage-cost",
            "composer-usage-tokens",
            "composer-usage-context",
            "composer-usage-cache",
        ] {
            assert!(rt.find(key).is_some(), "{key} mounted");
        }
    }

    #[test]
    fn dead_run_shows_send_again() {
        let env = Env::new(true, 1.0);
        let mut init = RuneTranscriptState::screenshot();
        init.live = false;
        let mut rt = Runtime::new(init, transcript_view, (468.0, 800.0), env);
        rt.frame(0.0);
        assert!(rt.find("composer-send").is_some());
        assert!(rt.find("composer-stop").is_none());
    }

    #[test]
    fn notes_button_opens_sheet_and_add_form() {
        let env = Env::new(true, 1.0);
        let mut init = RuneTranscriptState::screenshot();
        init.live = false;
        let mut rt = Runtime::new(init, transcript_view, (468.0, 800.0), env);
        rt.frame(0.0);
        assert!(rt.find("notes-open").is_some(), "notes button mounted");
        assert!(rt.find("notes-overlay").is_none());
        let (x, y) = rt.find("notes-open").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.state.thread(Thread::Main).notes.open, "sheet opens");
        assert!(rt.find("notes-overlay").is_some(), "overlay mounts");
        assert!(rt.find("notes-card").is_some());
        // Add form toggles (worker fails with no server — form still mounts).
        let (x, y) = rt.find("notes-add").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        for key in ["notes-form", "notes-title-input", "notes-save"] {
            assert!(rt.find(key).is_some(), "{key} mounted");
        }
    }

    #[test]
    fn typing_in_composer_edits_live_field() {
        use rune::{Key, KeyEvent, Mods};
        let env = Env::new(true, 2.0);
        let mut init = RuneTranscriptState::screenshot();
        init.edit = TextEdit::new("");
        init.edit.focused = true;
        let mut rt = Runtime::new(init, transcript_view, (468.0, 720.0), env);
        rt.frame(0.0);
        for ch in ["h", "i"] {
            rt.key(KeyEvent {
                key: Key::Text(ch.to_string()),
                mods: Mods::default(),
            });
            rt.frame(0.0);
        }
        assert_eq!(rt.state.edit.text, "hi");
    }

    #[test]
    fn enter_submits_staged_prompt() {
        use rune::{Key, KeyEvent, Mods};
        let env = Env::new(true, 2.0);
        let mut init = RuneTranscriptState::screenshot();
        init.edit = TextEdit::new("hello agent");
        init.edit.focused = true;
        init.live = false;
        init.api_port = 1;
        let n = init.rows.len();
        let mut rt = Runtime::new(init, transcript_view, (468.0, 720.0), env);
        rt.frame(0.0);
        rt.key(KeyEvent {
            key: Key::Enter,
            mods: Mods::default(),
        });
        rt.frame(0.0);
        assert_eq!(rt.state.rows.len(), n + 1);
        assert_eq!(rt.state.rows[n].body, "hello agent");
        assert!(rt.state.live);
        assert!(rt.state.edit.text.is_empty());
    }

    #[test]
    fn send_tap_stages_prompt_and_marks_live() {
        let env = Env::new(true, 2.0);
        let mut init = RuneTranscriptState::screenshot();
        init.edit = TextEdit::new("hello agent");
        init.live = false;
        // Closed port: the spawned run thread fails into the undrained channel
        // (Runtime::new wires no poll), so staging assertions stay exact and no
        // real server is ever touched.
        init.api_port = 1;
        let n = init.rows.len();
        let mut rt = Runtime::new(init, transcript_view, (468.0, 720.0), env);
        rt.frame(0.0);
        let (x, y) = rt.find("composer-send").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert_eq!(rt.state.rows.len(), n + 1);
        assert_eq!(rt.state.rows[n].role, "user");
        assert_eq!(rt.state.rows[n].body, "hello agent");
        assert!(rt.state.live);
        assert!(rt.state.edit.text.is_empty());
        assert!(rt.find(&rt.state.rows[n].key).is_some());
        assert!(rt.state.inbox.is_some(), "run thread owns the inbox");
    }

    #[test]
    fn poll_drain_applies_queued_frames_and_drops_dead_inbox() {
        use crate::stream::SseFrame;
        let mut s = RuneTranscriptState::screenshot();
        let n = s.rows.len();
        let (tx, rx) = std::sync::mpsc::channel();
        s.inbox = Some(rx);
        tx.send(SseFrame {
            event: "delta".to_string(),
            data: serde_json::json!({"kind":"text","text":"hi"}),
        })
        .unwrap();
        tx.send(SseFrame {
            event: "done".to_string(),
            data: serde_json::json!({"status":"ok"}),
        })
        .unwrap();
        drop(tx);
        let mut run_tx = rune::Tx::default();
        assert!(drain_run_inbox(&mut s, 0.0, &mut run_tx));
        assert_eq!(s.rows.len(), n + 1);
        assert_eq!(s.rows.last().unwrap().body, "hi");
        assert!(!s.live);
        assert!(s.inbox.is_none(), "dead inbox dropped");
        assert!(!drain_run_inbox(&mut s, 1.0, &mut run_tx));
    }

    #[test]
    fn settings_opens_closes_and_traps_background() {
        use rune::{Key, KeyEvent, Mods};
        let env = Env::new(true, 2.0);
        // Dead run: the dock shows Send, so the backdrop-tap half of this
        // test exercises the send path, not Stop.
        let mut init = RuneTranscriptState::screenshot();
        init.live = false;
        let mut rt = Runtime::new(init, transcript_view, (468.0, 720.0), env);
        rt.frame(0.0);
        assert!(rt.find("settings-card").is_none());
        // Open from the title bar.
        let (x, y) = rt.find("settings-open").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert!(rt.state.settings_open);
        for key in ["settings-card", "settings-title", "settings-close"] {
            assert!(rt.find(key).is_some(), "{key} mounted");
        }
        // Background is inert: tapping send dismisses the sheet (backdrop)
        // instead of staging a row.
        let n = rt.state.rows.len();
        let (sx, sy) = rt.find("composer-send").unwrap();
        rt.pointer_down(sx, sy);
        rt.pointer_up(sx, sy);
        rt.frame(0.0);
        assert!(!rt.state.settings_open, "backdrop press dismisses");
        assert_eq!(rt.state.rows.len(), n, "no row staged behind the modal");
        // Reopen, then Escape dismisses via the modal machinery.
        let (x, y) = rt.find("settings-open").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert!(rt.state.settings_open);
        rt.key(KeyEvent {
            key: Key::Escape,
            mods: Mods::default(),
        });
        rt.frame(0.0);
        assert!(!rt.state.settings_open, "Escape dismisses");
    }

    #[test]
    fn voice_toggle_flips_live_dot() {
        let env = Env::new(true, 2.0);
        let mut rt = Runtime::new(
            RuneTranscriptState::screenshot(),
            transcript_view,
            (468.0, 720.0),
            env,
        );
        rt.frame(0.0);
        assert!(!rt.state.voice.live);
        let (x, y) = rt.find("composer-voice").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        // Outcome depends on hardware (mic vs none), so assert the invariant:
        // either live capture staged, or an honest note explains why not.
        // (No poll in `Runtime::new`, so nothing races the assertion.)
        assert!(
            rt.state.voice.live || rt.state.voice.note.is_some(),
            "voice state: {:?}",
            rt.state.voice
        );
        assert!(rt.find("composer-voice-dot").is_some());
        let (x, y) = rt.find("composer-voice").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert!(!rt.state.voice.live);
        assert!(rt.state.voice.stop.is_none());
    }

    #[test]
    fn voice_drain_applies_levels_and_errors() {
        use crate::voice::{VoiceBus, VoiceMsg};
        let mut bus = VoiceBus::default();
        let (tx, rx) = std::sync::mpsc::channel();
        bus.rx = Some(rx);
        bus.live = true;
        tx.send(VoiceMsg::Level(0.6)).unwrap();
        assert!(bus.drain(0.016));
        // Fresh peak attacked to 0.6, then one idle-decay step (0.016s × 1.5).
        assert!((bus.level - 0.576).abs() < 1e-4, "{}", bus.level);
        assert_eq!(crate::voice::lit_bars(bus.level), 3);
        tx.send(VoiceMsg::Error("no microphone found".to_string()))
            .unwrap();
        drop(tx);
        assert!(bus.drain(0.016));
        assert!(!bus.live);
        assert_eq!(bus.note.as_deref(), Some("no microphone found"));
        assert_eq!(bus.level, 0.0);
    }

    #[test]
    fn markdown_code_block_mounts_in_row() {
        let env = Env::new(true, 2.0);
        let mut rt = Runtime::new(
            RuneTranscriptState::screenshot(),
            transcript_view,
            (468.0, 720.0),
            env,
        );
        rt.frame(0.0);
        for key in ["msg-1-b0-p", "msg-1-b1-code", "msg-1-b1-code-body"] {
            assert!(rt.find(key).is_some(), "{key} mounted");
        }
    }

    #[test]
    fn stress_frame_mounts_tail_window_only() {
        use stark_kit::transcript::{realize, OVERSCAN};
        let s = RuneTranscriptState::stress();
        let rows = s.transcript_rows();
        assert_eq!(rows.len(), 10_000);
        // stick_bottom opens at the end: the pure window at the bottom offset
        // must be a small tail window, agreeing with Rune's mounted window.
        let total: f64 = rows.iter().map(|r| r.height() as f64).sum();
        let w = realize(&rows, total, 900.0, OVERSCAN);
        assert_eq!(w.end, 10_000);
        assert!(w.end - w.start < 60, "overscanned tail only: {w:?}");
        let env = Env::new(true, 2.0);
        let mut rt = Runtime::new(s, transcript_view, (468.0, 900.0), env);
        rt.frame(0.0);
        assert!(rt.find("msg-9999").is_some(), "tail row mounted");
        assert!(rt.find("msg-0").is_none(), "head row virtualized out");
        assert!(rt.find("composer-send").is_some());
    }
}
