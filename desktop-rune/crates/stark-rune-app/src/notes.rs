//! Sticky notes — port of `NotesButton.tsx` + `services/api/notes.ts`.
//!
//! Contract (verified against `notes.ts`):
//! - `GET /api/notes/{chat|group}/{scope_id}` → `{notes:[...]}`.
//! - `POST /api/notes {scope_type, scope_id, title, body, todos:[str]}`.
//! - `PATCH /api/notes/{id} {todos:[{text,done}]}` (todo flips carry the
//!   whole list, like `toggleTodo`).
//! - `DELETE /api/notes/{id}`.
//!
//! All writes are explicit button presses (user go-ahead at press time, like
//! composer send). Delete is two-tap: Rune has no `window.confirm`, so the
//! first tap arms (`confirm_delete`) and the second fires. Amber card
//! language mirrors the React panel. Mounts: transcript header (chat scope,
//! like `ChatView.tsx`). Group scope (`GroupProjectModal.tsx`) arrives with
//! the modal port — the state is already scope-driven.

use crate::keys::EnterPress;
use rune::{col, row, text, text_input, El, TextEdit};

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

/// Wall-clock millis. `SystemTime` panics on wasm32-unknown-unknown (no OS
/// clock), so `clock` reads `Date.now()` there instead. `STARK_PARITY_NOW`
/// freezes it for deterministic tests.
pub fn now_millis() -> u64 {
    crate::clock::now_millis()
}

/// Mint an opaque thread id (`rune-<millis>`). React reuses the persistent
/// `chat.id`; Rune threads mint one at construction so notes/abort have a
/// stable scope before the first send.
pub fn mint_id(prefix: &str) -> String {
    format!("{prefix}-{}", now_millis())
}

/// One checklist item.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteTodo {
    pub text: String,
    pub done: bool,
}

impl NoteTodo {
    pub fn from_json(v: &serde_json::Value) -> Self {
        Self {
            text: v
                .get("text")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            done: v.get("done").and_then(|x| x.as_bool()).unwrap_or(false),
        }
    }
}

/// One sticky note.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentNote {
    pub id: String,
    pub title: String,
    pub body: String,
    pub todos: Vec<NoteTodo>,
}

impl AgentNote {
    pub fn from_json(v: &serde_json::Value) -> Self {
        let str_of = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        Self {
            id: str_of("id"),
            title: str_of("title"),
            body: str_of("body"),
            todos: v
                .get("todos")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().map(NoteTodo::from_json).collect())
                .unwrap_or_default(),
        }
    }
}

/// A write op. Workers apply the op (unless `Refresh`) then re-fetch the
/// list, so one inbox + one poll drain every path — no write/read races.
#[derive(Debug, Clone)]
pub enum NotesOp {
    Refresh,
    Create {
        title: String,
        body: String,
        todos: Vec<String>,
    },
    ToggleTodo {
        note_id: String,
        index: usize,
    },
    Delete {
        note_id: String,
    },
}

pub struct NotesState {
    pub host: String,
    pub port: u16,
    pub scope_type: String,
    pub scope_id: String,
    pub notes: Vec<AgentNote>,
    pub open: bool,
    pub adding: bool,
    pub title_edit: TextEdit,
    pub body_edit: TextEdit,
    pub todos_edit: TextEdit,
    pub error: Option<String>,
    pub busy: bool,
    /// Note id armed by the first delete tap; the second fires.
    pub confirm_delete: Option<String>,
    pub inbox: Option<std::sync::mpsc::Receiver<Result<Vec<AgentNote>, String>>>,
}

impl std::fmt::Debug for NotesState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotesState")
            .field("scope_type", &self.scope_type)
            .field("scope_id", &self.scope_id)
            .field("notes", &self.notes.len())
            .field("open", &self.open)
            .field("busy", &self.busy)
            .finish()
    }
}

impl Clone for NotesState {
    fn clone(&self) -> Self {
        Self {
            host: self.host.clone(),
            port: self.port,
            scope_type: self.scope_type.clone(),
            scope_id: self.scope_id.clone(),
            notes: self.notes.clone(),
            open: self.open,
            adding: self.adding,
            title_edit: self.title_edit.clone(),
            body_edit: self.body_edit.clone(),
            todos_edit: self.todos_edit.clone(),
            error: self.error.clone(),
            busy: self.busy,
            confirm_delete: self.confirm_delete.clone(),
            inbox: None,
        }
    }
}

impl Default for NotesState {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8124,
            scope_type: "chat".to_string(),
            scope_id: String::new(),
            notes: Vec::new(),
            open: false,
            adding: false,
            title_edit: TextEdit::new(""),
            body_edit: TextEdit::new(""),
            todos_edit: TextEdit::new(""),
            error: None,
            busy: false,
            confirm_delete: None,
            inbox: None,
        }
    }
}

impl NotesState {
    #[cfg(not(target_arch = "wasm32"))]
    fn fetch_list(host: &str, port: u16, path: &str) -> Result<Vec<AgentNote>, String> {
        let v = crate::transport::get_json(host, port, path)?;
        Ok(v.get("notes")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().map(AgentNote::from_json).collect())
            .unwrap_or_default())
    }
}

/// Open the sheet for a scope and load it (React's `useEffect` on `[open]`).
pub fn open_notes(s: &mut NotesState, scope_type: &str, scope_id: &str) {
    s.scope_type = scope_type.to_string();
    s.scope_id = scope_id.to_string();
    s.open = true;
    s.adding = false;
    s.error = None;
    s.confirm_delete = None;
    dispatch_notes(s, NotesOp::Refresh);
}

/// Spawn a worker for `op`; no-op while one is in flight or the scope is
/// empty (mirrors the `if (!scopeId) return` guards).
pub fn dispatch_notes(s: &mut NotesState, op: NotesOp) {
    if s.busy || s.scope_id.trim().is_empty() {
        return;
    }
    s.busy = true;
    if !matches!(op, NotesOp::Refresh) {
        s.error = None;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    s.inbox = Some(rx);
    let (host, port) = (s.host.clone(), s.port);
    let (scope_type, scope_id) = (s.scope_type.clone(), s.scope_id.clone());
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let result = apply_op_wasm(&scope_type, &scope_id, op).await;
        let _ = tx.send(result);
        let _ = (host, port);
    });
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let result = apply_op_native(&host, port, &scope_type, &scope_id, op);
        let _ = tx.send(result);
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn apply_op_native(
    host: &str,
    port: u16,
    scope_type: &str,
    scope_id: &str,
    op: NotesOp,
) -> Result<Vec<AgentNote>, String> {
    let list_path = format!("/api/notes/{scope_type}/{scope_id}");
    match op {
        NotesOp::Refresh => {}
        NotesOp::Create { title, body, todos } => {
            crate::transport::send_json(
                host,
                port,
                "POST",
                "/api/notes",
                &serde_json::json!({
                    "scope_type": scope_type,
                    "scope_id": scope_id,
                    "title": if title.trim().is_empty() { "Note".to_string() } else { title },
                    "body": body,
                    "todos": todos,
                }),
            )?;
        }
        NotesOp::ToggleTodo { note_id, index } => {
            let current = NotesState::fetch_list(host, port, &list_path)?;
            if let Some(note) = current.iter().find(|n| n.id == note_id) {
                let todos: Vec<serde_json::Value> = note
                    .todos
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        serde_json::json!({"text": t.text, "done": if i == index { !t.done } else { t.done }})
                    })
                    .collect();
                crate::transport::send_json(
                    host,
                    port,
                    "PATCH",
                    &format!("/api/notes/{note_id}"),
                    &serde_json::json!({ "todos": todos }),
                )?;
            }
        }
        NotesOp::Delete { note_id } => {
            crate::transport::send_json(
                host,
                port,
                "DELETE",
                &format!("/api/notes/{note_id}"),
                &serde_json::Value::Null,
            )?;
        }
    }
    NotesState::fetch_list(host, port, &list_path)
}

#[cfg(target_arch = "wasm32")]
async fn apply_op_wasm(
    scope_type: &str,
    scope_id: &str,
    op: NotesOp,
) -> Result<Vec<AgentNote>, String> {
    let list_path = format!("/api/notes/{scope_type}/{scope_id}");
    let parse = |v: &serde_json::Value| {
        v.get("notes")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().map(AgentNote::from_json).collect())
            .unwrap_or_default()
    };
    match op {
        NotesOp::Refresh => {}
        NotesOp::Create { title, body, todos } => {
            crate::transport::fetch_send(
                "POST",
                "/api/notes",
                Some(&serde_json::json!({
                    "scope_type": scope_type,
                    "scope_id": scope_id,
                    "title": if title.trim().is_empty() { "Note".to_string() } else { title },
                    "body": body,
                    "todos": todos,
                })),
            )
            .await?;
        }
        NotesOp::ToggleTodo { note_id, index } => {
            let current: Vec<AgentNote> = parse(&crate::transport::fetch_json(&list_path).await?);
            if let Some(note) = current.iter().find(|n| n.id == note_id) {
                let todos: Vec<serde_json::Value> = note
                    .todos
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        serde_json::json!({"text": t.text, "done": if i == index { !t.done } else { t.done }})
                    })
                    .collect();
                crate::transport::fetch_send(
                    "PATCH",
                    &format!("/api/notes/{note_id}"),
                    Some(&serde_json::json!({ "todos": todos })),
                )
                .await?;
            }
        }
        NotesOp::Delete { note_id } => {
            crate::transport::fetch_send("DELETE", &format!("/api/notes/{note_id}"), None).await?;
        }
    }
    Ok(parse(&crate::transport::fetch_json(&list_path).await?))
}

/// Drain the notes inbox. Shared by every host.
pub fn poll_notes_state(s: &mut NotesState) -> bool {
    let (msgs, _) = crate::rooms::drain_results(&mut s.inbox);
    let mut changed = false;
    for msg in msgs {
        match msg {
            Ok(notes) => {
                s.notes = notes;
                s.error = None;
            }
            Err(message) => {
                s.error = Some(message);
            }
        }
        s.busy = false;
        changed = true;
    }
    changed
}

/// Header button with the count badge (`NotesButton` trigger).
pub fn notes_button<S: crate::rune_transcript::TranscriptHost + 'static>(
    s: &NotesState,
    which: crate::rune_transcript::Thread,
    scope_id: String,
) -> El {
    let mut btn = rune::el()
        .key("notes-open")
        .role("button")
        .focusable(true)
        .label("Open sticky notes")
        .size(32.0, 32.0)
        .radius(11.0)
        .fill(ink("#101013"))
        .border(1.0, ink("#ffffff14"))
        .center()
        .on_press_enter(move |s: &mut S| {
            let thread = s.thread_mut(which);
            let id = if scope_id.trim().is_empty() {
                thread.chat_id.clone()
            } else {
                scope_id.clone()
            };
            open_notes(&mut thread.notes, "chat", &id);
        })
        .child(
            text("✎".to_string())
                .font(rune::tok::TextStyle::new(14.0, 600.0))
                .color(ink("#c9c9d1")),
        );
    if !s.notes.is_empty() {
        btn = btn.child(
            rune::el()
                .key("notes-count")
                .absolute(20.0, -4.0)
                .size(15.0, 15.0)
                .radius(7.5)
                .fill(ink("#ffffff"))
                .center()
                .child(
                    text(s.notes.len().min(99).to_string())
                        .font(rune::tok::TextStyle::new(9.0, 700.0))
                        .color(ink("#0a0a0d")),
                ),
        );
    }
    btn
}

/// Notes sheet: dim backdrop + centred card, focus-trapped while mounted.
/// `s` is read directly; handlers resolve through the host + thread.
pub fn notes_sheet<S: crate::rune_transcript::TranscriptHost + 'static>(
    s: &NotesState,
    which: crate::rune_transcript::Thread,
) -> El {
    let label = rune::tok::TextStyle::new(12.0, 600.0);
    let field_text = rune::tok::TextStyle::new(12.0, 400.0);
    let mut list = col().key("notes-list").w_full().gap(10.0);
    if s.notes.is_empty() && !s.adding {
        list = list.child(
            text("No notes yet. Ask Stark to leave a note for this chat.".to_string())
                .key("notes-empty")
                .font(rune::tok::TextStyle::new(12.0, 400.0))
                .color(ink("#71717a")),
        );
    }
    for note in &s.notes {
        let note_id = note.id.clone();
        let mut card = col()
            .key(format!("note-{id}", id = note.id))
            .w_full()
            .gap(6.0)
            .radius(12.0)
            .fill(ink("#151311"))
            .border(1.0, ink("#fbbf2433"))
            .px(12.0)
            .py(10.0)
            .child(
                row()
                    .key(format!("note-{id}-head", id = note.id))
                    .w_full()
                    .items_center()
                    .gap(8.0)
                    .child(
                        text(note.title.clone())
                            .key(format!("note-{id}-title", id = note.id))
                            .grow(1.0)
                            .font(rune::tok::TextStyle::new(12.5, 600.0))
                            .color(ink("#f2f2f5")),
                    ),
            );
        if !note.body.trim().is_empty() {
            card = card.child(
                text(note.body.clone())
                    .key(format!("note-{id}-body", id = note.id))
                    .font(rune::tok::TextStyle::new(11.5, 400.0))
                    .wrap()
                    .color(ink("#a1a1aa")),
            );
        }
        for (index, todo) in note.todos.iter().enumerate() {
            let toggle_id = note_id.clone();
            card = card.child(
                row()
                    .key(format!("note-{id}-todo-{index}", id = note.id))
                    .w_full()
                    .items_center()
                    .gap(8.0)
                    .role("button")
                    .focusable(true)
                    .label(format!("Toggle {}", todo.text))
                    .on_press_enter(move |s: &mut S| {
                        dispatch_notes(
                            &mut s.thread_mut(which).notes,
                            NotesOp::ToggleTodo {
                                note_id: toggle_id.clone(),
                                index,
                            },
                        );
                    })
                    .child(
                        rune::el()
                            .key(format!("note-{id}-box-{index}", id = note.id))
                            .size(14.0, 14.0)
                            .radius(4.0)
                            .fill(ink(if todo.done { "#34d399" } else { "#00000000" }))
                            .border(1.0, ink(if todo.done { "#34d399" } else { "#71717a" }))
                            .center()
                            .child(
                                text(if todo.done { "✓" } else { "" }.to_string())
                                    .font(rune::tok::TextStyle::new(9.0, 700.0))
                                    .color(ink("#0a0a0d")),
                            ),
                    )
                    .child(
                        text(todo.text.clone())
                            .font(rune::tok::TextStyle::new(11.5, 400.0))
                            .color(ink(if todo.done { "#52525b" } else { "#d4d4d8" })),
                    ),
            );
        }
        // Delete is two-tap (no `window.confirm` natively): first tap arms.
        let armed = s.confirm_delete.as_deref() == Some(note.id.as_str());
        let del_id = note.id.clone();
        card = card.child(
            row()
                .key(format!("note-{id}-delrow", id = note.id))
                .w_full()
                .justify_end()
                .child(
                    text(if armed { "Confirm delete" } else { "Delete" }.to_string())
                        .key(format!("note-{id}-delete", id = note.id))
                        .role("button")
                        .focusable(true)
                        .label(format!("Delete {}", note.title))
                        .font(rune::tok::TextStyle::new(11.0, 600.0))
                        .px(10.0)
                        .py(6.0)
                        .radius(7.0)
                        .fill(ink(if armed { "#3a2226" } else { "#00000000" }))
                        .color(ink(if armed { "#fda4af" } else { "#71717a" }))
                        .on_press_enter(move |s: &mut S| {
                            let notes = &mut s.thread_mut(which).notes;
                            if notes.confirm_delete.as_deref() == Some(del_id.as_str()) {
                                notes.confirm_delete = None;
                                let id = del_id.clone();
                                dispatch_notes(notes, NotesOp::Delete { note_id: id });
                            } else {
                                notes.confirm_delete = Some(del_id.clone());
                            }
                        }),
                ),
        );
        list = list.child(card);
    }
    let mut sheet = col()
        .key("notes-card")
        .w(480.0)
        .radius(16.0)
        .fill(ink("#141417"))
        .border(1.0, ink("#ffffff14"))
        .px(16.0)
        .pt(12.0)
        .pb(14.0)
        .gap(10.0)
        .child(
            row()
                .key("notes-head")
                .w_full()
                .items_center()
                .gap(8.0)
                .child(
                    text("Notes".to_string())
                        .key("notes-title")
                        .grow(1.0)
                        .font(rune::tok::TextStyle::new(12.0, 650.0))
                        .color(ink("#a1a1aa")),
                )
                .child(
                    text(if s.adding { "Close" } else { "+ Add" }.to_string())
                        .key("notes-add")
                        .role("button")
                        .focusable(true)
                        .label("Add note")
                        .font(rune::tok::TextStyle::new(11.0, 600.0))
                        .px(10.0)
                        .py(6.0)
                        .radius(8.0)
                        .fill(ink("#ffffff"))
                        .color(ink("#0a0a0d"))
                        .on_press_enter(move |s: &mut S| {
                            let notes = &mut s.thread_mut(which).notes;
                            notes.adding = !notes.adding;
                        }),
                )
                .child(
                    rune::el()
                        .key("notes-close")
                        .size(14.0, 14.0)
                        .role("button")
                        .focusable(true)
                        .label("Close notes")
                        .shape(crate::lucide_x(14.0, 2.0, ink("#8a8a93")))
                        .on_press_enter(move |s: &mut S| {
                            s.thread_mut(which).notes.open = false;
                        }),
                ),
        );
    if s.adding {
        sheet = sheet.child(
            col()
                .key("notes-form")
                .w_full()
                .gap(8.0)
                .radius(12.0)
                .fill(ink("#151311"))
                .border(1.0, ink("#fbbf2440"))
                .px(12.0)
                .py(10.0)
                .child(
                    text_input(&s.title_edit, field_text)
                        .key("notes-title-input")
                        .w_full()
                        .placeholder("Title", ink("#52525b"))
                        .caret(ink("#f2f2f4"), crate::motion::TEXT_SELECTION)
                        .on_edit(move |s: &mut S| &mut s.thread_mut(which).notes.title_edit),
                )
                .child(
                    text_input(&s.body_edit, field_text)
                        .key("notes-body-input")
                        .w_full()
                        .placeholder("Note", ink("#52525b"))
                        .caret(ink("#f2f2f4"), crate::motion::TEXT_SELECTION)
                        .on_edit(move |s: &mut S| &mut s.thread_mut(which).notes.body_edit),
                )
                .child(
                    text_input(&s.todos_edit, field_text)
                        .key("notes-todos-input")
                        .w_full()
                        .placeholder("Checklist, one item per line", ink("#52525b"))
                        .caret(ink("#f2f2f4"), crate::motion::TEXT_SELECTION)
                        .on_edit(move |s: &mut S| &mut s.thread_mut(which).notes.todos_edit),
                )
                .child(
                    row()
                        .key("notes-form-actions")
                        .w_full()
                        .justify_end()
                        .gap(8.0)
                        .child(
                            text("Cancel".to_string())
                                .key("notes-cancel")
                                .role("button")
                                .focusable(true)
                                .label("Cancel")
                                .font(rune::tok::TextStyle::new(11.0, 400.0))
                                .color(ink("#a1a1aa"))
                                .on_press_enter(move |s: &mut S| {
                                    s.thread_mut(which).notes.adding = false;
                                }),
                        )
                        .child(
                            text(if s.busy { "Saving…" } else { "Save note" }.to_string())
                                .key("notes-save")
                                .role("button")
                                .focusable(true)
                                .label("Save note")
                                .font(label)
                                .px(10.0)
                                .py(6.0)
                                .radius(8.0)
                                .fill(ink("#fbbf24"))
                                .color(ink("#0a0a0d"))
                                .on_press_enter(move |s: &mut S| {
                                    let notes = &mut s.thread_mut(which).notes;
                                    let title = notes.title_edit.text.trim().to_string();
                                    let body = notes.body_edit.text.trim().to_string();
                                    let todos: Vec<String> = notes
                                        .todos_edit
                                        .text
                                        .lines()
                                        .map(|l| l.trim().to_string())
                                        .filter(|l| !l.is_empty())
                                        .collect();
                                    if title.is_empty() && body.is_empty() && todos.is_empty() {
                                        return;
                                    }
                                    notes.title_edit.text.clear();
                                    notes.body_edit.text.clear();
                                    notes.todos_edit.text.clear();
                                    notes.adding = false;
                                    dispatch_notes(notes, NotesOp::Create { title, body, todos });
                                }),
                        ),
                ),
        );
    }
    if let Some(err) = &s.error {
        sheet = sheet.child(
            text(err.clone())
                .key("notes-error")
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .color(ink("#fda4af")),
        );
    }
    sheet = sheet.child(list);
    rune::el()
        .key("notes-overlay")
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
            s.thread_mut(which).notes.open = false;
        })
        .child(sheet)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_parses_live_shape() {
        let v = serde_json::json!({
            "id": "n1", "title": "Ship it", "body": "before friday",
            "todos": [{"text": "tests", "done": true}, {"text": "docs"}],
        });
        let n = AgentNote::from_json(&v);
        assert_eq!(n.title, "Ship it");
        assert_eq!(n.todos.len(), 2);
        assert!(n.todos[0].done);
        assert!(!n.todos[1].done);
        let empty = AgentNote::from_json(&serde_json::json!({"id": "x"}));
        assert_eq!(empty.title, "");
        assert!(empty.todos.is_empty());
    }

    #[test]
    fn dispatch_guards_empty_scope() {
        let mut s = NotesState::default();
        assert!(s.scope_id.is_empty());
        dispatch_notes(&mut s, NotesOp::Refresh);
        assert!(!s.busy, "no worker without a scope");
        assert!(s.inbox.is_none());
    }
}
