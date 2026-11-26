//! Desktop presence: bridges the shell's chat/voice state to the native
//! Stark Island, the tray icon, and the background lifecycle.
//!
//! The shell owns truth. This module only:
//! - drains island/tray/shortcut requests into ordinary shell actions;
//! - notices finished voice utterances and finished runs;
//! - publishes a small immutable [`IslandModel`] to the overlay;
//! - keeps the tray menu/tooltip and close-to-tray behavior in sync.

use std::collections::HashMap;
use std::time::Duration;

use rune::Tx;

use crate::island::{
    IslandAttention, IslandAttentionKind, IslandCommand, IslandLink, IslandModel, IslandRun,
    IslandTarget,
};
use crate::rune_transcript::{stage_abort, stage_send, RuneTranscriptState};
use crate::shell::ShellState;

/// Everything the native presence layer remembers between frames.
#[derive(Clone, Default)]
pub struct PresenceState {
    /// Native island handle; `None` on web and in unit tests.
    pub link: Option<IslandLink>,
    /// App handle for opening the main window from the island/tray/shortcut.
    pub app: Option<rune::AppHandle<ShellState>>,
    pub listen_requested: bool,
    pub open_requested: bool,
    last_final: HashMap<String, u64>,
    last_live: HashMap<String, bool>,
    /// Newest completion/draft/error card. Pending questions are read live
    /// from each thread and take priority over this.
    toast: Option<IslandAttention>,
    island_visible: Option<bool>,
    capture_excluded: Option<bool>,
    close_hint_shown: bool,
    /// Whether the speech models are currently warm. Flipped by the
    /// reconcile step so one warm/release pair covers any number of arm
    /// toggles across threads.
    models_warm: bool,
}

impl std::fmt::Debug for PresenceState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PresenceState")
            .field("has_link", &self.link.is_some())
            .field("has_app", &self.app.is_some())
            .field("listen_requested", &self.listen_requested)
            .field("open_requested", &self.open_requested)
            .field("island_visible", &self.island_visible)
            .finish()
    }
}

/// All thread keys the shell can address, in stable order.
pub fn thread_keys(s: &ShellState) -> Vec<String> {
    let mut keys = vec!["main".to_string(), "personal".to_string()];
    keys.extend(s.canvas.sessions.order.iter().cloned());
    keys
}

pub fn thread_ref<'a>(s: &'a ShellState, key: &str) -> Option<&'a RuneTranscriptState> {
    match key {
        "main" => Some(&s.transcript),
        "personal" => Some(&s.personal),
        _ => s.canvas.sessions.threads.get(key),
    }
}

pub fn thread_mut<'a>(s: &'a mut ShellState, key: &str) -> Option<&'a mut RuneTranscriptState> {
    match key {
        "main" => Some(&mut s.transcript),
        "personal" => Some(&mut s.personal),
        _ => s.canvas.sessions.threads.get_mut(key),
    }
}

fn title_of(s: &ShellState, key: &str, thread: &RuneTranscriptState) -> String {
    let mut title = if !thread.title.trim().is_empty() {
        thread.title.clone()
    } else if let Some(card) = s.canvas.card(key) {
        card.title.clone()
    } else {
        match key {
            "personal" => "Personal".to_string(),
            "main" => "General".to_string(),
            _ => "New Chat".to_string(),
        }
    };
    if title.trim().is_empty() || title == "New Chat" {
        if let Some(prompt) = thread
            .rows
            .iter()
            .rev()
            .find(|row| row.role == "user" && !row.body.trim().is_empty())
            .map(|row| row.body.trim())
        {
            title = prompt
                .lines()
                .next()
                .unwrap_or(prompt)
                .chars()
                .take(42)
                .collect();
            if prompt.chars().count() > 42 {
                title.push('…');
            }
        }
    }
    title
}

fn target_for(s: &ShellState, key: &str) -> Option<IslandTarget> {
    let thread = thread_ref(s, key)?;
    let card_id = key.starts_with("chat:").then(|| key.to_string());
    let (mut workspace_id, mut chat_id) = (thread.workspace_id.clone(), thread.chat_id.clone());
    if (workspace_id.is_empty() || chat_id.is_empty()) && key.starts_with("chat:") {
        if let Some((ws, chat)) = key.trim_start_matches("chat:").split_once(':') {
            if !ws.starts_with("local:") {
                workspace_id = ws.to_string();
                chat_id = chat.to_string();
            }
        }
    }
    Some(IslandTarget {
        thread_key: key.to_string(),
        title: title_of(s, key, thread),
        card_id,
        workspace_id,
        chat_id,
    })
}

fn active_target(s: &ShellState) -> IslandTarget {
    if let Some(id) = s.canvas.sessions.sheet.as_deref() {
        if let Some(target) = target_for(s, id) {
            return target;
        }
    }
    target_for(s, "main").unwrap_or_default()
}

fn active_key(s: &ShellState) -> String {
    if let Some(id) = s.canvas.sessions.sheet.as_deref() {
        if thread_ref(s, id).is_some() {
            return id.to_string();
        }
    }
    "main".to_string()
}

fn target_elapsed(thread: &RuneTranscriptState, now_ms: f64) -> String {
    let Some(start) = thread.run_started_ms else {
        // A run adopted from the server has no local start stamp. Empty is
        // more honest than a fabricated 0:00.
        return String::new();
    };
    let elapsed = (now_ms - start).max(0.0) / 1000.0;
    let mins = (elapsed / 60.0).floor() as u64;
    let secs = (elapsed % 60.0).floor() as u64;
    format!("{mins}:{secs:02}")
}

fn activity_of(thread: &RuneTranscriptState) -> String {
    let row = thread
        .rows
        .iter()
        .rev()
        .find(|row| row.is_streaming)
        .or_else(|| thread.rows.last());
    if let Some(row) = row {
        let segment = row
            .timeline
            .last()
            .or_else(|| row.accum.as_ref().and_then(|accum| accum.segments.last()))
            .or_else(|| row.activity.last());
        if let Some(segment) = segment {
            match segment {
                crate::stream_accum::StreamSegment::Tool { ev } => {
                    let verb = crate::tool_labels::verb(&ev.name);
                    let arg = crate::tool_labels::primary_arg(ev.args.as_deref());
                    if arg.is_empty() {
                        return verb.to_string();
                    }
                    return format!("{verb} {arg}");
                }
                crate::stream_accum::StreamSegment::Reasoning { .. } => {
                    return "Thinking".to_string();
                }
                crate::stream_accum::StreamSegment::Text { .. } => {
                    return "Writing".to_string();
                }
            }
        }
        if !row.agent_name.trim().is_empty() {
            return format!("{} · working", row.agent_name.trim());
        }
    }
    "Working".to_string()
}

fn collect_runs(s: &ShellState, now_ms: f64) -> Vec<IslandRun> {
    let active = active_key(s);
    let mut runs = Vec::new();
    for key in thread_keys(s) {
        let Some(thread) = thread_ref(s, &key) else {
            continue;
        };
        if !thread.live {
            continue;
        }
        let Some(target) = target_for(s, &key) else {
            continue;
        };
        runs.push(IslandRun {
            target,
            activity: activity_of(thread),
            elapsed: target_elapsed(thread, now_ms),
            stoppable: true,
        });
    }

    // A cloud run has no local transcript. Show it when the active run is not
    // already represented by a local live thread.
    let cloud_running = s.chats.active_run.as_ref().is_some_and(|run| run.running);
    if cloud_running {
        if let Some(run) = &s.chats.active_run {
            if !runs
                .iter()
                .any(|row| row.target.chat_id == run.chat_id && !row.target.chat_id.is_empty())
            {
                let row = s.canvas.chat_rows.iter().find(|row| row.id == run.chat_id);
                let key = row.map(|row| row.key()).unwrap_or_default();
                let target = IslandTarget {
                    thread_key: key.clone(),
                    title: row
                        .map(|row| row.title.clone())
                        .filter(|title| !title.trim().is_empty())
                        .unwrap_or_else(|| "Cloud run".to_string()),
                    card_id: (!key.is_empty()).then(|| format!("chat:{key}")),
                    workspace_id: key
                        .split_once(':')
                        .map(|(ws, _)| ws.to_string())
                        .unwrap_or_default(),
                    chat_id: run.chat_id.clone(),
                };
                runs.push(IslandRun {
                    target,
                    activity: "Running on the VPS".to_string(),
                    elapsed: String::new(),
                    stoppable: false,
                });
            }
        }
    }

    // The active chat leads the list; the rest keep the shell's stable order.
    runs.sort_by_key(|row| row.target.thread_key != active);
    runs
}

fn pending_question(s: &ShellState) -> Option<IslandAttention> {
    for key in thread_keys(s) {
        let Some(thread) = thread_ref(s, &key) else {
            continue;
        };
        let Some(batch) =
            thread.ask.batches.iter().find(|batch| {
                thread.ask.dismissed_batch.as_deref() != Some(batch.batch_id.as_str())
            })
        else {
            continue;
        };
        let body = batch
            .questions
            .first()
            .map(|question| question.question.clone())
            .unwrap_or_else(|| "The agent is waiting for your answer.".to_string());
        return Some(IslandAttention {
            kind: IslandAttentionKind::Question,
            title: "Needs you".to_string(),
            body,
            target: target_for(s, &key)?,
            expires_ms: 0,
        });
    }
    None
}

fn voice_note(s: &ShellState) -> Option<IslandAttention> {
    for key in thread_keys(s) {
        let Some(thread) = thread_ref(s, &key) else {
            continue;
        };
        let Some(note) = thread.voice.note.clone() else {
            continue;
        };
        // cpal's buffer error is technically true and useless in a two-line
        // card. Keep the actionable copy; the detail remains in the log.
        let body = if note.starts_with("mic:") {
            "Microphone interrupted — press Ctrl+Space to retry".to_string()
        } else {
            note
        };
        return Some(IslandAttention {
            kind: IslandAttentionKind::Error,
            title: "Microphone".to_string(),
            body,
            target: target_for(s, &key)?,
            expires_ms: 0,
        });
    }
    None
}

/// Whether any thread has live mode armed (models warm, island up, mic off).
fn any_armed(s: &ShellState) -> bool {
    thread_keys(s)
        .iter()
        .any(|key| thread_ref(s, key).is_some_and(|thread| thread.voice.armed || thread.voice.live))
}

/// Whether any thread is actively capturing audio right now.
fn any_live_voice(s: &ShellState) -> bool {
    thread_keys(s)
        .iter()
        .any(|key| thread_ref(s, key).is_some_and(|thread| thread.voice.live))
}

/// Voice-owned attention: interim errors, drafts and voice-sent finals. Run
/// completions, failures, questions and the close hint belong to the tray,
/// never the island — live mode is the island's only trigger.
fn is_voice_attention(attention: &IslandAttention) -> bool {
    match attention.kind {
        IslandAttentionKind::Draft => true,
        IslandAttentionKind::Done => attention.title == "Voice sent",
        IslandAttentionKind::Error => attention.title == "Microphone",
        IslandAttentionKind::Question => false,
    }
}

struct VoiceView {
    target: IslandTarget,
    live: bool,
    level: f32,
    text: String,
}

fn voice_view(s: &ShellState) -> VoiceView {
    let active = active_key(s);
    // Prefer the active bus, then any other live bus, then the active bus's
    // idle state. This keeps the island glued to the chat the user sees.
    if let Some(thread) = thread_ref(s, &active) {
        if thread.voice.live {
            return VoiceView {
                target: target_for(s, &active).unwrap_or_default(),
                live: true,
                level: thread.voice.level,
                text: thread.voice.live_text.clone(),
            };
        }
    }
    for key in thread_keys(s) {
        let Some(thread) = thread_ref(s, &key) else {
            continue;
        };
        if thread.voice.live {
            return VoiceView {
                target: target_for(s, &key).unwrap_or_default(),
                live: true,
                level: thread.voice.level,
                text: thread.voice.live_text.clone(),
            };
        }
    }
    let thread = thread_ref(s, &active);
    VoiceView {
        target: target_for(s, &active).unwrap_or_default(),
        live: false,
        level: thread.map(|t| t.voice.level).unwrap_or(0.0),
        text: thread
            .map(|t| t.voice.live_text.clone())
            .unwrap_or_default(),
    }
}

fn snapshot(s: &ShellState, now_ms: u64) -> IslandModel {
    let runs = collect_runs(s, crate::clock::monotonic_millis());
    let voice = voice_view(s);
    let toast = s
        .presence
        .toast
        .clone()
        .filter(|toast| toast.expires_ms == 0 || toast.expires_ms > now_ms);
    let attention = pending_question(s).or_else(|| voice_note(s)).or(toast);
    let target = if voice.target.thread_key.is_empty() {
        active_target(s)
    } else {
        voice.target
    };
    // Live mode is the island's only trigger, and arming alone raises it:
    // the dock arms (warms models, shows the island idle) while only the
    // island's mic — or Ctrl+Space while armed — starts capture. Agent runs
    // and questions stay on the tray, never the island.
    let voice_card = attention.as_ref().is_some_and(is_voice_attention);
    let armed = any_armed(s);
    let live_now = voice.live || voice_card || armed;
    let tick_step = if voice.live { 80 } else { 1000 };
    IslandModel {
        runs,
        listening: voice.live,
        armed,
        level: voice.level,
        voice_text: voice.text,
        target,
        attention,
        auto_send_voice: s.settings.live.auto_send_voice,
        island_enabled: s.settings.live.island,
        // Keep the waveform breathing while live and the draft card ticking
        // while it is up, without a second timer in the overlay.
        tick_ms: if live_now {
            now_ms / tick_step * tick_step
        } else {
            0
        },
    }
}

fn open_main(s: &ShellState) {
    if let Some(app) = &s.presence.app {
        app.open();
    }
}

fn open_target(s: &mut ShellState, target: &IslandTarget) {
    if let Some(card_id) = &target.card_id {
        if s.canvas.card(card_id).is_some() {
            s.canvas.select_card(card_id);
        } else if !target.workspace_id.is_empty() && !target.chat_id.is_empty() {
            s.canvas
                .open_chat_row(&target.workspace_id, &target.chat_id);
        }
    } else if target.thread_key != "main"
        && target.thread_key != "personal"
        && !target.workspace_id.is_empty()
        && !target.chat_id.is_empty()
    {
        s.canvas
            .open_chat_row(&target.workspace_id, &target.chat_id);
    }
    open_main(s);
}

fn stop_all_voice(s: &mut ShellState) {
    // Emergency stop: every capture ends, but arming survives — the models
    // stay warm and the island drops back to idle, not away.
    for key in thread_keys(s) {
        if let Some(thread) = thread_mut(s, &key) {
            if thread.voice.live {
                thread.voice.stop_capture();
            }
        }
    }
}

/// Capture toggle for the island's mic (and Ctrl+Space while armed). If any
/// bus is live, one press stops it — the island never leaves a hot
/// microphone behind. Otherwise the active thread starts capturing.
fn toggle_capture(s: &mut ShellState) {
    for key in thread_keys(s) {
        let live = thread_ref(s, &key).is_some_and(|thread| thread.voice.live);
        if live {
            if let Some(thread) = thread_mut(s, &key) {
                thread.voice.stop_capture();
                thread.voice.note = None;
            }
            return;
        }
    }
    let key = active_key(s);
    if let Some(thread) = thread_mut(s, &key) {
        thread.voice.note = None;
        thread.voice.start_capture();
    }
}

/// Arm live mode on the active thread: models warm, island rises idle, mic
/// stays off. Ctrl+Space lands here while disarmed (arm-only by design).
fn arm_active(s: &mut ShellState) {
    let key = active_key(s);
    if let Some(thread) = thread_mut(s, &key) {
        thread.voice.note = None;
        thread.voice.set_armed(true);
    }
}

/// Disarm every bus and release the models. Only quit (and the dock's own
/// off press, per thread) goes here — closing the window keeps live mode.
fn disarm_all(s: &mut ShellState) {
    for key in thread_keys(s) {
        if let Some(thread) = thread_mut(s, &key) {
            thread.voice.set_armed(false);
        }
    }
    crate::voice::VoiceBus::release_models();
    s.presence.models_warm = false;
}

/// One warm/release pair for any number of armed buses. Arming warms
/// eagerly per bus (idempotent POSTs); this flag keeps the release side to
/// the single moment the last bus goes quiet.
fn reconcile_models(s: &mut ShellState) {
    let need = any_armed(s) || any_live_voice(s);
    if need && !s.presence.models_warm {
        crate::voice::VoiceBus::ensure_warmed();
        s.presence.models_warm = true;
    } else if !need && s.presence.models_warm {
        crate::voice::VoiceBus::release_models();
        s.presence.models_warm = false;
    }
}

struct FinalEvent {
    key: String,
    seq: u64,
    target: IslandTarget,
}

fn collect_final_events(s: &ShellState) -> Vec<FinalEvent> {
    let mut events = Vec::new();
    for key in thread_keys(s) {
        let Some(thread) = thread_ref(s, &key) else {
            continue;
        };
        let seq = thread.voice.final_seq;
        let last = s.presence.last_final.get(&key).copied().unwrap_or(0);
        if seq > last {
            if let Some(target) = target_for(s, &key) {
                events.push(FinalEvent { key, seq, target });
            }
        }
    }
    events
}

struct EndedRun {
    target: IslandTarget,
    error: bool,
    stopped: bool,
}

fn collect_ended_runs(s: &ShellState) -> Vec<EndedRun> {
    let mut ended = Vec::new();
    for key in thread_keys(s) {
        let Some(thread) = thread_ref(s, &key) else {
            continue;
        };
        let live = thread.live;
        let previous = s.presence.last_live.get(&key).copied();
        if previous == Some(true) && !live {
            let error = thread.rows.last().is_some_and(|row| row.role == "error");
            let stopped = thread.rows.last().is_some_and(|row| row.stopped)
                || thread.rows.iter().any(|row| row.event == "aborted");
            if let Some(target) = target_for(s, &key) {
                ended.push(EndedRun {
                    target,
                    error,
                    stopped,
                });
            }
        }
    }
    ended
}

fn update_live_tracking(s: &mut ShellState) {
    for key in thread_keys(s) {
        let live = thread_ref(s, &key).is_some_and(|thread| thread.live);
        s.presence.last_live.insert(key, live);
    }
}

fn handle_commands(s: &mut ShellState) -> bool {
    let Some(link) = s.presence.link.clone() else {
        return false;
    };
    let commands = link.take_commands();
    if commands.is_empty() {
        return false;
    }
    for command in commands {
        match command {
            IslandCommand::ToggleListen => toggle_capture(s),
            IslandCommand::StopListen => stop_all_voice(s),
            IslandCommand::OpenMain => open_main(s),
            IslandCommand::OpenChat(target) => open_target(s, &target),
            IslandCommand::StopRun(key) => {
                if let Some(thread) = thread_mut(s, &key) {
                    stage_abort(thread);
                }
            }
            IslandCommand::SendDraft(key) => {
                let target = target_for(s, &key);
                if let Some(thread) = thread_mut(s, &key) {
                    if stage_send(thread) {
                        if let Some(target) = target {
                            s.presence.toast = Some(IslandAttention {
                                kind: IslandAttentionKind::Done,
                                title: "Voice sent".to_string(),
                                body: format!("Sent to {}", target.display()),
                                target,
                                expires_ms: crate::clock::now_millis() + 2_500,
                            });
                        }
                    }
                }
            }
            IslandCommand::DismissVoiceNote(key) => {
                if let Some(thread) = thread_mut(s, &key) {
                    thread.voice.note = None;
                }
            }
        }
    }
    true
}

fn apply_finals(s: &mut ShellState, now_ms: u64, auto_send: bool) -> bool {
    let events = collect_final_events(s);
    if events.is_empty() {
        return false;
    }
    for event in events {
        s.presence.last_final.insert(event.key.clone(), event.seq);
        let mut sent = false;
        if auto_send {
            if let Some(thread) = thread_mut(s, &event.key) {
                if !thread.live && !thread.aborting {
                    sent = stage_send(thread);
                }
            }
        }
        s.presence.toast = Some(IslandAttention {
            kind: if sent {
                IslandAttentionKind::Done
            } else {
                IslandAttentionKind::Draft
            },
            title: if sent {
                "Voice sent".to_string()
            } else {
                "Voice draft ready".to_string()
            },
            body: if sent {
                format!("Sent to {}", event.target.display())
            } else {
                format!("Open {} to review", event.target.display())
            },
            target: event.target,
            expires_ms: now_ms + 4_000,
        });
    }
    true
}

fn apply_ended_runs(s: &mut ShellState, now_ms: u64) -> bool {
    let ended = collect_ended_runs(s);
    if ended.is_empty() {
        return false;
    }
    for run in ended {
        let kind = if run.error {
            IslandAttentionKind::Error
        } else {
            IslandAttentionKind::Done
        };
        let body = if run.error {
            "Run failed — open for details".to_string()
        } else if run.stopped {
            "Stopped — open to review".to_string()
        } else {
            "Finished — open to review".to_string()
        };
        s.presence.toast = Some(IslandAttention {
            kind,
            title: if run.error {
                "Run failed".to_string()
            } else if run.stopped {
                "Stopped".to_string()
            } else {
                "Done".to_string()
            },
            body,
            target: run.target,
            expires_ms: now_ms + if run.error { 8_000 } else { 4_500 },
        });
    }
    true
}

fn apply_requests(s: &mut ShellState) -> bool {
    let mut changed = false;
    if s.presence.listen_requested {
        s.presence.listen_requested = false;
        // Arm-only while disarmed: the shortcut raises the idle island and
        // warms the models. A second press (or the island mic) captures.
        if any_armed(s) || any_live_voice(s) {
            toggle_capture(s);
        } else {
            arm_active(s);
        }
        changed = true;
    }
    if s.presence.open_requested {
        s.presence.open_requested = false;
        open_main(s);
        changed = true;
    }
    changed |= handle_commands(s);
    changed
}

/// Arming alone raises the island: the overlay exists while any bus is
/// armed, while voice is live, or while a voice card (draft/error) is up.
/// Agent runs and questions stay on the tray, never the island.
fn island_desired(prefs_island: bool, model: &IslandModel) -> bool {
    let live_now =
        model.armed || model.listening || model.attention.as_ref().is_some_and(is_voice_attention);
    prefs_island && live_now
}

fn publish(s: &mut ShellState, model: &IslandModel) {
    let desired = island_desired(s.settings.live.island, model);
    if let Some(link) = s.presence.link.clone() {
        if s.presence.island_visible != Some(desired) {
            if desired {
                link.show();
            } else {
                link.hide();
            }
            s.presence.island_visible = Some(desired);
        }
        let exclude = s.settings.live.exclude_from_capture;
        if s.presence.capture_excluded != Some(exclude) {
            link.set_capture_exclusion(exclude);
            s.presence.capture_excluded = Some(exclude);
        }
        link.publish(model.clone());
    }
    #[cfg(not(target_arch = "wasm32"))]
    crate::tray::sync(model, &s.settings.live);
}

/// Run once per shell poll, after chat/voice inboxes have been drained.
pub fn tick(s: &mut ShellState, _clock: f64, tx: &mut Tx) -> bool {
    let now_ms = crate::clock::now_millis();
    let mut changed = apply_requests(s);
    let auto_send = s.settings.live.auto_send_voice;
    changed |= apply_finals(s, now_ms, auto_send);
    changed |= apply_ended_runs(s, now_ms);
    update_live_tracking(s);
    reconcile_models(s);

    if s.presence
        .toast
        .as_ref()
        .is_some_and(|toast| toast.expires_ms != 0 && toast.expires_ms <= now_ms)
    {
        s.presence.toast = None;
        changed = true;
    }

    let model = snapshot(s, now_ms);
    publish(s, &model);
    let island_up =
        model.armed || model.listening || model.attention.as_ref().is_some_and(is_voice_attention);
    if model.listening {
        // Keep the mic bars alive even between level packets.
        tx.wake_after(Duration::from_millis(80));
    }
    if island_up && (!model.runs.is_empty() || model.attention.is_some() || model.armed) {
        // Elapsed labels, expiring voice cards and the idle island tick
        // while the island is up.
        tx.wake_after(Duration::from_secs(1));
    }
    changed
}

/// Stop capture when the window is hidden/closed, but keep live mode armed:
/// the island is the live surface even with no chat window open, and the
/// models stay warm. Quitting disarms (see `on_quit`).
pub fn on_close(s: &mut ShellState, tx: &mut Tx) {
    stop_all_voice(s);
    // Close hides; agents keep working. Tell the user once per session where
    // the app went, without opening a modal over the hidden window.
    if !s.presence.close_hint_shown {
        s.presence.close_hint_shown = true;
        s.presence.toast = Some(IslandAttention {
            kind: IslandAttentionKind::Done,
            title: "Still running".to_string(),
            body: "Stark is in the tray — reopen any time".to_string(),
            target: active_target(s),
            expires_ms: crate::clock::now_millis() + 5_000,
        });
    }
    crate::prefs::save(&s.settings.live);
    // Publish the reassurance card on the hidden loop, not at the next
    // two-and-a-half-second chat poll.
    tx.wake_after(Duration::from_millis(40));
}

/// Quitting is the only action that ends background agents. Disarm every
/// bus and release the models, then persist the island choices — an idle
/// app holds no speech memory. Run state is left to the server's shutdown.
pub fn on_quit(s: &mut ShellState, _tx: &mut Tx) {
    disarm_all(s);
    crate::prefs::save(&s.settings.live);
}

pub fn shortcut_listen(s: &mut ShellState, tx: &mut Tx) {
    s.presence.listen_requested = true;
    tx.instant();
}

pub fn shortcut_open(s: &mut ShellState, tx: &mut Tx) {
    s.presence.open_requested = true;
    tx.instant();
}

/// Second launches must focus the existing process instead of showing the
/// launcher's old "already running" fatal dialog. The launcher sets this
/// manual-reset event; the UI waits on it and calls `AppHandle::open`.
#[cfg(windows)]
pub fn install_reopen_listener(handle: rune::AppHandle<ShellState>) {
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
    let name: Vec<u16> = "Local\\StarkRuneOpen\0".encode_utf16().collect();
    let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) };
    if event.is_null() {
        return;
    }
    // Win32 HANDLE is a pointer; carry it as an integer so the waiter thread
    // is `Send` (the event itself is process-wide and lives until exit).
    let event = event as isize;
    std::thread::Builder::new()
        .name("stark-reopen".into())
        .spawn(move || loop {
            let result = unsafe { WaitForSingleObject(event as *mut _, u32::MAX) };
            // WAIT_OBJECT_0; any other result means the event is gone.
            if result != 0 {
                break;
            }
            handle.open();
        })
        .ok();
}

#[cfg(not(windows))]
pub fn install_reopen_listener(_handle: rune::AppHandle<ShellState>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rune_transcript::RuneTranscriptState;

    fn shell() -> ShellState {
        let mut s = ShellState::default();
        s.chats.auto = false;
        s.canvas.cards.clear();
        s.canvas.sessions.order.clear();
        s.canvas.sessions.sheet = None;
        s.transcript.live = false;
        s
    }

    #[test]
    fn thread_keys_include_main_personal_and_canvas_order() {
        let mut s = shell();
        s.canvas.sessions.order = vec!["chat:ws:c1".to_string()];
        assert_eq!(
            thread_keys(&s),
            vec![
                "main".to_string(),
                "personal".to_string(),
                "chat:ws:c1".to_string()
            ]
        );
    }

    #[test]
    fn a_finished_voice_stages_and_reports_a_draft_without_auto_send() {
        let mut s = shell();
        s.transcript.voice.final_seq = 1;
        s.transcript.edit.replace_text("hello island".to_string());
        assert!(apply_finals(&mut s, 10, false));
        assert_eq!(s.transcript.edit.text, "hello island");
        let toast = s.presence.toast.as_ref().unwrap();
        assert_eq!(toast.kind, IslandAttentionKind::Draft);
        assert!(toast.title.contains("draft"));
    }

    #[test]
    fn auto_send_clears_the_draft_and_sends_it() {
        let mut s = shell();
        s.canvas
            .sessions
            .threads
            .insert("chat:ws:c1".to_string(), RuneTranscriptState::screenshot());
        s.canvas.sessions.order = vec!["chat:ws:c1".to_string()];
        let thread = s.canvas.sessions.threads.get_mut("chat:ws:c1").unwrap();
        thread.live = false;
        thread.voice.final_seq = 1;
        thread.chat_ready = true;
        thread.chat_id = "c1".to_string();
        thread.workspace_id = "ws".to_string();
        thread.edit.replace_text("ship it".to_string());
        assert!(apply_finals(&mut s, 10, true));
        let thread = s.canvas.sessions.threads.get("chat:ws:c1").unwrap();
        assert!(thread.edit.text.is_empty(), "stage_send clears the draft");
        assert!(thread.live || thread.aborting, "the run was staged");
        assert_eq!(
            s.presence.toast.as_ref().unwrap().title,
            "Voice sent".to_string()
        );
    }

    #[test]
    fn a_finished_run_raises_done_and_never_the_first_idle_poll() {
        let mut s = shell();
        update_live_tracking(&mut s);
        s.transcript.live = true;
        update_live_tracking(&mut s);
        s.transcript.live = false;
        assert!(apply_ended_runs(&mut s, 10));
        assert_eq!(
            s.presence.toast.as_ref().unwrap().kind,
            IslandAttentionKind::Done
        );
    }

    #[test]
    fn listening_prefers_the_active_bus_and_stop_all_ends_every_bus() {
        let mut s = shell();
        s.transcript.voice.live = true;
        s.transcript.voice.level = 0.5;
        assert!(voice_view(&s).live);
        assert_eq!(voice_view(&s).target.thread_key, "main");
        stop_all_voice(&mut s);
        assert!(!s.transcript.voice.live);
    }

    #[test]
    fn island_model_exposes_the_compact_status_and_auto_send_preference() {
        let mut s = shell();
        s.settings.live.auto_send_voice = true;
        s.transcript.live = true;
        s.transcript.run_started_ms = Some(crate::clock::monotonic_millis() - 42_000.0);
        let model = snapshot(&s, crate::clock::now_millis());
        assert_eq!(model.runs.len(), 1);
        assert!(model.runs[0].elapsed.starts_with("0:4"));
        assert!(model.auto_send_voice);
        assert_eq!(model.target.thread_key, "main");
    }

    #[test]
    fn island_stays_hidden_for_agent_runs_and_shows_for_live_voice() {
        use crate::island::{IslandAttention, IslandAttentionKind, IslandModel, IslandTarget};

        let target = IslandTarget::default();
        // Agent running, mic off, disarmed: tray material only, island stays down.
        let mut running = IslandModel::default();
        running.runs.push(crate::island::IslandRun {
            target: target.clone(),
            activity: "Working".to_string(),
            elapsed: "0:01".to_string(),
            stoppable: true,
        });
        assert!(
            !island_desired(true, &running),
            "agent runs never raise the island"
        );

        // Armed but idle: the island is up with no capture and no runs.
        let armed = IslandModel {
            armed: true,
            ..IslandModel::default()
        };
        assert!(
            island_desired(true, &armed),
            "arming raises the idle island"
        );

        // Mic live: island up even with no runs.
        let listening = IslandModel {
            listening: true,
            level: 0.5,
            ..IslandModel::default()
        };
        assert!(island_desired(true, &listening));

        // Voice draft outlives capture: the card keeps the island up.
        let draft = IslandModel {
            attention: Some(IslandAttention {
                kind: IslandAttentionKind::Draft,
                title: "Voice draft ready".to_string(),
                body: "Open main to review".to_string(),
                target: target.clone(),
                expires_ms: crate::clock::now_millis() + 4_000,
            }),
            ..IslandModel::default()
        };
        assert!(island_desired(true, &draft));

        // Run completions and questions stay on the tray.
        for (kind, title) in [
            (IslandAttentionKind::Done, "Done"),
            (IslandAttentionKind::Error, "Run failed"),
            (IslandAttentionKind::Question, "Needs you"),
        ] {
            let other = IslandModel {
                attention: Some(IslandAttention {
                    kind,
                    title: title.to_string(),
                    body: "open to review".to_string(),
                    target: target.clone(),
                    expires_ms: 0,
                }),
                ..IslandModel::default()
            };
            assert!(
                !island_desired(true, &other),
                "only voice cards raise the island"
            );
        }

        // Master switch still gates everything.
        assert!(!island_desired(false, &listening));
        assert!(!island_desired(false, &armed));
    }

    #[test]
    fn shortcut_arms_only_then_captures() {
        let mut s = shell();
        // First press while disarmed: arm only, mic stays shut.
        s.presence.listen_requested = true;
        apply_requests(&mut s);
        assert!(s.transcript.voice.armed, "shortcut arms live mode");
        assert!(
            !s.transcript.voice.live,
            "arming never opens the mic: {:?}",
            s.transcript.voice
        );
        // Second press while armed: the island mic path — toggles capture.
        s.presence.listen_requested = true;
        apply_requests(&mut s);
        // No microphone on CI: capture fails fast with an honest note, but
        // the bus stays armed so the island (and its retry) stay up.
        assert!(
            s.transcript.voice.armed,
            "capture never disarms: {:?}",
            s.transcript.voice
        );
    }

    #[test]
    fn capture_stop_keeps_the_island_up_while_armed() {
        let mut s = shell();
        s.transcript.voice.set_armed(true);
        s.transcript.voice.live = true;
        stop_all_voice(&mut s);
        assert!(!s.transcript.voice.live);
        let model = snapshot(&s, crate::clock::now_millis());
        assert!(model.armed);
        assert!(
            island_desired(true, &model),
            "dropping to idle keeps the island, not hides it"
        );
    }

    #[test]
    fn snapshot_ticks_only_while_live_voice_or_voice_cards() {
        let mut s = shell();
        // Agent run alone: collected for the tray, but no island tick.
        s.transcript.live = true;
        let idle_tick = snapshot(&s, 10_000).tick_ms;
        assert_eq!(idle_tick, 0, "runs alone must not animate the island");

        // Mic live: the waveform clock runs.
        s.transcript.live = false;
        s.transcript.voice.live = true;
        let live_tick = snapshot(&s, 10_000).tick_ms;
        assert_eq!(live_tick, 10_000);

        // Armed idle: the island ticks (cockpit hover clock) without capture.
        s.transcript.voice.live = false;
        s.transcript.voice.armed = true;
        let model = snapshot(&s, 10_000);
        assert!(model.armed);
        assert_eq!(model.tick_ms, 10_000);
        assert!(!model.listening);
    }
}
