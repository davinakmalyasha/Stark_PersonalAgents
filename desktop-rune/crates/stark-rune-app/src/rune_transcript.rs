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

use crate::chats::{ChatAttachment, ChatMsgRow};
use crate::stream_accum::{
    RetryNotice, RunStatusNotice, RunUsage, StreamAccum, StreamSegment, TraceStep,
};

/// Result of the blocking `/rewind` + `GET chat` work: the restored prompt
/// and the canonical truncated history. The clicked id is kept so the UI can
/// apply React's defensive slice even though the server already truncated.
#[derive(Debug, Clone)]
pub struct RewindOk {
    pub row_id: String,
    pub prompt: String,
    /// The clicked user message's `metadata.attachments`; React restores the
    /// draft deck from the clicked message, not from the rewind response.
    pub attachments: Vec<ChatAttachment>,
    pub rows: Vec<ChatMsgRow>,
}

#[path = "rune_transcript/activity.rs"]
mod activity;
#[path = "rune_transcript/md_render.rs"]
mod md_render;

/// Row content fed to both the pure window math and the Rune view.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RowData {
    pub key: String,
    pub role: &'static str,
    pub body: String,
    /// Persisted `content` before `presentStream` moved text into the
    /// timeline (MessageStream dedupe/rail read the raw message).
    pub raw_body: String,
    /// ISO-8601 creation stamp (`CanvasChatBubble clock()` renders it as
    /// `hh:MM AM/PM` in the bubble foot). Empty for scene fixtures.
    pub at: String,
    /// `metadata.attachments` for user rows: image previews + file chips
    /// inside the bubble (`MessageBubble`'s `.message-attachments`).
    pub attachments: Vec<ChatAttachment>,
    /// `metadata.duration` for the "Worked for …" trace divider.
    pub duration: String,
    pub show_trace: bool,
    pub hide_body: bool,
    /// `metadata.kind` for `SessionEventDivider`: `"aborted"` or `"compaction"`.
    pub event: String,
    /// Compaction token metadata (`SessionEventDivider`).
    pub event_tokens_before: u64,
    pub event_input: u64,
    pub event_output: u64,
    /// `metadata.usage` present: React shows the token/time footer only then.
    pub has_usage: bool,
    /// `metadata.usage.total_input_tokens` / `total_output_tokens`.
    pub tokens_in: u64,
    pub tokens_out: u64,
    /// Live agent name (`streaming.agent || getAssistantName()`).
    pub agent_name: String,
    /// The row is the active run: timeline inline, no completed trace shell.
    pub is_streaming: bool,
    /// `metadata.stopped` — "Stopped — partial response kept".
    pub stopped: bool,
    /// Persisted `metadata.liveTimeline`, or the live accumulator's segments.
    pub timeline: Vec<StreamSegment>,
    /// `presentStream(...).activitySegments` cached for persisted rows.
    pub activity: Vec<StreamSegment>,
    /// `presentStream(...).finalText` cached for persisted rows.
    pub final_text: String,
    /// `metadata.thinking`, used as a reasoning segment when no timeline exists.
    pub thinking: String,
    /// Legacy `metadata.steps`.
    pub steps: Vec<TraceStep>,
    /// Completed "Worked for …" accordion open.
    pub trace_open: bool,
    /// Per-tool-group open state. Absence means React's default.
    pub tool_open: std::collections::HashMap<String, bool>,
    /// Live fold owned by the row (never duplicated in app state).
    pub accum: Option<StreamAccum>,
    /// Ephemeral reconnect notice while streaming.
    pub retry: Option<RetryNotice>,
    /// Compaction / context notice while streaming.
    pub live_status: Option<RunStatusNotice>,
    /// Current run's tokens, persisted into `metadata.usage`.
    pub run_usage: Option<RunUsage>,
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
    /// Real workspace id (`_general`, `_personal`, or a project id).
    pub workspace_id: String,
    /// `chat_id` names a chat that exists server-side. Canvas-opened chats
    /// start true; the home thread creates one before its first send.
    pub chat_ready: bool,
    /// Monotonic start of the active run (duration uses `Math.round(ms/1000)`).
    pub run_started_ms: Option<f64>,
    /// Abort requested: deltas are ignored until the terminal frame.
    pub aborting: bool,
    /// Completed/partial messages waiting for the ChatStore writer.
    pub pending_saves: Vec<crate::chats::ChatSave>,
    /// Refresh this chat after an aborted turn settles.
    pub refresh_chat: Option<(String, String)>,
    /// Jump-to-latest pill opacity (`on_scroll_offset` writes it live).
    pub scroll_pill: rune::Live<f32>,
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
    /// Row keys whose copy button is showing React's 1.5s Check state.
    pub copied_messages: std::collections::HashMap<String, std::time::Instant>,
    /// User rows armed for the two-step rewind (3.2s, like `rewindArmed`).
    pub rewind_armed: std::collections::HashMap<String, std::time::Instant>,
    /// Blocking `/rewind` + history fetch running for this thread.
    pub rewind_inbox: Option<std::sync::mpsc::Receiver<Result<RewindOk, String>>>,
    /// Composer deck: local picks (`id` empty) and restored uploads.
    pub draft_attachments: Vec<ChatAttachment>,
    /// Amber line under the input (`20 MB`/`8 files`/picker failures).
    pub attachments_error: Option<String>,
    /// A file drag hovers the dock (`.prompt-dock.is-dragging`).
    pub attachments_dragging: bool,
    /// Full-viewport image preview (`message-lightbox`).
    pub attachment_preview: Option<ChatAttachment>,
    /// Ask the shell to refresh the sidebar after create/rewind.
    pub refresh_list: bool,
    /// `chat` SSE frame landed: the shell rekeys a `chat:local:*` card to the
    /// server id (`workspace`, `chat`).
    pub pending_rekey: Option<(String, String)>,
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
            .field("workspace_id", &self.workspace_id)
            .field("chat_ready", &self.chat_ready)
            .field("aborting", &self.aborting)
            .field("notes", &self.notes.notes.len())
            .field("run_target", &self.run_target)
            .field("usage", &self.usage)
            .field("models", &self.models.len())
            .field("settings_open", &self.settings_open)
            .field("voice", &self.voice)
            .field("inbox", &self.inbox.is_some())
            .field("copied_code", &self.copied_code)
            .field("expanded_diffs", &self.expanded_diffs.len())
            .field("copied_messages", &self.copied_messages.len())
            .field("rewind_armed", &self.rewind_armed.len())
            .field("rewind_inbox", &self.rewind_inbox.is_some())
            .field("draft_attachments", &self.draft_attachments.len())
            .field("attachment_preview", &self.attachment_preview.is_some())
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
            workspace_id: self.workspace_id.clone(),
            chat_ready: self.chat_ready,
            run_started_ms: self.run_started_ms,
            aborting: self.aborting,
            pending_saves: Vec::new(),
            refresh_chat: self.refresh_chat.clone(),
            scroll_pill: self.scroll_pill.clone(),
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
            copied_messages: self.copied_messages.clone(),
            rewind_armed: self.rewind_armed.clone(),
            rewind_inbox: None,
            draft_attachments: self.draft_attachments.clone(),
            attachments_error: self.attachments_error.clone(),
            attachments_dragging: false,
            attachment_preview: None,
            refresh_list: self.refresh_list,
            pending_rekey: self.pending_rekey.clone(),
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
            workspace_id: "_general".to_string(),
            chat_ready: false,
            run_started_ms: None,
            aborting: false,
            pending_saves: Vec::new(),
            refresh_chat: None,
            scroll_pill: rune::Live::new(0.0),
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
            copied_messages: std::collections::HashMap::new(),
            rewind_armed: std::collections::HashMap::new(),
            rewind_inbox: None,
            draft_attachments: Vec::new(),
            attachments_error: None,
            attachments_dragging: false,
            attachment_preview: None,
            refresh_list: false,
            pending_rekey: None,
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
            workspace_id: "_general".to_string(),
            chat_ready: false,
            run_started_ms: None,
            aborting: false,
            pending_saves: Vec::new(),
            refresh_chat: None,
            scroll_pill: rune::Live::new(0.0),
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
            copied_messages: std::collections::HashMap::new(),
            rewind_armed: std::collections::HashMap::new(),
            rewind_inbox: None,
            draft_attachments: Vec::new(),
            attachments_error: None,
            attachments_dragging: false,
            attachment_preview: None,
            refresh_list: false,
            pending_rekey: None,
        }
    }

    /// Parity scene: a live run mid-tool (reasoning + running/finished tool +
    /// answer-status note), so the activity timeline is screenshot-testable
    /// without a server.
    pub fn stream_demo() -> Self {
        let mut state = Self::screenshot();
        state.title = "Stream".to_string();
        state.live = true;
        state.chat_id = "stream-demo".to_string();
        state.chat_ready = true;
        let mut accum = StreamAccum {
            agent: "Orchestrator".to_string(),
            ..StreamAccum::default()
        };
        let mut rate = crate::stream_accum::RateAccum::default();
        accum.apply(
            &mut rate,
            &crate::stream_accum::Delta {
                kind: "reasoning".to_string(),
                text: "I should inspect the current player state first.".to_string(),
                agent: Some("Orchestrator".to_string()),
                ..crate::stream_accum::Delta::default()
            },
            1,
        );
        accum.apply(
            &mut rate,
            &crate::stream_accum::Delta {
                kind: "tool_started".to_string(),
                tool: Some("grep".to_string()),
                args: Some(r#"{"pattern":"Cinnamon Girl","path":"music/"}"#.to_string()),
                agent: Some("Orchestrator".to_string()),
                ..crate::stream_accum::Delta::default()
            },
            2,
        );
        accum.apply(
            &mut rate,
            &crate::stream_accum::Delta {
                kind: "tool_finished".to_string(),
                tool: Some("grep".to_string()),
                ok: Some(true),
                ms: Some(42),
                text: "music/player.rs:88: Cinnamon Girl".to_string(),
                agent: Some("Orchestrator".to_string()),
                ..crate::stream_accum::Delta::default()
            },
            3,
        );
        accum.apply(
            &mut rate,
            &crate::stream_accum::Delta {
                kind: "text".to_string(),
                text: "Playing **Cinnamon Girl** in your YouTube tab now — the track is at 04:48."
                    .to_string(),
                agent: Some("Orchestrator".to_string()),
                ..crate::stream_accum::Delta::default()
            },
            4,
        );
        state.rows = vec![
            RowData {
                key: "stream-user".to_string(),
                role: "user",
                body: "play that in the current youtube tab".to_string(),
                at: "2026-11-12T12:00:00Z".to_string(),
                ..RowData::default()
            },
            RowData {
                key: "stream-live".to_string(),
                role: "agent",
                at: "2026-11-12T12:00:04Z".to_string(),
                duration: "active".to_string(),
                agent_name: "Orchestrator".to_string(),
                is_streaming: true,
                accum: Some(accum),
                ..RowData::default()
            },
        ];
        state
    }

    /// Parity scene: a completed row with the trace accordion open.
    pub fn trace_demo() -> Self {
        let mut state = Self::screenshot();
        state.title = "Trace".to_string();
        state.live = false;
        let reasoning = StreamSegment::Reasoning {
            text: "I checked the player state before answering.".to_string(),
            agent: Some("Orchestrator".to_string()),
        };
        let tool = StreamSegment::Tool {
            ev: crate::stream_accum::LiveToolEvent {
                id: "trace-tool".to_string(),
                name: "grep".to_string(),
                args: Some(r#"{"pattern":"Cinnamon Girl","path":"music/"}"#.to_string()),
                status: crate::stream_accum::ToolStatus::Done,
                ok: Some(true),
                ms: Some(42),
                preview: Some("music/player.rs:88: Cinnamon Girl".to_string()),
                agent: Some("Orchestrator".to_string()),
                children: Vec::new(),
            },
        };
        let answer = "Playing **Cinnamon Girl** in your YouTube tab now — the track is at 04:48.";
        state.rows = vec![
            RowData {
                key: "trace-user".to_string(),
                role: "user",
                body: "play that in the current youtube tab".to_string(),
                at: "2026-11-12T12:00:00Z".to_string(),
                ..RowData::default()
            },
            RowData {
                key: "trace-agent".to_string(),
                role: "agent",
                body: answer.to_string(),
                at: "2026-11-12T12:00:08Z".to_string(),
                duration: "8s".to_string(),
                agent_name: "Orchestrator".to_string(),
                show_trace: true,
                trace_open: true,
                timeline: vec![reasoning.clone(), tool.clone()],
                activity: vec![reasoning, tool],
                final_text: answer.to_string(),
                has_usage: true,
                tokens_in: 1240,
                tokens_out: 86,
                run_usage: Some(RunUsage {
                    input: 1240,
                    output: 86,
                    ..RunUsage::default()
                }),
                ..RowData::default()
            },
        ];
        state
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

/// Standalone window size (the shell has its own static): keeps the
/// full-viewport lightbox at React's 92vw/86vh without threading dimensions
/// through every transcript builder.
static mut TRANSCRIPT_WINDOW: (f32, f32) = (468.0, 720.0);

fn transcript_window() -> (f32, f32) {
    // SAFETY: views run single-threaded on the UI thread.
    unsafe { TRANSCRIPT_WINDOW }
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

/// PromptDock's file limits (`MAX_FILES` / `MAX_FILE_SIZE`).
pub const MAX_ATTACHMENTS: usize = 8;
pub const MAX_ATTACHMENT_BYTES: u64 = 20 * 1024 * 1024;

/// Shared send path (button + Enter): stage the field as a user row, mark the
/// run live, and spawn the blocking `POST /api/run` thread into `inbox`.
/// Attachments count as content, so an attachment-only send is allowed; the
/// text is only cleared once staging succeeded (React's guard keeps a live
/// run's draft in place).
pub fn stage_send(s: &mut RuneTranscriptState) {
    let prompt = s.edit.text.trim().to_string();
    if (prompt.is_empty() && s.draft_attachments.is_empty()) || s.live || s.aborting {
        return;
    }
    let attachments = s.draft_attachments.clone();
    if stage_send_prompt_with_attachments(s, prompt, attachments) {
        s.edit.text.clear();
        s.edit.anchor = 0;
        s.edit.focus = 0;
        // React's PromptDock clears both dock lists after `handleSend`.
        s.draft_attachments.clear();
        s.attachments_error = None;
    }
}

/// One picked path becomes a local draft card (`id` empty until upload).
/// React's `addFiles`: drop >20 MB files, dedupe by name+size, cap at eight,
/// and report the matching amber line.
pub fn add_attachment_paths(s: &mut RuneTranscriptState, paths: &[std::path::PathBuf]) {
    if paths.is_empty() {
        return;
    }
    let mut valid = Vec::new();
    for path in paths {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if size > MAX_ATTACHMENT_BYTES {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let mime = crate::chats::mime_for_path(path);
        valid.push(ChatAttachment {
            id: String::new(),
            name: name.to_string(),
            file_type: mime.to_string(),
            size,
            path: path.to_string_lossy().into_owned(),
            is_image: mime.starts_with("image/"),
            url: None,
        });
    }
    let size_rejected = valid.len() != paths.len();
    let mut dropped = false;
    for file in valid {
        if s.draft_attachments
            .iter()
            .any(|item| item.id.is_empty() && item.name == file.name && item.size == file.size)
        {
            dropped = true;
            continue;
        }
        s.draft_attachments.push(file);
    }
    if s.draft_attachments.len() > MAX_ATTACHMENTS {
        let extra = s.draft_attachments.len() - MAX_ATTACHMENTS;
        s.draft_attachments.truncate(MAX_ATTACHMENTS);
        dropped |= extra > 0;
    }
    if size_rejected {
        s.attachments_error = Some("Files must be 20 MB or smaller.".to_string());
    } else if dropped {
        s.attachments_error = Some(format!(
            "You can attach up to {MAX_ATTACHMENTS} files at once."
        ));
    } else if !s.draft_attachments.is_empty() {
        s.attachments_error = None;
    }
}

/// Remove one deck card. Local picks have no id yet (the composer passes
/// `local:<index>`); restored uploads pass their id. Clearing back under the
/// cap also drops React's "up to 8" line.
pub fn remove_draft_attachment(s: &mut RuneTranscriptState, target: &str) {
    if let Some(index) = target
        .strip_prefix("local:")
        .and_then(|value| value.parse::<usize>().ok())
    {
        if index < s.draft_attachments.len() {
            s.draft_attachments.remove(index);
        }
    } else {
        s.draft_attachments.retain(|file| file.id != target);
    }
    if s.draft_attachments.len() < MAX_ATTACHMENTS
        && s.attachments_error
            .as_deref()
            .is_some_and(|error| error.contains("up to 8"))
    {
        s.attachments_error = None;
    }
}

/// The dock's attach button / dashed tile: one native file dialog per pick
/// (Rune selects a single path; drag-and-drop is the multi-file path).
pub fn open_attachment_picker<S: TranscriptHost + 'static>(
    _s: &mut S,
    which: Thread,
    tx: &mut rune::Tx,
) {
    let options = rune::FileDialogOptions {
        title: "Attach files".to_string(),
        ..Default::default()
    };
    tx.open_file(options, move |s: &mut S, result, _tx| match result {
        Ok(Some(path)) => add_attachment_paths(s.thread_mut(which), &[path]),
        Ok(None) => {}
        Err(error) => {
            s.thread_mut(which).attachments_error = Some(error.to_string());
        }
    });
}

/// Drop files anywhere on the composer card (multi-file, unlike the picker).
pub fn drop_attachment_files(s: &mut RuneTranscriptState, files: &[std::path::PathBuf]) {
    add_attachment_paths(s, files);
    s.attachments_dragging = false;
}

/// React `chat.title` for a fresh conversation: first 32 chars + ellipsis.
fn chat_title(prompt: &str) -> String {
    let trimmed = prompt.trim();
    let head: String = trimmed.chars().take(32).collect();
    let head = if head.is_empty() {
        "New Conversation".to_string()
    } else {
        head
    };
    if trimmed.chars().count() > 32 {
        format!("{head}...")
    } else {
        head
    }
}

impl RuneTranscriptState {
    /// `workspaceKind` sent with `/api/run` (deterministic routing).
    pub fn workspace_kind(&self) -> String {
        match self.workspace_id.as_str() {
            "_house" => "house".to_string(),
            "_personal" => "personal".to_string(),
            _ => "general".to_string(),
        }
    }
}

/// Stage an explicit prompt (map dock sends land here with the dock's text).
pub fn stage_send_prompt(s: &mut RuneTranscriptState, prompt: String) {
    let _ = stage_send_prompt_with_attachments(s, prompt, Vec::new());
}

/// Stage a send with its dock attachments. Restored files already have ids;
/// local picks are uploaded synchronously first (after creating the chat, so
/// the upload has somewhere to land). A failure leaves the draft intact and
/// reports through the dock's amber line, exactly where the send came from.
pub fn stage_send_prompt_with_attachments(
    s: &mut RuneTranscriptState,
    prompt: String,
    attachments: Vec<ChatAttachment>,
) -> bool {
    let prompt = prompt.trim().to_string();
    if (prompt.is_empty() && attachments.is_empty()) || s.live || s.aborting {
        return false;
    }
    // React keeps the two dock lists separate: restored files are sent
    // as-is, picked files upload now, and the wire order is restored first.
    #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
    let mut send_attachments: Vec<ChatAttachment> = attachments
        .iter()
        .filter(|file| !file.id.is_empty())
        .cloned()
        .collect();
    let locals: Vec<std::path::PathBuf> = attachments
        .iter()
        .filter(|file| file.id.is_empty())
        .map(|file| std::path::PathBuf::from(&file.path))
        .collect();
    if !locals.is_empty() {
        #[cfg(not(target_arch = "wasm32"))]
        {
            // `handleSend` creates the conversation before uploading.
            if !s.chat_ready || s.chat_id.trim().is_empty() {
                match crate::transport::create_chat(
                    &s.api_host,
                    s.api_port,
                    &s.workspace_id,
                    &chat_title(&prompt),
                ) {
                    Ok(id) => {
                        s.chat_id = id.clone();
                        s.chat_ready = true;
                        s.pending_rekey = Some((s.workspace_id.clone(), id));
                        s.refresh_list = true;
                    }
                    Err(_) => {
                        s.attachments_error = Some("Could not create a conversation.".to_string());
                        return false;
                    }
                }
            }
            match crate::transport::upload_attachments(
                &s.api_host,
                s.api_port,
                &s.workspace_id,
                &s.chat_id,
                &locals,
            ) {
                Ok(value) => {
                    let uploaded: Vec<ChatAttachment> = value
                        .get("attachments")
                        .and_then(|v| v.as_array())
                        .map(|files| files.iter().map(ChatAttachment::from_json).collect())
                        .unwrap_or_default();
                    if uploaded.is_empty() {
                        s.attachments_error = Some("Your files could not be uploaded.".to_string());
                        return false;
                    }
                    send_attachments.extend(uploaded);
                }
                Err(_) => {
                    s.attachments_error = Some("Your files could not be uploaded.".to_string());
                    return false;
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            s.attachments_error = Some("File uploads need the desktop app.".to_string());
            return false;
        }
    }
    let file_count = send_attachments.len();
    let content = if prompt.is_empty() {
        format!(
            "Attached {file_count} file{}",
            if file_count > 1 { "s" } else { "" }
        )
    } else {
        prompt.clone()
    };
    let n = s.rows.len();
    s.rows.push(RowData {
        key: format!("user-{n}"),
        role: "user",
        body: content.clone(),
        raw_body: content,
        at: now_iso(),
        attachments: send_attachments.clone(),
        ..RowData::default()
    });
    // Show the live bubble immediately (React appends `house-live` on send,
    // before the first delta) with the same Thinking placeholder.
    s.rows.push(RowData {
        key: crate::stream::LIVE_ROW_KEY.to_string(),
        role: "agent",
        at: now_iso(),
        duration: "active".to_string(),
        agent_name: "Stark".to_string(),
        is_streaming: true,
        accum: Some(StreamAccum::default()),
        ..RowData::default()
    });
    s.live = true;
    s.aborting = false;
    s.run_started_ms = Some(crate::clock::monotonic_millis());
    // The abort path needs the same id the run started with.
    if s.chat_id.trim().is_empty() {
        s.chat_id = crate::notes::mint_id("rune");
        s.chat_ready = false;
    }
    let req = crate::stream::RunRequest {
        prompt,
        model: s.model.clone(),
        temperature: 0.7,
        workspace_path: String::new(),
        workspace_id: s.workspace_id.clone(),
        chat_id: s.chat_id.clone(),
        workspace_kind: s.workspace_kind(),
        attachments: send_attachments,
    };
    let workspace_id = s.workspace_id.clone();
    let title = chat_title(&req.prompt);
    let chat_id = s.chat_id.clone();
    let ready = s.chat_ready && !chat_id.trim().is_empty();
    let (tx, rx) = std::sync::mpsc::channel();
    s.inbox = Some(rx);
    let (host, port) = (s.api_host.clone(), s.api_port);
    // Browsers have no TCP sockets: drive the run over fetch + ReadableStream
    // into the same inbox the poll hook drains. Same rows, same framing.
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let mut done = false;
        let mut emit = |frame: crate::stream::SseFrame| {
            if frame.event == "done" {
                done = true;
            }
            let _ = tx.send(frame);
        };
        let mut chat_id = chat_id;
        if !ready {
            match crate::transport::create_chat_wasm(&workspace_id, &title).await {
                Ok(id) => {
                    let _ = tx.send(crate::stream::SseFrame {
                        event: "chat".to_string(),
                        data: serde_json::json!({"id": id, "workspace_id": workspace_id}),
                    });
                    chat_id = id;
                }
                Err(message) => {
                    let _ = tx.send(crate::stream::SseFrame {
                        event: "error".to_string(),
                        data: serde_json::json!({"message": message}),
                    });
                    return;
                }
            }
        }
        let req = crate::stream::RunRequest { chat_id, ..req };
        crate::transport::post_run_stream(&req, &mut emit).await;
        if !done {
            let _ = tx.send(crate::stream::SseFrame {
                event: "done".to_string(),
                data: serde_json::json!({"status": "complete"}),
            });
        }
        let _ = (host, port);
    });
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let mut chat_id = chat_id;
        if !ready {
            match crate::transport::create_chat(&host, port, &workspace_id, &title) {
                Ok(id) => {
                    let _ = tx.send(crate::stream::SseFrame {
                        event: "chat".to_string(),
                        data: serde_json::json!({"id": id, "workspace_id": workspace_id}),
                    });
                    chat_id = id;
                }
                Err(message) => {
                    let _ = tx.send(crate::stream::SseFrame {
                        event: "error".to_string(),
                        data: serde_json::json!({"message": message}),
                    });
                    return;
                }
            }
        }
        let req = crate::stream::RunRequest { chat_id, ..req };
        let mut done = false;
        let mut emit = |frame: crate::stream::SseFrame| {
            if frame.event == "done" {
                done = true;
            }
            let _ = tx.send(frame);
        };
        match crate::transport::post_run(&host, port, &req, &mut emit) {
            Ok(_) if !done => {
                let _ = tx.send(crate::stream::SseFrame {
                    event: "done".to_string(),
                    data: serde_json::json!({"status": "complete"}),
                });
            }
            Err(message) => {
                let _ = tx.send(crate::stream::SseFrame {
                    event: "error".to_string(),
                    data: serde_json::json!({"message": message}),
                });
            }
            _ => {}
        }
    });
    true
}

fn push_error_row(s: &mut RuneTranscriptState, message: &str) -> String {
    let key = format!("error-{}-{}", crate::clock::now_millis(), s.rows.len());
    s.rows.push(RowData {
        key: key.clone(),
        role: "error",
        body: message.to_string(),
        at: now_iso(),
        ..RowData::default()
    });
    key
}

fn queue_save(
    s: &mut RuneTranscriptState,
    role: &str,
    content: String,
    metadata: serde_json::Value,
) {
    if s.workspace_id.trim().is_empty() || s.chat_id.trim().is_empty() {
        return;
    }
    s.pending_saves.push(crate::chats::ChatSave {
        workspace_id: s.workspace_id.clone(),
        chat_id: s.chat_id.clone(),
        role: role.to_string(),
        content,
        metadata,
    });
}

/// Freeze the live row into a normal agent row and queue its persistence.
///
/// - `error`: an inline error follows the partial response (React's `onError`
///   path persists the partial first, then the error).
/// - `stopped`: Stop button, `metadata.stopped = true`.
///
/// Returns true when an assistant save was queued.
pub fn finalize_run(s: &mut RuneTranscriptState, error: Option<String>, stopped: bool) -> bool {
    let Some(index) = s
        .rows
        .iter()
        .rposition(|row| row.accum.is_some() && row.is_streaming)
    else {
        if let Some(message) = error {
            push_error_row(s, &message);
            queue_save(s, "error", message, serde_json::Value::Null);
        }
        s.live = false;
        s.run_started_ms = None;
        return false;
    };
    let Some(accum) = s.rows[index].accum.take() else {
        return false;
    };
    let has_partial = !accum.text.trim().is_empty()
        || !accum.reasoning.trim().is_empty()
        || !accum.segments.is_empty()
        || !accum.report.trim().is_empty();
    let is_error = error.is_some();
    let stopped = stopped || is_error;
    let content = if !accum.text.trim().is_empty() {
        accum.text.trim().to_string()
    } else {
        accum.report.trim().to_string()
    };
    let presentation =
        crate::stream_presentation::present_stream(&accum.segments, &content, false, &[]);
    let final_text = presentation.final_text;
    let activity = presentation.activity;
    let duration = s.run_started_ms.and_then(|start| {
        let secs = ((crate::clock::monotonic_millis() - start).max(0.0) / 1000.0).round() as u64;
        (secs > 0).then(|| format!("{secs}s"))
    });
    let run_usage = accum.usage.clone();
    let tokens_in = run_usage.input;
    let tokens_out = run_usage.output;
    let has_usage = accum.usage_json.is_some()
        || run_usage.input > 0
        || run_usage.output > 0
        || run_usage.cache_hit > 0
        || run_usage.cost > 0.0;

    // An error with no partial output removes the empty placeholder; React's
    // live bubble disappears and only the error row remains.
    if is_error && !has_partial {
        s.rows.remove(index);
    } else if stopped && !has_partial {
        // Stop before the first token: no partial message is persisted.
        s.rows.remove(index);
    } else {
        let row = &mut s.rows[index];
        row.body = final_text.clone();
        row.raw_body = content.clone();
        row.final_text = final_text.clone();
        row.timeline = accum.segments.clone();
        row.activity = activity.clone();
        row.show_trace = !activity.is_empty();
        row.hide_body = final_text.trim().is_empty();
        row.duration = duration.clone().unwrap_or_default();
        row.at = now_iso();
        row.agent_name = if accum.agent.is_empty() {
            row.agent_name.clone()
        } else {
            accum.agent.clone()
        };
        row.is_streaming = false;
        row.stopped = stopped;
        row.has_usage = has_usage;
        row.tokens_in = tokens_in;
        row.tokens_out = tokens_out;
        row.run_usage = Some(run_usage.clone());
        row.retry = None;
        row.live_status = None;
        row.accum = None;
    }

    let mut queued = false;
    if !is_error || has_partial {
        let mut metadata = serde_json::json!({
            "isTrace": true,
            "steps": [],
        });
        if has_usage {
            metadata["usage"] = accum
                .usage_json
                .clone()
                .unwrap_or_else(|| run_usage.to_json());
        }
        if let Some(duration) = &duration {
            metadata["duration"] = serde_json::json!(duration);
        }
        if !accum.reasoning.trim().is_empty() {
            metadata["thinking"] = serde_json::json!(accum.reasoning);
        }
        if !accum.segments.is_empty() {
            metadata["liveTimeline"] = serde_json::Value::Array(
                accum
                    .segments
                    .iter()
                    .map(crate::stream_accum::segment_to_json)
                    .collect(),
            );
        }
        if stopped {
            metadata["stopped"] = serde_json::json!(true);
        }
        if let Some(artifacts) = &accum.artifacts {
            metadata["artifacts"] = artifacts.clone();
        }
        queue_save(s, "agent", content, metadata);
        queued = true;
    }
    if let Some(message) = error {
        push_error_row(s, &message);
        queue_save(s, "error", message, serde_json::Value::Null);
    }
    s.live = false;
    s.run_started_ms = None;
    queued
}

/// Shared stop path (composer Stop button): persist the partial immediately
/// (React's `handleCancel`), then fire-and-forget `POST /api/abort`.
pub fn stage_abort(s: &mut RuneTranscriptState) {
    if !s.live || s.aborting {
        return;
    }
    let _ = finalize_run(s, None, true);
    s.aborting = true;
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

pub fn transcript_view(s: &RuneTranscriptState, cx: &mut Cx<RuneTranscriptState>) -> El {
    let (w, h) = cx
        .window_layout("stark-transcript")
        .map(|r| (r[2], r[3]))
        .filter(|(w, h)| *w > 1.0 && *h > 1.0)
        .unwrap_or((468.0, 720.0));
    // SAFETY: views run single-threaded on the UI thread.
    unsafe {
        TRANSCRIPT_WINDOW = (w, h);
    }
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
    /// `retryMap`: the last user prompt at or before each row. React builds it
    /// over the deduped list, so hidden internal/duplicate rows never leak a
    /// prompt. Empty prompt is a valid value (the retry button is still shown).
    retry_prompts: Vec<String>,
}

fn raw_of(row: &RowData) -> &str {
    if row.raw_body.is_empty() {
        &row.body
    } else {
        &row.raw_body
    }
}

/// `MessageBubble`'s `src`: the persisted URL or the chat attachment endpoint.
/// The native image cache can also decode the upload's local path.
fn attachment_has_src(s: &RuneTranscriptState, file: &ChatAttachment) -> bool {
    file.url.as_deref().is_some_and(|url| !url.is_empty())
        || (!s.workspace_id.is_empty() && !s.chat_id.is_empty() && !file.id.is_empty())
        || (!file.path.is_empty() && std::path::Path::new(&file.path).exists())
}

/// Open a message attachment the way React's anchor does: the explicit URL,
/// or the local upload path as a `file:` URL. Server-only attachments have
/// no native download endpoint, so they keep the anchor's visual but no press.
fn attachment_open_url(s: &RuneTranscriptState, file: &ChatAttachment) -> Option<String> {
    let _ = s;
    if let Some(url) = file.url.as_deref().filter(|url| !url.is_empty()) {
        return Some(url.to_string());
    }
    if !file.path.is_empty() && std::path::Path::new(&file.path).exists() {
        let path = file.path.replace('\\', "/");
        return Some(format!("file:///{}", path.trim_start_matches('/')));
    }
    #[cfg(target_arch = "wasm32")]
    if !s.workspace_id.is_empty() && !s.chat_id.is_empty() && !file.id.is_empty() {
        return Some(format!(
            "/api/workspaces/{}/chats/{}/attachments/{}",
            s.workspace_id, s.chat_id, file.id
        ));
    }
    None
}

fn attachment_image(
    s: &RuneTranscriptState,
    file: &ChatAttachment,
    size: f32,
) -> Option<rune::Image> {
    crate::image_cache::attachment(file, size, &s.workspace_id, &s.chat_id)
}

/// `MessageBubble`'s `.message-attachments`: 86×66 image buttons, audio
/// bars and file chips. Rune has no native audio element, so an audio row
/// shows the same icon/name instead of playback controls.
fn home_message_attachments<S: TranscriptHost + 'static>(
    s: &S,
    which: Thread,
    r: &RowData,
) -> Option<El> {
    if r.attachments.is_empty() {
        return None;
    }
    let t = s.thread(which);
    let mut deck = row()
        .key(format!("{}-attachments", r.key))
        .flex_wrap()
        .items_center()
        .gap(6.0)
        .mt(9.0);
    for (index, file) in r.attachments.iter().enumerate() {
        let id = if file.id.is_empty() {
            index.to_string()
        } else {
            file.id.clone()
        };
        let key = format!("{}-file-{}", r.key, id);
        let has_src = attachment_has_src(t, file);
        if file.is_image && has_src {
            let image = attachment_image(t, file, 86.0);
            let mut button = rune::el()
                .key(key)
                .role("button")
                .label(format!("Open {}", file.name))
                .size(86.0, 66.0)
                .radius(9.0)
                .border(1.0, home_rgba(255.0, 255.0, 255.0, 0.11))
                .border_layout()
                .fill(home_rgba(0.0, 0.0, 0.0, 0.2))
                .clip_rounded(9.0);
            if let Some(image) = image {
                button = button.child(
                    rune::el()
                        .size(84.0, 64.0)
                        .radius(8.0)
                        .pixels_fit(image.pixels(), rune::SurfaceFit::Fill)
                        .inert(),
                );
            } else {
                button = button.center().child(
                    rune::el()
                        .size(16.0, 16.0)
                        .inert()
                        .shape(crate::lucide_image(16.0, home_ink("#c9c9d1"))),
                );
            }
            let file = file.clone();
            deck = deck.child(button.on_press(move |s: &mut S| {
                s.thread_mut(which).attachment_preview = Some(file.clone());
            }));
            continue;
        }
        if file.is_audio() && has_src {
            let child = row()
                .key(key)
                .role("group")
                .label(file.name.clone())
                .items_center()
                .gap(7.0)
                .max_w(280.0)
                .px(7.0)
                .py(5.0)
                .radius(9.0)
                .border(1.0, home_rgba(255.0, 255.0, 255.0, 0.1))
                .border_layout()
                .fill(home_rgba(0.0, 0.0, 0.0, 0.18))
                .child(
                    rune::el()
                        .size(14.0, 14.0)
                        .inert()
                        .shape(crate::lucide_audio_lines(14.0, home_ink("#cfcfd6"))),
                )
                .child(
                    text(file.name.clone())
                        .min_w(0.0)
                        .max_w(220.0)
                        .truncate()
                        .font(rune::tok::TextStyle::new(11.0, 400.0))
                        .color(home_ink("#cfcfd6")),
                );
            deck = deck.child(child);
            continue;
        }
        let src = has_src;
        let open = attachment_open_url(t, file);
        let mut chip = row()
            .key(key)
            .role("link")
            .label(format!("Download {}", file.name))
            .items_center()
            .gap(5.0)
            .max_w(220.0)
            .px(7.0)
            .py(5.0)
            .radius(8.0)
            .border(1.0, home_rgba(255.0, 255.0, 255.0, 0.1))
            .border_layout()
            .fill(home_rgba(0.0, 0.0, 0.0, 0.16))
            .child(rune::el().size(14.0, 14.0).inert().shape(if file.is_image {
                crate::lucide_image(14.0, home_ink("#cdcdd4"))
            } else {
                crate::lucide_file_text(14.0, home_ink("#cdcdd4"))
            }))
            .child(
                text(file.name.clone())
                    .min_w(0.0)
                    .grow(1.0)
                    .truncate()
                    .font(rune::tok::TextStyle::new(11.0, 400.0))
                    .color(home_ink("#cdcdd4")),
            );
        if src {
            chip = chip.child(
                rune::el()
                    .size(12.0, 12.0)
                    .opacity(0.6)
                    .inert()
                    .shape(crate::lucide_download(12.0, home_ink("#cdcdd4"))),
            );
        }
        if let Some(url) = open {
            chip = chip.on_press_tx(move |_: &mut S, tx: &mut rune::Tx| {
                let _ = tx.open_url(&url);
            });
        }
        deck = deck.child(chip);
    }
    Some(deck)
}

/// `message-lightbox`: fixed full-viewport preview, blurred backdrop, and a
/// close button. Any press dismisses it, matching React's backdrop handler.
pub fn attachment_lightbox<S: 'static>(
    file: &ChatAttachment,
    window_w: f32,
    window_h: f32,
    workspace_id: &str,
    chat_id: &str,
    close: impl Fn(&mut S) + 'static,
) -> El {
    let close = std::rc::Rc::new(close);
    let max_w = (window_w * 0.92).clamp(280.0, 1200.0);
    let max_h = (window_h * 0.86).clamp(220.0, 2400.0);
    let mut content = rune::el()
        .key("message-lightbox-image")
        .max_w(max_w)
        .max_h(max_h)
        .radius(14.0)
        .clip_rounded(14.0)
        .shadow(
            30.0,
            90.0,
            rune::Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.7,
            },
        );
    let image = crate::image_cache::attachment(file, max_w, workspace_id, chat_id);
    match image {
        Some(image) => {
            if let Some([w, h]) = image.size() {
                content = content
                    .aspect_fit(w as f32 / h.max(1) as f32)
                    .max_w(max_w)
                    .max_h(max_h);
            } else {
                content = content.size((max_w * 0.62).min(760.0), (max_h * 0.62).min(520.0));
            }
            content = content.pixels_fit(image.pixels(), rune::SurfaceFit::Fit);
        }
        None => {
            content = content
                .w((max_w * 0.5).min(520.0))
                .px(24.0)
                .py(22.0)
                .radius(14.0)
                .border(1.0, home_rgba(255.0, 255.0, 255.0, 0.12))
                .border_layout()
                .fill(home_rgba(20.0, 20.0, 24.0, 0.96))
                .child(
                    text(file.name.clone())
                        .font(rune::tok::TextStyle::new(13.5, 500.0))
                        .color(home_ink("#f0f0f2")),
                );
        }
    }
    rune::el()
        .key("message-lightbox")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(home_rgba(0.0, 0.0, 0.0, 0.82))
        .backdrop_blur(10.0)
        .center()
        .modal(true, {
            let close = close.clone();
            move |s: &mut S| close(s)
        })
        .child(
            rune::el()
                .key("message-lightbox-close")
                .pin(None, Some(18.0), Some(18.0), None)
                .role("button")
                .label("Close image preview")
                .size(34.0, 34.0)
                .radius(10.0)
                .border(1.0, home_rgba(255.0, 255.0, 255.0, 0.14))
                .border_layout()
                .fill(home_rgba(20.0, 20.0, 24.0, 0.8))
                .center()
                .hover(home_rgba(255.0, 255.0, 255.0, 0.12))
                .on_press({
                    let close = close.clone();
                    move |s: &mut S| close(s)
                })
                .child(rune::el().size(16.0, 16.0).inert().shape(crate::lucide_x(
                    16.0,
                    2.0,
                    home_ink("#dddddd"),
                ))),
        )
        .child(content)
}

/// `MessageBubble.copyMessage`: write the row's raw text and, for user and
/// error rows only, hold React's 1.5s Check state. Agent copy has no feedback.
fn copy_message<S: TranscriptHost + 'static>(
    s: &mut S,
    which: Thread,
    key: &str,
    text: String,
    feedback: bool,
    tx: &mut rune::Tx,
) {
    tx.copy(text);
    if feedback {
        s.thread_mut(which)
            .copied_messages
            .insert(key.to_string(), std::time::Instant::now());
        // Poll again when the 1.5s window ends even if nothing else moves.
        tx.wake_after(std::time::Duration::from_millis(50));
    }
}

/// First press arms the row; the 3.2s poll timeout disarms it. React keeps
/// this state local to each `MessageBubble`.
fn arm_rewind<S: TranscriptHost + 'static>(s: &mut S, which: Thread, key: &str, tx: &mut rune::Tx) {
    s.thread_mut(which)
        .rewind_armed
        .insert(key.to_string(), std::time::Instant::now());
    tx.wake_after(std::time::Duration::from_millis(50));
}

/// Detach the active stream before `/rewind`: like `abortRef.current.abort()`
/// followed by `resetStream()`, no partial is finalized or persisted. The
/// server truncates and the fetched history replaces these rows.
fn detach_live_run(s: &mut RuneTranscriptState) {
    s.inbox = None;
    s.live = false;
    s.aborting = false;
    s.run_started_ms = None;
    s.pending_saves.clear();
    s.refresh_chat = None;
    s.rows
        .retain(|row| !(row.is_streaming || row.accum.is_some()));
}

/// Second press: POST `/rewind` and refetch the chat. Results land in
/// `rewind_inbox` for the poll hook, exactly like a run.
fn start_rewind(s: &mut RuneTranscriptState, row_id: String, attachments: Vec<ChatAttachment>) {
    if s.rewind_inbox.is_some() {
        return;
    }
    s.rewind_armed.remove(&row_id);
    detach_live_run(s);
    if s.chat_id.trim().is_empty() || s.workspace_id.trim().is_empty() {
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    let (host, port) = (s.api_host.clone(), s.api_port);
    let (workspace, chat) = (s.workspace_id.clone(), s.chat_id.clone());
    let (tx, rx) = std::sync::mpsc::channel();
    s.rewind_inbox = Some(rx);
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let result = (|| -> Result<RewindOk, String> {
            let body = serde_json::json!({
                "message_id": row_id,
                "restore_files": true,
                "force": false,
            });
            let value = crate::transport::send_json(
                &host,
                port,
                "POST",
                &format!("/api/workspaces/{workspace}/chats/{chat}/rewind"),
                &body,
            )?;
            let prompt = value
                .get("prompt")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let page = crate::transport::get_json(
                &host,
                port,
                &format!("/api/workspaces/{workspace}/chats/{chat}"),
            )?;
            Ok(RewindOk {
                row_id: row_id.clone(),
                prompt,
                attachments,
                rows: crate::chats::parse_messages(&page),
            })
        })();
        let _ = tx.send(result);
    });
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let result = async {
            let body = serde_json::json!({
                "message_id": row_id,
                "restore_files": true,
                "force": false,
            });
            let value = crate::transport::fetch_send(
                "POST",
                &format!("/api/workspaces/{workspace}/chats/{chat}/rewind"),
                Some(&body),
            )
            .await?;
            let prompt = value
                .get("prompt")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let page =
                crate::transport::fetch_json(&format!("/api/workspaces/{workspace}/chats/{chat}"))
                    .await?;
            Ok(RewindOk {
                row_id: row_id.clone(),
                prompt,
                attachments,
                rows: crate::chats::parse_messages(&page),
            })
        }
        .await;
        let _ = tx.send(result);
    });
}

/// Apply React's `handleRewind` result: defensive slice before the clicked
/// id, restore the prompt and the clicked message's attachments, then refresh
/// the sidebar. Failures stay silent: `HouseChat` declares `notification` but
/// never renders it, so the React UI shows nothing either.
fn apply_rewind(s: &mut RuneTranscriptState, result: RewindOk) {
    let cut = result
        .rows
        .iter()
        .position(|row| row.key == result.row_id)
        .unwrap_or(result.rows.len());
    s.rows = result.rows[..cut]
        .iter()
        .map(crate::rune_canvas::row_from_chat)
        .collect();
    s.edit.replace_text(result.prompt);
    s.draft_attachments = result.attachments;
    s.attachments_error = None;
    s.attachment_preview = None;
    s.rewind_armed.clear();
    s.live = false;
    s.aborting = false;
    s.run_started_ms = None;
    s.refresh_list = true;
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
            .filter(|r| r.role == "error" && !raw_of(r).trim().is_empty())
            .map(|r| normalize(raw_of(r)))
            .collect();
        let mut visible = vec![true; rows.len()];
        for (i, r) in rows.iter().enumerate() {
            if crate::stream_presentation::is_internal_transport(raw_of(r)) {
                visible[i] = false;
                continue;
            }
            if r.role != "agent" || raw_of(r).trim().is_empty() || errors.is_empty() {
                continue;
            }
            let text = normalize(raw_of(r));
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
        let mut retry_prompts = vec![String::new(); rows.len()];
        let mut previous_day = String::new();
        let mut last_user = String::new();
        for i in 0..rows.len() {
            if !visible[i] {
                continue;
            }
            if rows[i].role == "user" && !raw_of(&rows[i]).trim().is_empty() {
                last_user = raw_of(&rows[i]).to_string();
            }
            retry_prompts[i] = last_user.clone();
            let day = day_key(&rows[i].at);
            if !day.is_empty() && day != previous_day {
                dividers[i] = Some(day_label(&rows[i].at));
            }
            previous_day = day;
        }
        Self {
            visible,
            dividers,
            retry_prompts,
        }
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

/// Lucide `ArrowDown` at a 24-unit box (jump-to-latest pill).
fn home_icon_arrow_down(size: f32, color: rune::Color) -> rune::Shape {
    let p = rune::Path::new()
        .move_to([12.0, 5.0])
        .line_to([12.0, 19.0])
        .move_to([19.0, 12.0])
        .line_to([12.0, 19.0])
        .line_to([5.0, 12.0]);
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
                    style.line_height = 13.5;
                    // `.chat-date-label` `letter-spacing: .05em`.
                    style.tracking = 0.45;
                    style
                })
                .color(home_ink("#5f5f68"))
                .px(8.0)
                .py(2.0)
                .radius(999.0)
                .border(1.0, home_ink("#ffffff0f"))
                .border_layout()
                .fill(home_ink("#ffffff05")),
        )
        .child(line())
}

fn home_user_row<S: TranscriptHost + 'static>(s: &S, which: Thread, r: &RowData, max_w: f32) -> El {
    let stamp = clock(&r.at);
    let copied = s.thread(which).copied_messages.contains_key(&r.key);
    let armed = s.thread(which).rewind_armed.contains_key(&r.key);
    let mut bubble = col()
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
        .max_w(max_w);
    if !r.body.trim().is_empty() {
        bubble = bubble.child(
            render_body_styled::<S>(s, which, &r.key, &r.body, "#e4e4e7", home_body_style())
                .pt(12.0)
                .pb(12.0),
        );
    }
    if let Some(attachments) = home_message_attachments::<S>(s, which, r) {
        bubble = bubble.child(attachments);
    }
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
    let copy_key = r.key.clone();
    let copy_text = raw_of(r).to_string();
    actions = actions.child(
        rune::el()
            .key(format!("{}-copy", r.key))
            .role("button")
            .label(if copied { "Copied" } else { "Copy message" })
            .size(24.0, 24.0)
            .radius(6.0)
            .center()
            .hover(home_rgba(255.0, 255.0, 255.0, 0.07))
            .on_press_tx(move |s: &mut S, tx: &mut rune::Tx| {
                copy_message(s, which, &copy_key, copy_text.clone(), true, tx);
            })
            .child(rune::el().size(12.0, 12.0).inert().shape(if copied {
                crate::lucide_check(12.0, 2.0, home_ink("#34d399"))
            } else {
                home_icon_copy(12.0, home_ink("#52525b"))
            })),
    );
    if armed {
        let rewind_key = r.key.clone();
        let rewind_attachments = r.attachments.clone();
        actions = actions.child(
            row()
                .key(format!("{}-rewind", r.key))
                .role("button")
                .label("Confirm rewind")
                .items_center()
                .gap(4.0)
                .h(24.0)
                .px(8.0)
                .radius(6.0)
                .fill(home_rgba(239.0, 68.0, 68.0, 0.15))
                .hover(home_rgba(239.0, 68.0, 68.0, 0.22))
                .on_press_tx(move |s: &mut S, _tx: &mut rune::Tx| {
                    start_rewind(
                        s.thread_mut(which),
                        rewind_key.clone(),
                        rewind_attachments.clone(),
                    );
                })
                .child(
                    rune::el()
                        .size(11.0, 11.0)
                        .inert()
                        .shape(home_icon_rotate(11.0, home_ink("#fca5a5"))),
                )
                .child(
                    text("Rewind?".to_string())
                        .font({
                            let mut style = rune::tok::TextStyle::new(10.0, 600.0);
                            style.line_height = 15.0;
                            style
                        })
                        .color(home_ink("#fca5a5")),
                ),
        );
    } else {
        let rewind_key = r.key.clone();
        actions = actions.child(
            rune::el()
                .key(format!("{}-rewind", r.key))
                .role("button")
                .label("Rewind to this message")
                .size(24.0, 24.0)
                .radius(6.0)
                .center()
                .hover(home_rgba(239.0, 68.0, 68.0, 0.1))
                .on_press_tx(move |s: &mut S, tx: &mut rune::Tx| {
                    arm_rewind(s, which, &rewind_key, tx);
                })
                .child(
                    rune::el()
                        .size(12.0, 12.0)
                        .inert()
                        .shape(home_icon_rotate(12.0, home_ink("#52525b"))),
                ),
        );
    }
    col()
        .key(format!("{}-group", r.key))
        .w_full()
        .mb(24.0)
        .child(row().w_full().justify_end().child(bubble))
        .child(actions)
}

fn home_agent_row<S: TranscriptHost + 'static>(
    s: &S,
    which: Thread,
    r: &RowData,
    live: bool,
    retry_prompt: &str,
) -> El {
    let streaming = r.is_streaming || (live && r.accum.is_some());
    let presentation = if let Some(acc) = &r.accum {
        crate::stream_presentation::present_stream(&acc.segments, &acc.text, true, &[])
    } else if r.final_text.is_empty() && r.activity.is_empty() {
        // Seed/demo rows carry only `body` (no cached presentation).
        crate::stream_presentation::present_stream(&r.timeline, &r.body, false, &[])
    } else {
        crate::stream_presentation::Presentation {
            final_text: r.final_text.clone(),
            activity: r.activity.clone(),
        }
    };
    let final_text = presentation.final_text;
    let activity = presentation.activity;
    let retry = r.accum.as_ref().and_then(|acc| acc.retry.as_ref());
    let live_status = r.accum.as_ref().and_then(|acc| acc.status.as_ref());
    let mut group = col()
        .key(format!("{}-group", r.key))
        .w_full()
        .mb(24.0)
        .hover_group();

    if !r.agent_name.is_empty() {
        group = group.child(
            text(r.agent_name.to_uppercase())
                .key(format!("{}-agent-name", r.key))
                .font({
                    let mut style = rune::tok::TextStyle::new(11.0, 700.0);
                    style.line_height = 16.5;
                    style.tracking = 0.55;
                    style
                })
                .color(home_ink("#71717a"))
                .mb(6.0),
        );
    }

    if streaming {
        if let Some(retry) = retry {
            group = group.child(activity::retry_status(&r.key, retry));
        }
        if let Some(status) = live_status {
            group = group.child(activity::live_compaction_status(&r.key, status));
        }
        if !activity.is_empty() {
            group = group.child(activity::activity_timeline::<S>(
                s, which, &r.key, &activity, true, r,
            ));
        } else if final_text.trim().is_empty() && retry.is_none() {
            group = group.child(activity::thinking_placeholder(&r.key));
        }
        if let Some(cards) = activity::live_delegation_cards(r, &activity) {
            group = group.child(cards);
        }
    } else if r.show_trace {
        group = group.child(activity::trace_accordion::<S>(s, which, r));
    }

    if r.stopped && !streaming {
        group = group.child(activity::stopped_badge(&r.key));
    }

    if !final_text.trim().is_empty() && !r.hide_body {
        if !streaming && !activity.is_empty() {
            group = group.child(
                text("FINAL RESPONSE".to_string())
                    .key(format!("{}-kicker", r.key))
                    .font({
                        let mut style = rune::tok::TextStyle::new(10.0, 700.0);
                        style.line_height = 15.0;
                        style.tracking = 0.8;
                        style
                    })
                    .color(home_ink("#8d9099"))
                    .mb(5.0),
            );
        }
        let response = if streaming {
            md_render::render_body_streaming::<S>(
                s,
                which,
                &r.key,
                &final_text,
                "#e4e4e7",
                home_body_style(),
            )
        } else {
            render_body_styled::<S>(s, which, &r.key, &final_text, "#e4e4e7", home_body_style())
        }
        .pt(12.0)
        // React's `.agent-message-response` ends after the last block's
        // own bottom margin (`mb-12`/`mb-20`), not at the final glyph.
        .pb(md_render::last_margin(&final_text));
        group = group.child(response);
        if streaming {
            group = group.child(
                text("▍".to_string())
                    .key(format!("{}-caret", r.key))
                    .font(home_body_style())
                    .color(home_ink("#a78bfa"))
                    .ml(1.0)
                    .looping(rune::LoopSpec::opacity(0.9, 0.0, 0.0, 1.0)),
            );
        }
    }

    if live && r.accum.is_none() {
        // Legacy/demo live row (no accumulator): keep the 3-dot typing
        // bounce under the visible body, as `canvas-bubble-typing` does.
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
        group = group.child(dots);
    }

    if r.has_usage && !streaming {
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
                text(format!("↑{}", thousands(r.tokens_in)))
                    .key(format!("{}-in", r.key))
                    .font(mono)
                    .color(home_ink("#52525b")),
            )
            .child(
                text(format!("↓{}", thousands(r.tokens_out)))
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
        group = group.child(footer);
    }
    group.child(home_agent_actions(
        s,
        which,
        r,
        &final_text,
        streaming,
        retry_prompt,
    ))
}

/// `compactTokens` (`SessionEventDivider`): `1240` → `1.2k`, `2M` → `2.0M`.
fn compact_tokens(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 10_000 {
        format!("{:.0}k", value as f64 / 1_000.0)
    } else if value >= 1_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

/// `toLocaleString()` token grouping (en-US): `1234` → `1,234`.
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Hover-revealed agent actions (React reserves the 25pt row at rest).
/// Retry hides while the answer streams; copy always stays. Agent copy has
/// no Check feedback (React writes to the clipboard only).
fn home_agent_actions<S: TranscriptHost + 'static>(
    _s: &S,
    which: Thread,
    r: &RowData,
    response: &str,
    streaming: bool,
    retry_prompt: &str,
) -> El {
    let mut actions = row()
        .key(format!("{}-actions", r.key))
        .w_full()
        .h(25.0)
        .justify_end()
        .items_center()
        .gap(2.0)
        .mt(8.0)
        .group_opacity(0.0, 1.0);
    if !streaming {
        let prompt = retry_prompt.to_string();
        actions = actions.child(
            rune::el()
                .key(format!("{}-retry", r.key))
                .role("button")
                .label("Retry — re-run the last request")
                .size(25.0, 25.0)
                .radius(6.0)
                .center()
                .hover(home_ink("#27272a"))
                .on_press_tx(move |s: &mut S, _tx: &mut rune::Tx| {
                    stage_send_prompt(s.thread_mut(which), prompt.clone());
                })
                .child(
                    rune::el()
                        .size(12.0, 12.0)
                        .inert()
                        .shape(home_icon_rotate(12.0, home_ink("#71717a"))),
                ),
        );
    }
    let copy_text = response.to_string();
    let copy_key = r.key.clone();
    actions.child(
        rune::el()
            .key(format!("{}-copy", r.key))
            .role("button")
            .label("Copy to clipboard")
            .size(25.0, 25.0)
            .radius(6.0)
            .center()
            .hover(home_ink("#27272a"))
            .on_press_tx(move |s: &mut S, tx: &mut rune::Tx| {
                copy_message(s, which, &copy_key, copy_text.clone(), false, tx);
            })
            .child(
                rune::el()
                    .size(12.0, 12.0)
                    .inert()
                    .shape(home_icon_copy(12.0, home_ink("#71717a"))),
            ),
    )
}

/// `MessageBubble` system note (`.system-message-note`).
fn home_system_row(r: &RowData) -> El {
    col()
        .key(format!("{}-group", r.key))
        .w_full()
        .mb(24.0)
        .child(
            row()
                .key(format!("{}-note", r.key))
                .items_center()
                .gap(7.0)
                .mt(2.0)
                .px(9.0)
                .py(6.0)
                .radius(8.0)
                .border(1.0, home_rgba(255.0, 255.0, 255.0, 0.06))
                .border_layout()
                .fill(home_rgba(255.0, 255.0, 255.0, 0.018))
                .child(
                    rune::el()
                        .key(format!("{}-dot", r.key))
                        .size(5.0, 5.0)
                        .radius(2.5)
                        .fill(home_ink("#777780")),
                )
                .child(
                    text(r.body.clone())
                        .key(format!("{}-text", r.key))
                        .font({
                            let mut style = rune::tok::TextStyle::new(11.0, 400.0);
                            style.line_height = 15.4;
                            style
                        })
                        .color(home_ink("#777780")),
                ),
        )
}

fn home_error_row<S: TranscriptHost + 'static>(
    s: &S,
    which: Thread,
    r: &RowData,
    retry_prompt: &str,
) -> El {
    let copied = s.thread(which).copied_messages.contains_key(&r.key);
    let mut body_style = rune::tok::TextStyle::new(13.0, 400.0);
    body_style.line_height = 13.0 * 1.625;
    let body = text(r.body.clone())
        .key(format!("{}-error-text", r.key))
        .font(body_style)
        .wrap_anywhere()
        .color(home_rgba(252.0, 165.0, 165.0, 0.9));
    let retry_prompt = retry_prompt.to_string();
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
        .on_press_tx(move |s: &mut S, _tx: &mut rune::Tx| {
            stage_send_prompt(s.thread_mut(which), retry_prompt.clone());
        })
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
    let copy_key = r.key.clone();
    let copy_text = r.body.clone();
    let copy = row()
        .key(format!("{}-copy", r.key))
        .role("button")
        .label(if copied { "Copied" } else { "Copy" })
        .items_center()
        .gap(4.0)
        .px(8.0)
        .py(4.0)
        .radius(6.0)
        .hover(home_rgba(255.0, 255.0, 255.0, 0.06))
        .on_press_tx(move |s: &mut S, tx: &mut rune::Tx| {
            copy_message(s, which, &copy_key, copy_text.clone(), true, tx);
        })
        .child(rune::el().size(10.0, 10.0).inert().shape(if copied {
            crate::lucide_check(10.0, 2.0, home_ink("#34d399"))
        } else {
            home_icon_copy(10.0, home_ink("#71717a"))
        }))
        .child(
            text(if copied { "Copied" } else { "Copy" }.to_string())
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
fn home_session_event<S: TranscriptHost + 'static>(
    r: &RowData,
    which: Thread,
    retry_prompt: &str,
) -> El {
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
                // `.session-event-label` `letter-spacing: .01em`.
                style.tracking = 0.105;
                style
            })
            .color(if aborted {
                home_ink("#e4c46a")
            } else {
                home_ink("#a1a1a9")
            }),
        );
    if !aborted {
        let meta = if r.event_input > 0 || r.event_output > 0 {
            let mut parts = Vec::new();
            if r.event_input > 0 {
                parts.push(format!("{} in", compact_tokens(r.event_input)));
            }
            if r.event_output > 0 {
                parts.push(format!("{} out", compact_tokens(r.event_output)));
            }
            parts.join(" · ")
        } else if r.event_tokens_before > 0 {
            format!("{} context", compact_tokens(r.event_tokens_before))
        } else {
            String::new()
        };
        if !meta.is_empty() {
            main = main.child(
                text(meta)
                    .font({
                        let mut style = rune::tok::TextStyle::new(9.5, 400.0).mono();
                        style.line_height = 9.5;
                        style
                    })
                    .color(home_ink("#5f5f68")),
            );
        }
    }
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
        let retry_prompt = retry_prompt.to_string();
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
                .on_press_tx(move |s: &mut S, _tx: &mut rune::Tx| {
                    stage_send_prompt(s.thread_mut(which), retry_prompt.clone());
                })
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
    if r.role == "user" {
        root = root.arrives_whole();
    } else if r.is_streaming {
        root = root.streaming(true);
    }
    if let Some(label) = &meta.dividers[i] {
        root = root.child(home_divider(label));
    }
    if !meta.visible[i] {
        return root;
    }
    let live = t.live && i + 1 == t.rows.len() && r.role != "user" && r.role != "error";
    let retry_prompt = meta.retry_prompts[i].clone();
    let group = if !r.event.is_empty() {
        home_session_event::<S>(r, which, &retry_prompt)
    } else {
        match r.role {
            "user" => home_user_row::<S>(s, which, r, max_w),
            "error" => home_error_row::<S>(s, which, r, &retry_prompt),
            "system" => home_system_row(r),
            _ => home_agent_row::<S>(s, which, r, live, &retry_prompt),
        }
    };
    root.child(group)
}

/// `ConversationRail`: up to six 7×2 turn dashes centered in a 24pt column
/// at the scroll area's left edge (React hides it below two turns).
fn conversation_rail(rows: &[RowData]) -> Option<El> {
    let mut turns: Vec<Vec<&RowData>> = Vec::new();
    let mut current: Vec<&RowData> = Vec::new();
    for r in rows.iter().filter(|r| !raw_of(r).trim().is_empty()) {
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
                        .font({
                            let mut style = rune::tok::TextStyle::new(19.0, 500.0);
                            // `tracking-tight`: -0.025em.
                            style.tracking = -0.475;
                            style
                        })
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
    let pill = s.scroll_pill.clone();
    let viewport = col()
        .key("transcript-viewport")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: true })
        .on_scroll_offset(move |at| {
            // React shows the pill past 120pt from the bottom; fade it in
            // over the following 40pt so it never pops.
            let distance = (at.max[1] - at.offset[1]).max(0.0);
            pill.set(((distance - 120.0) / 40.0).clamp(0.0, 1.0));
        })
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
    // Jump-to-latest pill (`.scroll-bottom-pill`). Always mounted with a
    // live opacity so parity captures at the bottom stay byte-identical.
    view = view.child(
        row()
            .key("scroll-pill-row")
            .w_full()
            .pin(Some(0.0), None, Some(0.0), Some(14.0))
            .justify_center()
            .child(
                row()
                    .key("scroll-bottom-pill")
                    .role("button")
                    .label("Jump to latest")
                    .items_center()
                    .gap(6.0)
                    .px(12.0)
                    .py(6.0)
                    .radius(999.0)
                    .border(1.0, home_rgba(255.0, 255.0, 255.0, 0.12))
                    .border_layout()
                    .fill(home_rgba(24.0, 24.0, 28.0, 0.92))
                    .shadow(
                        10.0,
                        30.0,
                        rune::Color {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 0.4,
                        },
                    )
                    .live_opacity(&s.scroll_pill)
                    .on_press_tx(|_: &mut S, tx: &mut rune::Tx| {
                        tx.scroll_to_bottom("transcript-viewport");
                    })
                    .child(
                        rune::el()
                            .size(13.0, 13.0)
                            .inert()
                            .shape(home_icon_arrow_down(13.0, home_ink("#cfcfd6"))),
                    )
                    .child(
                        text("Latest".to_string())
                            .font({
                                let mut style = rune::tok::TextStyle::new(11.5, 600.0);
                                style.line_height = 17.25;
                                style
                            })
                            .color(home_ink("#cfcfd6")),
                    ),
            ),
    );
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
        std::rc::Rc::new(move |s: &mut S| &mut s.thread_mut(which).edit),
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
            attachments: &s.draft_attachments,
            attachments_error: s.attachments_error.as_deref(),
            attachments_dragging: s.attachments_dragging,
            workspace_id: &s.workspace_id,
            chat_id: &s.chat_id,
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
            attach: std::rc::Rc::new(move |s: &mut S, tx: &mut rune::Tx| {
                open_attachment_picker(s, which, tx);
            }),
            remove_attachment: std::rc::Rc::new(move |s: &mut S, target: String| {
                remove_draft_attachment(s.thread_mut(which), &target);
            }),
            drop_files: std::rc::Rc::new(move |s: &mut S, files: &[std::path::PathBuf]| {
                drop_attachment_files(s.thread_mut(which), files);
            }),
            drop_hover: std::rc::Rc::new(move |s: &mut S, over: bool| {
                s.thread_mut(which).attachments_dragging = over;
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
        if let Some(file) = s.attachment_preview.clone() {
            let (w, h) = transcript_window();
            root = root.child(attachment_lightbox::<S>(
                &file,
                w,
                h,
                &s.workspace_id,
                &s.chat_id,
                move |s: &mut S| {
                    s.thread_mut(which).attachment_preview = None;
                },
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
                            rune::el()
                                .key("settings-close")
                                .size(16.0, 16.0)
                                .role("button")
                                .label("Close settings")
                                .shape(crate::lucide_x(16.0, 2.0, ink("#8a8a93")))
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

fn apply_attachments(s: &mut RuneTranscriptState) {
    *s = attachments_demo();
}

/// Composer attachment deck proof: image thumbnail (real PNG asset), file and
/// audio chips, the count, amber error line and the attachment-only send key;
/// plus a persisted user turn with message cards.
pub fn attachments_demo() -> RuneTranscriptState {
    let mut s = RuneTranscriptState::screenshot();
    s.live = false;
    s.draft_attachments = vec![
        ChatAttachment {
            id: "demo-plan".to_string(),
            name: "quarterly-plan.pdf".to_string(),
            file_type: "application/pdf".to_string(),
            size: 1_240_000,
            path: String::new(),
            is_image: false,
            url: None,
        },
        ChatAttachment {
            id: String::new(),
            name: "attachment-sample.png".to_string(),
            file_type: "image/png".to_string(),
            size: 667,
            path: concat!(env!("CARGO_MANIFEST_DIR"), "/assets/attachment-sample.png").to_string(),
            is_image: true,
            url: None,
        },
        ChatAttachment {
            id: "demo-voice".to_string(),
            name: "voice-note.mp3".to_string(),
            file_type: "audio/mpeg".to_string(),
            size: 482_000,
            path: String::new(),
            is_image: false,
            url: None,
        },
    ];
    s.attachments_error = Some("Files must be 20 MB or smaller.".to_string());
    // A persisted user turn exercises the message cards, not just the deck.
    let mut row = RowData::default();
    row.key = "demo-user-attachments".to_string();
    row.role = "user";
    row.body = "Here are the files for this pass.".to_string();
    row.raw_body = row.body.clone();
    row.at = "2026-11-12T12:04:00Z".to_string();
    row.attachments = vec![
        ChatAttachment {
            id: "demo-msg-image".to_string(),
            name: "attachment-sample.png".to_string(),
            file_type: "image/png".to_string(),
            size: 667,
            path: concat!(env!("CARGO_MANIFEST_DIR"), "/assets/attachment-sample.png").to_string(),
            is_image: true,
            url: None,
        },
        ChatAttachment {
            id: "demo-msg-pdf".to_string(),
            name: "quarterly-plan.pdf".to_string(),
            file_type: "application/pdf".to_string(),
            size: 1_240_000,
            path: String::new(),
            is_image: false,
            url: None,
        },
    ];
    s.rows.push(row);
    s
}

/// `poll` hook: drain run frames queued by the transport thread into rows.
/// Returns true when state changed (requests another frame).
pub fn drain_run_inbox(s: &mut RuneTranscriptState, _clock: f64, tx: &mut rune::Tx) -> bool {
    let mut changed = false;
    // `Markdown.tsx`'s copy button shows the Check for 1.8s.
    if s.copied_at
        .is_some_and(|at| at.elapsed() >= std::time::Duration::from_millis(1800))
    {
        s.copied_code = None;
        s.copied_at = None;
        changed = true;
    }
    // Message copy (1.5s) and rewind arming (3.2s) are simple expiries.
    let now = std::time::Instant::now();
    let copied_before = s.copied_messages.len();
    s.copied_messages
        .retain(|_, at| now.duration_since(*at) < std::time::Duration::from_millis(1500));
    let armed_before = s.rewind_armed.len();
    s.rewind_armed
        .retain(|_, at| now.duration_since(*at) < std::time::Duration::from_millis(3200));
    if s.copied_messages.len() != copied_before || s.rewind_armed.len() != armed_before {
        changed = true;
    }
    if !s.copied_messages.is_empty() || !s.rewind_armed.is_empty() {
        tx.wake_after(std::time::Duration::from_millis(50));
    }
    // Detaching the live run may leave the view at rest, so keep a poll
    // ticket alive until the blocking `/rewind` result lands.
    if s.rewind_inbox.is_some() {
        tx.wake_after(std::time::Duration::from_millis(50));
    }
    // `/rewind` result: fetched history replaces the detached live rows.
    match s.rewind_inbox.as_ref().map(|rx| rx.try_recv()) {
        Some(Ok(Ok(result))) => {
            s.rewind_inbox = None;
            apply_rewind(s, result);
            changed = true;
        }
        Some(Ok(Err(_message))) => {
            // No visible error banner in React's HouseChat.
            s.rewind_inbox = None;
            changed = true;
        }
        Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
            s.rewind_inbox = None;
            changed = true;
        }
        Some(Err(std::sync::mpsc::TryRecvError::Empty)) | None => {}
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
            // React calls `finish()` when the stream closes without a
            // terminal event, so a dropped connection still lands the
            // partial answer instead of leaving a live row forever.
            if s.aborting {
                s.aborting = false;
                s.refresh_chat = Some((s.workspace_id.clone(), s.chat_id.clone()));
                changed = true;
            } else if s.live {
                finalize_run(s, None, false);
                changed = true;
            }
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
            ("attachments", apply_attachments),
        ],
        poll: Some(drain_run_inbox),
        setup: Some(crate::image_cache::setup::<RuneTranscriptState>),
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
        assert_eq!(rt.state.rows.len(), n + 2);
        assert_eq!(rt.state.rows[n].body, "hello agent");
        assert_eq!(rt.state.rows[n + 1].key, crate::stream::LIVE_ROW_KEY);
        assert!(rt.state.rows[n + 1].is_streaming);
        assert!(rt.state.live);
        assert!(rt.state.edit.text.is_empty());
    }

    #[test]
    fn shift_enter_inserts_a_newline_instead_of_sending() {
        use rune::{Key, KeyEvent, Mods};
        let env = Env::new(true, 2.0);
        let mut init = RuneTranscriptState::screenshot();
        init.edit = TextEdit::new("hello".to_string());
        init.edit.focused = true;
        init.live = false;
        init.api_port = 1;
        let n = init.rows.len();
        let mut rt = Runtime::new(init, transcript_view, (468.0, 720.0), env);
        rt.frame(0.0);
        rt.key(KeyEvent {
            key: Key::Enter,
            mods: Mods {
                shift: true,
                ..Mods::default()
            },
        });
        rt.frame(0.0);
        assert_eq!(rt.state.rows.len(), n);
        assert!(rt.state.edit.text.contains('\n'));
        assert!(!rt.state.live);
    }

    #[test]
    fn send_while_live_is_a_noop() {
        let mut s = RuneTranscriptState::screenshot();
        s.live = true;
        let n = s.rows.len();
        crate::rune_transcript::stage_send_prompt(&mut s, "second".to_string());
        assert_eq!(s.rows.len(), n);
        assert_eq!(s.pending_saves.len(), 0);
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
        assert_eq!(rt.state.rows.len(), n + 2);
        assert_eq!(rt.state.rows[n].role, "user");
        assert_eq!(rt.state.rows[n].body, "hello agent");
        assert_eq!(rt.state.rows[n + 1].key, crate::stream::LIVE_ROW_KEY);
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
    fn attachment_deck_mounts_and_remove_wires() {
        let env = Env::new(true, 2.0);
        let mut init = RuneTranscriptState::screenshot();
        init.live = false;
        init.draft_attachments = vec![
            ChatAttachment {
                id: "att-1".to_string(),
                name: "notes.md".to_string(),
                file_type: "text/markdown".to_string(),
                size: 2048,
                path: String::new(),
                is_image: false,
                url: None,
            },
            ChatAttachment {
                id: String::new(),
                name: "photo.png".to_string(),
                file_type: "image/png".to_string(),
                size: 4096,
                path: "no-such-photo.png".to_string(),
                is_image: true,
                url: None,
            },
            ChatAttachment {
                id: String::new(),
                name: "notes.md".to_string(),
                file_type: "text/plain; charset=utf-8".to_string(),
                size: 512,
                path: String::new(),
                is_image: false,
                url: None,
            },
        ];
        let mut rt = Runtime::new(init, transcript_view, (468.0, 720.0), env);
        rt.frame(0.0);
        for key in [
            "composer-attachment-deck",
            "composer-file-att-1",
            "composer-file-local-1",
            "composer-file-local-2",
            "composer-file-thumb-local-1",
            "composer-file-thumb-local-2",
            "composer-attachment-add",
            "composer-attachment-count",
        ] {
            assert!(rt.find(key).is_some(), "{key} mounted");
        }
        // Local draft cards have no upload id yet; the index keeps their
        // thumbs and copy blocks keyed uniquely.
        assert!(rt.find("composer-file-thumb-").is_none());
        // Attachments count as content: the violet send arrow, not Stop.
        assert!(rt.find("composer-send").is_some());
        assert!(rt.find("composer-stop").is_none());
        // Remove the local card (index target) then the restored upload.
        let (x, y) = rt.find("composer-file-remove-local-1").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert_eq!(rt.state.draft_attachments.len(), 2);
        assert_eq!(rt.state.draft_attachments[0].id, "att-1");
        assert_eq!(rt.state.draft_attachments[1].name, "notes.md");
        // The surviving local pick reindexes from 2 to 1.
        let (x, y) = rt.find("composer-file-remove-local-1").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert_eq!(rt.state.draft_attachments.len(), 1);
        assert_eq!(rt.state.draft_attachments[0].id, "att-1");
        let (x, y) = rt.find("composer-file-remove-att-1").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert!(rt.state.draft_attachments.is_empty());
        assert!(rt.find("composer-attachment-deck").is_none());
        assert!(rt.find("composer-file-att-1").is_none());
        // The empty deck drops back to the disabled arrow, never Stop.
        assert!(rt.find("composer-send").is_some());
        assert!(rt.find("composer-stop").is_none());
    }

    /// The home transcript with the shell's root lightbox, for tests that need
    /// the persisted user-row cards (the standalone sheet body has none).
    fn home_attachments_view(s: &RuneTranscriptState, _cx: &mut Cx<RuneTranscriptState>) -> El {
        let mut root = col().key("test-home-root").w_full().h_full().fill(bg());
        root = root.child(home_transcript_body::<RuneTranscriptState>(
            s,
            Thread::Main,
            468.0,
            720.0,
        ));
        if let Some(file) = s.attachment_preview.clone() {
            root = root.child(attachment_lightbox::<RuneTranscriptState>(
                &file,
                468.0,
                720.0,
                &s.workspace_id,
                &s.chat_id,
                |s: &mut RuneTranscriptState| {
                    s.attachment_preview = None;
                },
            ));
        }
        root
    }

    #[test]
    fn message_image_opens_and_closes_lightbox() {
        let env = Env::new(true, 2.0);
        let mut init = RuneTranscriptState::screenshot();
        init.live = false;
        let mut row = RowData::default();
        row.key = "u-att".to_string();
        row.role = "user";
        row.body = "Here are the files.".to_string();
        row.raw_body = row.body.clone();
        row.at = "2026-11-12T12:04:00Z".to_string();
        row.attachments = vec![
            ChatAttachment {
                id: "img-1".to_string(),
                name: "attachment-sample.png".to_string(),
                file_type: "image/png".to_string(),
                size: 667,
                path: concat!(env!("CARGO_MANIFEST_DIR"), "/assets/attachment-sample.png")
                    .to_string(),
                is_image: true,
                url: None,
            },
            ChatAttachment {
                id: "pdf-1".to_string(),
                name: "quarterly-plan.pdf".to_string(),
                file_type: "application/pdf".to_string(),
                size: 1_240_000,
                path: String::new(),
                is_image: false,
                url: None,
            },
        ];
        init.rows.push(row);
        let mut rt = Runtime::new(init, home_attachments_view, (468.0, 720.0), env);
        rt.frame(0.0);
        for key in ["u-att-attachments", "u-att-file-img-1", "u-att-file-pdf-1"] {
            assert!(rt.find(key).is_some(), "{key} mounted");
        }
        let (x, y) = rt.find("u-att-file-img-1").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert_eq!(
            rt.state.attachment_preview.as_ref().map(|f| f.id.as_str()),
            Some("img-1")
        );
        assert!(rt.find("message-lightbox").is_some());
        assert!(rt.find("message-lightbox-close").is_some());
        let (x, y) = rt.find("message-lightbox-close").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert!(rt.state.attachment_preview.is_none());
        assert!(rt.find("message-lightbox").is_none());
    }

    #[test]
    fn attachment_limits_match_promptdock() {
        let dir = std::env::temp_dir().join(format!(
            "stark-attach-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let paths: Vec<std::path::PathBuf> = (0..10)
            .map(|i| {
                let path = dir.join(format!("f{i}.txt"));
                std::fs::write(&path, b"x").unwrap();
                path
            })
            .collect();
        let mut s = RuneTranscriptState::screenshot();
        s.draft_attachments.clear();
        add_attachment_paths(&mut s, &paths);
        assert_eq!(s.draft_attachments.len(), MAX_ATTACHMENTS);
        assert_eq!(
            s.attachments_error.as_deref(),
            Some("You can attach up to 8 files at once.")
        );
        // Duplicates are dropped and re-report the cap, like React's
        // `next.length < current.length + valid.length` check.
        add_attachment_paths(&mut s, &paths[..1]);
        assert_eq!(s.draft_attachments.len(), MAX_ATTACHMENTS);
        assert_eq!(
            s.attachments_error.as_deref(),
            Some("You can attach up to 8 files at once.")
        );
        remove_draft_attachment(&mut s, "local:0");
        assert_eq!(s.draft_attachments.len(), MAX_ATTACHMENTS - 1);
        assert!(s.attachments_error.is_none(), "cap note clears");
        // Unknown ids are a no-op.
        remove_draft_attachment(&mut s, "att-missing");
        assert_eq!(s.draft_attachments.len(), MAX_ATTACHMENTS - 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rewind_truncates_before_clicked_and_restores_draft() {
        let mut s = RuneTranscriptState::screenshot();
        s.live = true;
        s.aborting = true;
        s.run_started_ms = Some(1.0);
        s.refresh_list = false;
        s.rewind_armed
            .insert("u2".to_string(), std::time::Instant::now());
        s.attachment_preview = Some(ChatAttachment {
            id: "old".to_string(),
            name: "old.png".to_string(),
            file_type: "image/png".to_string(),
            size: 1,
            path: String::new(),
            is_image: true,
            url: None,
        });
        let restored = ChatAttachment {
            id: "pdf-1".to_string(),
            name: "quarterly-plan.pdf".to_string(),
            file_type: "application/pdf".to_string(),
            size: 1_240_000,
            path: String::new(),
            is_image: false,
            url: None,
        };
        let row = |key: &str, role: &str, body: &str| ChatMsgRow {
            key: key.to_string(),
            role: role.to_string(),
            body: body.to_string(),
            raw_body: body.to_string(),
            ..ChatMsgRow::default()
        };
        apply_rewind(
            &mut s,
            RewindOk {
                row_id: "u2".to_string(),
                prompt: "edited prompt".to_string(),
                attachments: vec![restored.clone()],
                rows: vec![
                    row("u1", "user", "first"),
                    row("a1", "agent", "answer"),
                    row("u2", "user", "clicked"),
                    row("a2", "agent", "after"),
                ],
            },
        );
        // Everything from the clicked message on is gone, like React's
        // defensive slice before `message_id`.
        assert_eq!(s.rows.len(), 2);
        assert_eq!(s.rows[0].key, "u1");
        assert_eq!(s.rows[1].key, "a1");
        assert_eq!(s.edit.text, "edited prompt");
        assert_eq!(s.draft_attachments, vec![restored]);
        assert!(s.attachments_error.is_none());
        assert!(s.attachment_preview.is_none());
        assert!(s.rewind_armed.is_empty());
        assert!(!s.live);
        assert!(!s.aborting);
        assert!(s.run_started_ms.is_none());
        assert!(s.refresh_list);
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
