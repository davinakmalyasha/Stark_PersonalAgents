//! Inline `ask_user_question` panel — port of `AskQuestionPanel.tsx` +
//! `usePendingAsk.ts` (via `askDock.css`).
//!
//! Contract (verified against `frontend/src/services/api/stream.ts` and
//! `backend-rs/crates/stark-tools/src/questions.rs`):
//! - `GET /api/ask/pending` → `{batches:[{batch_id,chat_id,questions,created}]}`,
//!   polled every 1.5s while the chat is on screen.
//! - `POST /api/ask/answer` with `{batch_id, answers:[{id,selected,custom}]}`.
//! - A non-200 (unknown/expired batch) means delivery failed; the agent keeps
//!   waiting until its own timeout.
//!
//! Dismiss is client-only, exactly like React: the batch stays pending for the
//! agent (voice or another surface may still answer it). The panel replaces
//! the prompt dock for the owning chat, never a global overlay.

use crate::keys::EnterPress;
use rune::{col, row, text, text_input, El, ScrollSpec, TextEdit};

use crate::rune_transcript::{RuneTranscriptState, Thread, TranscriptHost};

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

fn rgba(r: f32, g: f32, b: f32, a: f32) -> rune::Color {
    rune::Color {
        r: r / 255.0,
        g: g / 255.0,
        b: b / 255.0,
        a,
    }
}

/// `usePendingAsk.ts` interval.
pub const POLL_MS: u64 = 1500;

/// One selectable answer. React accepts either a bare string or
/// `{label, description}`.
#[derive(Debug, Clone, PartialEq)]
pub struct AskOption {
    pub label: String,
    pub description: Option<String>,
}

impl AskOption {
    pub fn from_json(v: &serde_json::Value) -> Self {
        if let Some(label) = v.as_str() {
            return Self {
                label: label.to_string(),
                description: None,
            };
        }
        Self {
            label: v
                .get("label")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            description: v
                .get("description")
                .and_then(|x| x.as_str())
                .map(ToString::to_string),
        }
    }
}

/// One question in a batch.
#[derive(Debug, Clone, PartialEq)]
pub struct AskQuestion {
    pub id: String,
    pub question: String,
    pub detail: Option<String>,
    /// `multiSelect` (camel) or `multi_select` (snake).
    pub multi: bool,
    pub options: Vec<AskOption>,
}

impl AskQuestion {
    pub fn from_json(v: &serde_json::Value) -> Self {
        let str_of = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let multi = v
            .get("multiSelect")
            .and_then(|x| x.as_bool())
            .or_else(|| v.get("multi_select").and_then(|x| x.as_bool()))
            .unwrap_or(false);
        let detail = v
            .get("detail")
            .and_then(|x| x.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(ToString::to_string);
        Self {
            id: str_of("id"),
            question: str_of("question"),
            detail,
            multi,
            options: v
                .get("options")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().map(AskOption::from_json).collect())
                .unwrap_or_default(),
        }
    }
}

/// A pending question batch; `chat_id` routes it to its owning chat.
#[derive(Debug, Clone, PartialEq)]
pub struct AskBatch {
    pub batch_id: String,
    pub chat_id: String,
    pub questions: Vec<AskQuestion>,
    pub created: i64,
}

impl AskBatch {
    pub fn from_json(v: &serde_json::Value) -> Self {
        let str_of = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        Self {
            batch_id: str_of("batch_id"),
            chat_id: str_of("chat_id"),
            questions: v
                .get("questions")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().map(AskQuestion::from_json).collect())
                .unwrap_or_default(),
            created: v.get("created").and_then(|x| x.as_i64()).unwrap_or(0),
        }
    }

    /// Parse `{batches:[…]}` (an absent/blank list is an empty poll result).
    pub fn list_from_json(v: &serde_json::Value) -> Vec<AskBatch> {
        v.get("batches")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().map(AskBatch::from_json).collect())
            .unwrap_or_default()
    }
}

/// Per-thread ask UI state. Answers are keyed by question id because one
/// batch is visible at a time per chat (`useAskBatchForChat`).
pub struct AskState {
    pub batches: Vec<AskBatch>,
    /// Batch hidden for this client session; cleared once it leaves the server
    /// list so a later batch with the same id can never be masked.
    pub dismissed_batch: Option<String>,
    /// The batch the form fields below belong to.
    pub bound_batch: Option<String>,
    pub answers: std::collections::HashMap<String, Vec<String>>,
    pub custom_open: std::collections::HashSet<String>,
    pub custom_edits: std::collections::HashMap<String, TextEdit>,
    pub collapsed: bool,
    pub submitting: bool,
    pub error: Option<String>,
    pub poll_inbox: Option<std::sync::mpsc::Receiver<Result<Vec<AskBatch>, String>>>,
    /// Successful result carries the answered batch id.
    pub submit_inbox: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    pub last_poll: f64,
}

impl std::fmt::Debug for AskState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AskState")
            .field("batches", &self.batches.len())
            .field("dismissed_batch", &self.dismissed_batch)
            .field("bound_batch", &self.bound_batch)
            .field("answers", &self.answers.len())
            .field("collapsed", &self.collapsed)
            .field("submitting", &self.submitting)
            .finish()
    }
}

impl Clone for AskState {
    fn clone(&self) -> Self {
        Self {
            batches: self.batches.clone(),
            dismissed_batch: self.dismissed_batch.clone(),
            bound_batch: self.bound_batch.clone(),
            answers: self.answers.clone(),
            custom_open: self.custom_open.clone(),
            custom_edits: self.custom_edits.clone(),
            collapsed: self.collapsed,
            submitting: self.submitting,
            error: self.error.clone(),
            poll_inbox: None,
            submit_inbox: None,
            last_poll: self.last_poll,
        }
    }
}

impl Default for AskState {
    fn default() -> Self {
        Self {
            batches: Vec::new(),
            dismissed_batch: None,
            bound_batch: None,
            answers: std::collections::HashMap::new(),
            custom_open: std::collections::HashSet::new(),
            custom_edits: std::collections::HashMap::new(),
            collapsed: false,
            submitting: false,
            error: None,
            poll_inbox: None,
            submit_inbox: None,
            last_poll: f64::NEG_INFINITY,
        }
    }
}

impl AskState {
    /// Batch that belongs to `chat_id` and is not dismissed client-side.
    pub fn visible_batch(&self, chat_id: &str) -> Option<&AskBatch> {
        if chat_id.trim().is_empty() {
            return None;
        }
        self.batches.iter().find(|batch| {
            batch.chat_id == chat_id
                && self.dismissed_batch.as_deref() != Some(batch.batch_id.as_str())
        })
    }

    /// Adopt a poll result, dropping the dismissed marker once the batch is
    /// gone (React filters `dismissed` the same way).
    pub fn adopt(&mut self, batches: Vec<AskBatch>) {
        if let Some(id) = &self.dismissed_batch {
            if !batches.iter().any(|batch| &batch.batch_id == id) {
                self.dismissed_batch = None;
            }
        }
        self.batches = batches;
    }

    /// Point the form at the visible batch. Returns true when the visible
    /// batch changed (the panel behaves like React's per-batch `key`).
    pub fn bind_visible(&mut self, chat_id: &str) -> bool {
        let visible = self
            .visible_batch(chat_id)
            .map(|batch| batch.batch_id.clone());
        if visible == self.bound_batch {
            return false;
        }
        self.bound_batch = visible.clone();
        self.reset_form();
        if let Some(id) = visible {
            if let Some(batch) = self.batches.iter().find(|batch| batch.batch_id == id) {
                for question in &batch.questions {
                    self.custom_edits
                        .insert(question.id.clone(), TextEdit::new(""));
                }
            }
        }
        true
    }

    fn reset_form(&mut self) {
        self.answers.clear();
        self.custom_open.clear();
        self.custom_edits.clear();
        self.collapsed = false;
        self.submitting = false;
        self.error = None;
    }

    /// Hide the batch for this client; the agent keeps waiting.
    pub fn dismiss(&mut self, batch_id: &str) {
        self.dismissed_batch = Some(batch_id.to_string());
        self.bound_batch = None;
        self.reset_form();
    }

    pub fn toggle(&mut self, question_id: &str, label: &str, multi: bool) {
        let selected = self.answers.entry(question_id.to_string()).or_default();
        if multi {
            if let Some(index) = selected.iter().position(|entry| entry == label) {
                selected.remove(index);
            } else {
                selected.push(label.to_string());
            }
        } else {
            *selected = vec![label.to_string()];
        }
    }

    /// First click on “Type your own answer…” clears the option selection,
    /// like React's `!custom` branch. Later clicks keep both.
    pub fn open_custom(&mut self, question_id: &str) {
        let already = self.custom_open.contains(question_id)
            || self
                .custom_edits
                .get(question_id)
                .is_some_and(|edit| !edit.text.trim().is_empty());
        self.custom_open.insert(question_id.to_string());
        if !already {
            self.answers.insert(question_id.to_string(), Vec::new());
            self.custom_edits
                .insert(question_id.to_string(), TextEdit::new(""));
        }
    }

    pub fn custom_text(&self, question_id: &str) -> String {
        self.custom_edits
            .get(question_id)
            .map(|edit| edit.text.trim().to_string())
            .unwrap_or_default()
    }

    pub fn is_answered(&self, question_id: &str) -> bool {
        self.answers
            .get(question_id)
            .is_some_and(|selected| !selected.is_empty())
            || !self.custom_text(question_id).is_empty()
    }

    pub fn answered_count(&self, batch: &AskBatch) -> usize {
        batch
            .questions
            .iter()
            .filter(|question| self.is_answered(&question.id))
            .count()
    }

    pub fn all_answered(&self, batch: &AskBatch) -> bool {
        batch
            .questions
            .iter()
            .all(|question| self.is_answered(&question.id))
    }

    /// Exact `AskAnswer[]` body (`selected` keeps option order, `custom` is
    /// trimmed like `setCustom` + `submit`).
    pub fn submit_body(&self, batch: &AskBatch) -> serde_json::Value {
        let answers: Vec<serde_json::Value> = batch
            .questions
            .iter()
            .map(|question| {
                serde_json::json!({
                    "id": question.id,
                    "selected": self.answers.get(&question.id).cloned().unwrap_or_default(),
                    "custom": self.custom_text(&question.id),
                })
            })
            .collect();
        serde_json::json!({
            "batch_id": batch.batch_id,
            "answers": answers,
        })
    }
}

/// Submit the visible batch. No-op while another submit is in flight or any
/// question is unanswered (React's disabled guard, enforced again here for
/// the Enter key).
pub fn submit<S: TranscriptHost + 'static>(s: &mut S, which: Thread, batch: &AskBatch) {
    let thread = s.thread_mut(which);
    if thread.ask.submitting || !thread.ask.all_answered(batch) {
        return;
    }
    let body = thread.ask.submit_body(batch);
    thread.ask.submitting = true;
    thread.ask.error = None;
    let (tx, rx) = std::sync::mpsc::channel();
    thread.ask.submit_inbox = Some(rx);
    let (host, port) = (thread.api_host.clone(), thread.api_port);
    let batch_id = batch.batch_id.clone();
    spawn_submit(host, port, body, batch_id, tx);
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_submit(
    host: String,
    port: u16,
    body: serde_json::Value,
    batch_id: String,
    tx: std::sync::mpsc::Sender<Result<String, String>>,
) {
    std::thread::spawn(move || {
        let result = crate::transport::send_json(&host, port, "POST", "/api/ask/answer", &body)
            .map(|_| batch_id);
        let _ = tx.send(result);
    });
}

#[cfg(target_arch = "wasm32")]
fn spawn_submit(
    host: String,
    port: u16,
    body: serde_json::Value,
    batch_id: String,
    tx: std::sync::mpsc::Sender<Result<String, String>>,
) {
    wasm_bindgen_futures::spawn_local(async move {
        let result = crate::transport::fetch_send("POST", "/api/ask/answer", Some(&body))
            .await
            .map(|_| batch_id);
        let _ = tx.send(result);
        let _ = (host, port);
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_poll(host: String, port: u16, tx: std::sync::mpsc::Sender<Result<Vec<AskBatch>, String>>) {
    std::thread::spawn(move || {
        let result = crate::transport::get_json(&host, port, "/api/ask/pending")
            .map(|value| AskBatch::list_from_json(&value));
        let _ = tx.send(result);
    });
}

#[cfg(target_arch = "wasm32")]
fn spawn_poll(host: String, port: u16, tx: std::sync::mpsc::Sender<Result<Vec<AskBatch>, String>>) {
    wasm_bindgen_futures::spawn_local(async move {
        let result = match crate::transport::fetch_json("/api/ask/pending").await {
            Ok(value) => Ok(AskBatch::list_from_json(&value)),
            Err(message) => Err(message),
        };
        let _ = tx.send(result);
        let _ = (host, port);
    });
}

/// Drain ask poll/submit results and start the 1.5s poll. Only called for the
/// visible sheet thread, so background chats stay offline until opened.
pub fn poll_ask_state(s: &mut RuneTranscriptState, clock: f64) -> bool {
    let mut changed = false;
    let (polls, poll_dead) = crate::rooms::drain_results(&mut s.ask.poll_inbox);
    // Poll failures are silent, like `fetchPendingQuestions`' catch.
    for batches in polls.into_iter().flatten() {
        s.ask.adopt(batches);
        changed = true;
    }
    let (submits, submit_dead) = crate::rooms::drain_results(&mut s.ask.submit_inbox);
    for msg in submits {
        s.ask.submitting = false;
        match msg {
            Ok(batch_id) => {
                // Drop it immediately; the next poll confirms it too.
                s.ask.batches.retain(|batch| batch.batch_id != batch_id);
                s.ask.bound_batch = None;
                s.ask.reset_form();
            }
            Err(_) => {
                s.ask.error = Some(
                    "Failed to deliver your answer. The agent may have timed out.".to_string(),
                );
            }
        }
        changed = true;
    }
    if submit_dead && s.ask.submitting {
        // Worker dropped without a result (panic/kill): surface React's error
        // instead of leaving the button stuck on “Sending…”.
        s.ask.submitting = false;
        s.ask.error =
            Some("Failed to deliver your answer. The agent may have timed out.".to_string());
        changed = true;
    }
    let _ = poll_dead;
    if s.ask.bind_visible(&s.chat_id) {
        changed = true;
    }
    if s.services_auto
        && !s.chat_id.trim().is_empty()
        && s.ask.poll_inbox.is_none()
        && clock - s.ask.last_poll >= POLL_MS as f64 / 1000.0
    {
        s.ask.last_poll = clock;
        let (tx, rx) = std::sync::mpsc::channel();
        s.ask.poll_inbox = Some(rx);
        spawn_poll(s.api_host.clone(), s.api_port, tx);
        changed = true;
    }
    changed
}

/// Number of milliseconds until the next poll is due; the shell asks Rune to
/// wake then (`usePendingAsk`'s `setInterval`).
pub fn poll_wait_ms(s: &RuneTranscriptState, clock: f64) -> Option<u64> {
    if !s.services_auto || s.chat_id.trim().is_empty() || s.ask.poll_inbox.is_some() {
        return None;
    }
    let due = s.ask.last_poll + POLL_MS as f64 / 1000.0;
    Some(((due - clock).max(0.0) * 1000.0).ceil() as u64)
}

/// True while a poll or submit result is in flight (the shell keeps a 100ms
/// poll ticket alive until it lands).
pub fn ask_busy(s: &RuneTranscriptState) -> bool {
    s.ask.poll_inbox.is_some() || s.ask.submit_inbox.is_some()
}

// ---------------------------------------------------------------------------
// Icons
// ---------------------------------------------------------------------------

fn help_circle(size: f32, color: rune::Color) -> rune::Shape {
    // Lucide `circle-help`: r=10 circle plus the question hook and dot.
    const K: f32 = 5.5228;
    let p = rune::Path::new()
        .move_to([12.0, 2.0])
        .cubic_to([12.0 + K, 2.0], [22.0, 12.0 - K], [22.0, 12.0])
        .cubic_to([22.0, 12.0 + K], [12.0 + K, 22.0], [12.0, 22.0])
        .cubic_to([12.0 - K, 22.0], [2.0, 12.0 + K], [2.0, 12.0])
        .cubic_to([2.0, 12.0 - K], [12.0 - K, 2.0], [12.0, 2.0])
        .close()
        .move_to([9.09, 9.0])
        .cubic_to([9.575, 7.615], [10.981, 6.789], [12.427, 7.037])
        .cubic_to([13.872, 7.285], [14.923, 8.533], [14.92, 10.0])
        .cubic_to([14.92, 12.0], [11.92, 13.0], [11.92, 13.0])
        .move_to([12.0, 17.0])
        .line_to([12.01, 17.0]);
    rune::Shape::new()
        .stroke_path(&p, crate::lucide_stroke(size), rune::LineCap::Round, color)
        .view_box(24.0, 24.0)
}

fn chevron_down(size: f32, collapsed: bool, color: rune::Color) -> rune::Shape {
    let p = if collapsed {
        rune::Path::new()
            .move_to([9.0, 18.0])
            .line_to([15.0, 12.0])
            .line_to([9.0, 6.0])
    } else {
        rune::Path::new()
            .move_to([6.0, 9.0])
            .line_to([12.0, 15.0])
            .line_to([18.0, 9.0])
    };
    rune::Shape::new()
        .stroke_path(&p, crate::lucide_stroke(size), rune::LineCap::Round, color)
        .view_box(24.0, 24.0)
}

fn style(size: f32, weight: f32, line_height: f32) -> rune::tok::TextStyle {
    let mut style = rune::tok::TextStyle::new(size, weight);
    style.line_height = line_height;
    style
}

// ---------------------------------------------------------------------------
// Panel
// ---------------------------------------------------------------------------

fn icon_button<S: TranscriptHost + 'static>(
    key: &str,
    label: &str,
    size: f32,
    icon: f32,
    shape: rune::Shape,
    press: std::rc::Rc<dyn Fn(&mut S)>,
) -> El {
    rune::el()
        .key(key)
        .role("button")
        .focusable(true)
        .label(label)
        .size(size, size)
        .radius(if size > 24.0 { 7.0 } else { 6.0 })
        .center()
        .hover(ink("#ffffff0f"))
        .on_press_enter(move |s: &mut S| press(s))
        .child(rune::el().size(icon, icon).inert().shape(shape))
}

fn option_row<S: TranscriptHost + 'static>(
    question: &AskQuestion,
    index: usize,
    label: &str,
    description: Option<&str>,
    selected: bool,
    compact: bool,
    on_press: std::rc::Rc<dyn Fn(&mut S)>,
) -> El {
    let gap = if compact { 7.0 } else { 10.0 };
    let pad_x = if compact { 9.0 } else { 12.0 };
    let pad_y = if compact { 7.0 } else { 10.0 };
    let radius = if compact { 8.0 } else { 10.0 };
    let radio = if compact { 14.0 } else { 16.0 };
    let index_size = if compact { 17.0 } else { 20.0 };
    let label_style = style(
        if compact { 11.0 } else { 13.0 },
        400.0,
        if compact { 14.85 } else { 17.55 },
    );
    let desc_style = style(
        if compact { 9.5 } else { 11.5 },
        400.0,
        if compact { 13.3 } else { 16.1 },
    );
    let index_style = style(
        if compact { 8.5 } else { 9.5 },
        400.0,
        if compact { 12.75 } else { 14.25 },
    );

    let border = if selected {
        rgba(99.0, 102.0, 241.0, 0.70)
    } else {
        ink("#27272a")
    };
    let fill = if selected {
        rgba(99.0, 102.0, 241.0, 0.08)
    } else {
        ink("#181818")
    };

    let mut radio_el = rune::el()
        .size(radio, radio)
        .radius(if question.multi { 5.0 } else { radio / 2.0 })
        .border(
            1.0,
            if selected {
                ink("#818cf8")
            } else {
                ink("#52525b")
            },
        )
        .border_layout()
        .center();
    if selected {
        radio_el = radio_el.fill(rgba(99.0, 102.0, 241.0, 0.30)).child(
            rune::el()
                .size(10.0, 10.0)
                .inert()
                .shape(crate::lucide_check(10.0, 2.0, ink("#a5b4fc"))),
        );
    }

    let mut copy = col()
        .key(format!("ask-option-copy-{}-{index}", question.id))
        .min_w(0.0)
        .grow(1.0)
        .child(
            text(label.to_string())
                .font(label_style)
                .color(ink("#f4f4f5")),
        );
    if let Some(description) = description {
        copy = copy.child(
            text(description.to_string())
                .pt(2.0)
                .font(desc_style)
                .wrap()
                .color(ink("#71717a")),
        );
    }

    let index_el = rune::el()
        .size(index_size, index_size)
        .radius(if compact { 5.0 } else { 6.0 })
        .fill(rgba(255.0, 255.0, 255.0, 0.06))
        .border(1.0, rgba(255.0, 255.0, 255.0, 0.10))
        .border_layout()
        .center()
        .child(
            text((index + 1).to_string())
                .font(index_style)
                .color(ink("#71717a")),
        );
    let mut option = row()
        .key(format!("ask-option-{}-{index}-button", question.id))
        .role("button")
        .focusable(true)
        .label(label.to_string())
        .w_full()
        .items_center()
        .gap(gap)
        .px(pad_x)
        .py(pad_y)
        .radius(radius)
        .fill(fill)
        .border(1.0, border)
        .border_layout();
    // `.ask-option:hover` replaces the zinc surface and lifts the border to
    // `#3f3f46`. The fill is opaque, so the overlay replaces it exactly; the
    // border needs a paint-only sibling because Rune has no declarative
    // hover-border builder (a `Live` border track is the alternative).
    if !selected {
        option = option.hover(ink("#1c1c20"));
    }
    option = option
        .on_press_enter(move |s: &mut S| on_press(s))
        .child(radio_el)
        .child(copy)
        .child(index_el);

    // The outer stack keeps the public key and the full border box; the
    // padded button sits inside it, so the overlay border is not inset by
    // the button's padding.
    let mut stack = col()
        .key(format!("ask-option-{}-{index}", question.id))
        .w_full();
    if selected {
        stack = stack.child(option);
    } else {
        stack = stack.hover_group().child(option).child(
            rune::el()
                .absolute(0.0, 0.0)
                .w_full()
                .h_full()
                .radius(radius)
                .border(1.0, ink("#3f3f46"))
                .group_opacity(0.0, 1.0)
                .inert(),
        );
    }
    stack
}

fn question_block<S: TranscriptHost + 'static>(
    batch: &AskBatch,
    question: &AskQuestion,
    ask: &AskState,
    which: Thread,
    compact: bool,
) -> El {
    let title_style = style(
        if compact { 11.5 } else { 14.0 },
        600.0,
        if compact { 16.1 } else { 19.6 },
    );
    let hint_style = style(
        if compact { 9.5 } else { 11.0 },
        400.0,
        if compact { 14.25 } else { 16.5 },
    );
    let detail_style = style(
        if compact { 10.0 } else { 12.0 },
        400.0,
        if compact { 15.0 } else { 18.6 },
    );
    let mut block = col()
        .key(format!("ask-question-{}", question.id))
        .w_full()
        .child(
            text(question.question.clone())
                .font(title_style)
                .wrap()
                .color(ink("#f4f4f5")),
        )
        .child(
            text(
                if question.multi {
                    "Select one or more answers"
                } else {
                    "Select one answer"
                }
                .to_string(),
            )
            .pt(2.0)
            .font(hint_style)
            .color(ink("#71717a")),
        );
    if let Some(detail) = &question.detail {
        block = block.child(
            text(detail.clone())
                .pt(6.0)
                .font(detail_style)
                .wrap()
                .color(ink("#a1a1aa")),
        );
    }

    let mut options = col()
        .key(format!("ask-options-{}", question.id))
        .w_full()
        .gap(if compact { 5.0 } else { 6.0 })
        .pt(if compact { 7.0 } else { 10.0 })
        .role("group")
        .label("Answers")
        .focus_group();
    for (index, option) in question.options.iter().enumerate() {
        let selected = ask
            .answers
            .get(&question.id)
            .is_some_and(|picked| picked.iter().any(|entry| entry == &option.label));
        let question_id = question.id.clone();
        let label = option.label.clone();
        let multi = question.multi;
        options = options.child(option_row::<S>(
            question,
            index,
            &option.label,
            option.description.as_deref(),
            selected,
            compact,
            std::rc::Rc::new(move |s: &mut S| {
                s.thread_mut(which).ask.toggle(&question_id, &label, multi);
            }),
        ));
    }

    let custom =
        ask.custom_open.contains(&question.id) || !ask.custom_text(&question.id).is_empty();
    {
        let question_id = question.id.clone();
        options = options.child(option_row::<S>(
            question,
            question.options.len(),
            "Type your own answer…",
            None,
            custom,
            compact,
            std::rc::Rc::new(move |s: &mut S| {
                s.thread_mut(which).ask.open_custom(&question_id);
            }),
        ));
    }

    if custom {
        if let Some(edit) = ask.custom_edits.get(&question.id) {
            let question_id = question.id.clone();
            let input_style = style(if compact { 11.0 } else { 13.0 }, 400.0, 16.9);
            let enter_batch = batch.clone();
            options = options.child(
                text_input(edit, input_style)
                    .key(format!("ask-custom-{}", question.id))
                    .single_line()
                    .w_full()
                    .px(if compact { 9.0 } else { 12.0 })
                    .py(if compact { 7.0 } else { 9.0 })
                    .radius(if compact { 8.0 } else { 10.0 })
                    .fill(ink("#181818"))
                    .border(1.0, ink("#3f3f46"))
                    .border_layout()
                    .placeholder("Type your answer…", ink("#52525b"))
                    .caret(ink("#e4e4e7"), crate::motion::TEXT_SELECTION)
                    .on_edit(move |s: &mut S| {
                        let ask = &mut s.thread_mut(which).ask;
                        ask.custom_edits
                            .entry(question_id.clone())
                            .or_insert_with(|| TextEdit::new(""))
                    })
                    .on_input_key(move |s: &mut S, key: &rune::KeyEvent, _tx: &mut rune::Tx| {
                        if key.key != rune::Key::Enter || key.mods.shift {
                            return false;
                        }
                        if !s.thread(which).ask.all_answered(&enter_batch) {
                            return false;
                        }
                        submit::<S>(s, which, &enter_batch);
                        true
                    }),
            );
        }
    }
    block.child(options)
}

/// The ask panel that replaces the prompt dock for `batch`'s chat.
pub fn ask_panel<S: TranscriptHost + 'static>(
    batch: &AskBatch,
    ask: &AskState,
    which: Thread,
    compact: bool,
    window_h: f32,
) -> El {
    let radius = if compact { 12.0 } else { 16.0 };
    let max_h = if compact {
        // Canvas cards cap at 250px (`canvas.css`); the dock's compact panel
        // is the home card's, so keep `askDock.css`'s 292pt cap here.
        292.0
    } else {
        (window_h * 0.62).min(560.0)
    };
    // The panel's outlined border box caps at `max_h`; its two 1pt borders
    // and the measured head/foot heights leave the rest for the scrolling
    // body (React's `flex: 1 1 auto` inside the same max-height).
    let head_h = if compact { 45.65 } else { 58.9 };
    let foot_h = if compact { 42.0 } else { 57.75 };
    let body_cap = (max_h - 2.0 - head_h - foot_h).max(48.0);

    let icon_size = if compact { 13.0 } else { 16.0 };
    let button_size = if compact { 22.0 } else { 26.0 };
    let control_icon = if compact { 13.0 } else { 15.0 };
    let pad_x = if compact { 12.0 } else { 18.0 };
    let head_y = if compact { 9.0 } else { 14.0 };
    let gap = if compact { 8.0 } else { 10.0 };

    let batch_dismiss = batch.batch_id.clone();
    let batch_collapse = batch.batch_id.clone();
    let collapse_label = if ask.collapsed {
        "Expand question"
    } else {
        "Collapse question"
    };

    let head = row()
        .key("ask-panel-head")
        .w_full()
        .items_center()
        .gap(gap)
        .px(pad_x)
        .py(head_y)
        .border_sides([0.0, 0.0, 1.0, 0.0], ink("#27272a"))
        .border_layout()
        .child(
            rune::el()
                .size(icon_size, icon_size)
                .inert()
                .shape(help_circle(icon_size, ink("#a1a1aa"))),
        )
        .child(
            col()
                .key("ask-panel-heading")
                .min_w(0.0)
                .grow(1.0)
                .child(
                    text("Question".to_string())
                        .font(style(
                            if compact { 11.0 } else { 12.5 },
                            600.0,
                            if compact { 14.3 } else { 16.25 },
                        ))
                        .color(ink("#f4f4f5")),
                )
                .child(
                    text("The agent is waiting for your answer.".to_string())
                        .font(style(
                            if compact { 9.5 } else { 10.5 },
                            400.0,
                            if compact { 12.35 } else { 13.65 },
                        ))
                        .color(ink("#71717a")),
                ),
        )
        .child(icon_button::<S>(
            "ask-collapse",
            collapse_label,
            button_size,
            control_icon,
            chevron_down(control_icon, ask.collapsed, ink("#71717a")),
            std::rc::Rc::new(move |s: &mut S| {
                let ask = &mut s.thread_mut(which).ask;
                ask.collapsed = !ask.collapsed;
                let _ = &batch_collapse;
            }),
        ))
        .child(icon_button::<S>(
            "ask-dismiss",
            "Dismiss",
            button_size,
            control_icon,
            crate::lucide_x(control_icon, 2.0, ink("#71717a")),
            std::rc::Rc::new(move |s: &mut S| {
                s.thread_mut(which).ask.dismiss(&batch_dismiss);
            }),
        ));

    let mut body = col()
        .key("ask-panel-body")
        .w_full()
        .gap(if compact { 10.0 } else { 18.0 })
        .px(pad_x)
        .py(if compact { 10.0 } else { 16.0 })
        .max_h(body_cap)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    for question in &batch.questions {
        body = body.child(question_block::<S>(batch, question, ask, which, compact));
    }
    if let Some(error) = &ask.error {
        body = body.child(
            text(error.clone())
                .role("alert")
                .live_region(rune::LiveRegion::Assertive)
                .font(style(
                    if compact { 10.0 } else { 12.0 },
                    400.0,
                    if compact { 15.0 } else { 18.0 },
                ))
                .wrap()
                .color(ink("#f87171")),
        );
    }

    let answered = ask.answered_count(batch);
    let total = batch.questions.len();
    let all_answered = ask.all_answered(batch);
    let disabled = ask.submitting || !all_answered;
    let submit_batch = batch.clone();
    let dismiss_foot = batch.batch_id.clone();

    let foot = row()
        .key("ask-panel-foot")
        .w_full()
        .items_center()
        .justify_between()
        .gap(8.0)
        .px(pad_x)
        .py(if compact { 8.0 } else { 12.0 })
        .border_sides([1.0, 0.0, 0.0, 0.0], ink("#27272a"))
        .border_layout()
        .child(
            text("Dismiss".to_string())
                .key("ask-dismiss-foot")
                .role("button")
                .focusable(true)
                .label("Dismiss")
                .font(style(
                    if compact { 10.0 } else { 12.0 },
                    400.0,
                    if compact { 15.0 } else { 18.0 },
                ))
                .px(if compact { 7.0 } else { 10.0 })
                .py(if compact { 5.0 } else { 7.0 })
                .radius(8.0)
                .color(ink("#a1a1aa"))
                .hover(rgba(255.0, 255.0, 255.0, 0.05))
                .on_press_enter(move |s: &mut S| {
                    s.thread_mut(which).ask.dismiss(&dismiss_foot);
                }),
        )
        .child(
            row()
                .key("ask-submit-wrap")
                .items_center()
                .gap(10.0)
                .child(
                    text(format!("{answered}/{total} answered"))
                        .font(style(
                            if compact { 8.5 } else { 10.5 },
                            400.0,
                            if compact { 12.75 } else { 15.75 },
                        ))
                        .color(ink("#52525b")),
                )
                .child(
                    text(
                        if ask.submitting {
                            "Sending…"
                        } else {
                            "Submit"
                        }
                        .to_string(),
                    )
                    .key("ask-submit")
                    .role("button")
                    .focusable(true)
                    .label("Submit answer")
                    .font(style(
                        if compact { 10.0 } else { 12.5 },
                        600.0,
                        if compact { 15.0 } else { 18.75 },
                    ))
                    .px(if compact { 9.0 } else { 14.0 })
                    .py(if compact { 5.0 } else { 7.0 })
                    .radius(if compact { 7.0 } else { 8.0 })
                    .fill(ink("#e4e4e7"))
                    .color(ink("#0a0a0d"))
                    .opacity(if disabled { 0.4 } else { 1.0 })
                    .disabled(disabled)
                    .on_press_enter(move |s: &mut S| {
                        submit::<S>(s, which, &submit_batch);
                    }),
                ),
        );

    let mut panel = col()
        .key("ask-panel")
        .role("dialog")
        .label("Question")
        .w_full()
        .max_h(max_h)
        .radius(radius)
        .fill(ink("#141414"))
        .border(1.0, ink("#27272a"))
        .border_layout()
        .clip_rounded(radius)
        .shadow(
            if compact { 18.0 } else { 24.0 },
            if compact { 48.0 } else { 64.0 },
            rune::Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: if compact { 0.46 } else { 0.50 },
            },
        )
        .child(head);
    if !ask.collapsed {
        panel = panel.child(body);
    }
    panel.child(foot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rune_transcript::{RuneTranscriptState, Thread};

    fn batch_json() -> serde_json::Value {
        serde_json::json!({
            "batches": [{
                "batch_id": "b1",
                "chat_id": "chat-a",
                "created": 42,
                "questions": [{
                    "id": "q1",
                    "question": "Which branch?",
                    "detail": "Pick one",
                    "header": "Branch",
                    "options": ["main", {"label": "dev", "description": "integration"}],
                }, {
                    "id": "q2",
                    "question": "Which files?",
                    "multi_select": true,
                    "options": [{"label": "frontend"}, "backend"],
                    "intent": {"kind": "plan-review", "approve": "dev"}
                }]
            }]
        })
    }

    #[test]
    fn parses_live_shape() {
        let batches = AskBatch::list_from_json(&batch_json());
        assert_eq!(batches.len(), 1);
        let batch = &batches[0];
        assert_eq!(batch.batch_id, "b1");
        assert_eq!(batch.chat_id, "chat-a");
        assert_eq!(batch.created, 42);
        assert_eq!(batch.questions.len(), 2);
        assert_eq!(batch.questions[0].question, "Which branch?");
        assert_eq!(batch.questions[0].detail.as_deref(), Some("Pick one"));
        assert!(!batch.questions[0].multi);
        assert_eq!(batch.questions[0].options[0].label, "main");
        assert_eq!(
            batch.questions[0].options[1].description.as_deref(),
            Some("integration")
        );
        assert!(batch.questions[1].multi);
        assert!(batch.questions[1].detail.is_none());
        assert!(AskBatch::list_from_json(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn toggles_single_and_multi() {
        let batches = AskBatch::list_from_json(&batch_json());
        let batch = batches[0].clone();
        let mut ask = AskState::default();
        ask.adopt(batches);
        assert!(ask.bind_visible("chat-a"));
        ask.toggle("q1", "main", false);
        assert_eq!(ask.answers["q1"], vec!["main"]);
        ask.toggle("q1", "dev", false);
        assert_eq!(ask.answers["q1"], vec!["dev"]);
        ask.toggle("q2", "frontend", true);
        ask.toggle("q2", "backend", true);
        assert_eq!(ask.answers["q2"], vec!["frontend", "backend"]);
        ask.toggle("q2", "frontend", true);
        assert_eq!(ask.answers["q2"], vec!["backend"]);
        assert_eq!(ask.answered_count(&batch), 2);
        assert!(ask.all_answered(&batch));
    }

    #[test]
    fn custom_answer_clears_selection_once_and_submits() {
        let batches = AskBatch::list_from_json(&batch_json());
        let batch = batches[0].clone();
        let mut ask = AskState::default();
        ask.adopt(batches);
        assert!(ask.bind_visible("chat-a"));
        ask.toggle("q1", "main", false);
        ask.open_custom("q1");
        assert!(ask.answers["q1"].is_empty());
        ask.custom_edits.get_mut("q1").unwrap().text = "  release  ".to_string();
        assert!(ask.is_answered("q1"));
        // Opening again keeps the current selection (React's `custom` is
        // already true, so it skips the clearing branch).
        ask.toggle("q1", "dev", false);
        ask.open_custom("q1");
        assert_eq!(ask.answers["q1"], vec!["dev"]);
        ask.toggle("q2", "backend", true);
        assert!(ask.all_answered(&batch));
        let body = ask.submit_body(&batch);
        assert_eq!(body["batch_id"], "b1");
        assert_eq!(body["answers"][0]["id"], "q1");
        assert_eq!(body["answers"][0]["custom"], "release");
        assert_eq!(body["answers"][0]["selected"], serde_json::json!(["dev"]));
        assert_eq!(
            body["answers"][1]["selected"],
            serde_json::json!(["backend"])
        );
    }

    #[test]
    fn dismiss_hides_until_batch_leaves_the_list() {
        let batches = AskBatch::list_from_json(&batch_json());
        let mut ask = AskState::default();
        ask.adopt(batches.clone());
        assert!(ask.bind_visible("chat-a"));
        assert!(ask.visible_batch("chat-a").is_some());
        assert!(ask.visible_batch("chat-b").is_none());
        ask.dismiss("b1");
        assert!(ask.visible_batch("chat-a").is_none());
        assert!(ask.bound_batch.is_none());
        // Same list keeps the dismissal.
        ask.adopt(batches);
        assert!(ask.visible_batch("chat-a").is_none());
        // Once the batch leaves, a later batch is not masked.
        ask.adopt(Vec::new());
        assert!(ask.dismissed_batch.is_none());
        ask.adopt(AskBatch::list_from_json(&batch_json()));
        assert!(ask.visible_batch("chat-a").is_some());
    }

    #[test]
    fn real_socket_poll_and_submit() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (posted_tx, posted_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut gets = 0;
            for _ in 0..8 {
                let (mut sock, _) = listener.accept().unwrap();
                let mut raw = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    let n = match sock.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    raw.extend_from_slice(&buf[..n]);
                    if let Some(head_end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
                        let len = head
                            .lines()
                            .find_map(|line| line.strip_prefix("Content-Length: "))
                            .and_then(|value| value.parse::<usize>().ok())
                            .unwrap_or(0);
                        if raw.len() >= head_end + 4 + len {
                            break;
                        }
                    }
                }
                let request = String::from_utf8_lossy(&raw).to_string();
                let is_post = request.starts_with("POST /api/ask/answer");
                let (status, body) = if request.starts_with("GET /api/ask/pending") {
                    gets += 1;
                    if gets == 1 {
                        (
                            "200 OK",
                            r#"{"batches":[{"batch_id":"b1","chat_id":"chat-a","created":1,"questions":[{"id":"q1","question":"Pick one","options":["alpha","beta"]}]}]}"#,
                        )
                    } else {
                        // The backend drops the batch once it is answered.
                        ("200 OK", r#"{"batches":[]}"#)
                    }
                } else {
                    let _ = posted_tx.send(request);
                    ("200 OK", r#"{"ok":true}"#)
                };
                let _ = sock.write_all(
                    format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                );
                if is_post {
                    break;
                }
            }
        });

        let mut s = RuneTranscriptState::screenshot();
        s.chat_id = "chat-a".to_string();
        s.api_host = "127.0.0.1".to_string();
        s.api_port = port;
        s.services_auto = true;
        s.ask.last_poll = f64::NEG_INFINITY;
        let mut clock = 0.0;
        let mut waited = 0;
        while s.ask.visible_batch("chat-a").is_none() && waited < 500 {
            poll_ask_state(&mut s, clock);
            if s.ask.visible_batch("chat-a").is_none() {
                std::thread::sleep(std::time::Duration::from_millis(10));
                clock += 1.6;
                waited += 1;
            }
        }
        let batch = s
            .ask
            .visible_batch("chat-a")
            .cloned()
            .expect("batch landed");
        // The poll already bound the form when the batch landed; stop
        // scheduling new polls so no late snapshot can re-add the answered
        // batch while the submit drain runs.
        assert_eq!(s.ask.bound_batch.as_deref(), Some("b1"));
        s.services_auto = false;
        s.ask.toggle("q1", "alpha", false);
        assert!(s.ask.all_answered(&batch));
        submit(&mut s, Thread::Main, &batch);
        assert!(s.ask.submitting);
        waited = 0;
        while s.ask.submitting && waited < 500 {
            poll_ask_state(&mut s, clock);
            if s.ask.submitting {
                std::thread::sleep(std::time::Duration::from_millis(10));
                waited += 1;
            }
        }
        assert!(!s.ask.submitting);
        assert!(s.ask.error.is_none());
        assert!(s.ask.batches.is_empty());
        let posted = posted_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("answer POST");
        assert!(posted.starts_with("POST /api/ask/answer"));
        assert!(posted.contains("\"batch_id\":\"b1\""));
        assert!(posted.contains("\"selected\":[\"alpha\"]"));
    }

    #[test]
    fn bind_resets_form_when_visible_batch_changes() {
        let mut batches = AskBatch::list_from_json(&batch_json());
        let mut ask = AskState::default();
        ask.adopt(batches.clone());
        assert!(ask.bind_visible("chat-a"));
        assert!(ask.custom_edits.contains_key("q1"));
        ask.toggle("q1", "main", false);
        ask.collapsed = true;
        batches[0].batch_id = "b2".to_string();
        ask.adopt(batches);
        assert!(ask.bind_visible("chat-a"));
        assert_eq!(ask.bound_batch.as_deref(), Some("b2"));
        assert!(ask.answers.is_empty());
        assert!(!ask.collapsed);
        assert!(ask.custom_edits.contains_key("q1"));
    }
}
