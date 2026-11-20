//! Group-scope sticky notes over the project modal. The chat notes sheet is
//! transcript-owned; this is the same card for `scope_type = "group"`.

use rune::{col, row, text, text_input, El};

use crate::notes::{AgentNote, NotesOp, NotesState};
use crate::projects::ProjectsHost;

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

fn note_card<S: ProjectsHost + 'static>(note: &AgentNote, armed: bool) -> El {
    let id = note.id.clone();
    let del = note.id.clone();
    let mut card = col()
        .key(format!("gp-note-{}", note.id))
        .w_full()
        .gap(6.0)
        .radius(12.0)
        .border(1.0, ink("#fbbf2433"))
        .border_layout()
        .fill(ink("#151311"))
        .px(12.0)
        .py(10.0)
        .child(
            text(note.title.clone())
                .font(rune::tok::TextStyle::new(12.5, 600.0))
                .color(ink("#f2f2f5")),
        );
    if !note.body.trim().is_empty() {
        card = card.child(
            text(note.body.clone())
                .font(rune::tok::TextStyle::new(11.5, 400.0))
                .color(ink("#a1a1aa")),
        );
    }
    for (index, todo) in note.todos.iter().enumerate() {
        let todo_id = note.id.clone();
        let done = todo.done;
        card = card.child(
            row()
                .items_center()
                .gap(6.0)
                .role("button")
                .label(format!("Toggle {}", todo.text))
                .on_press(move |s: &mut S| {
                    crate::notes::dispatch_notes(
                        &mut s.projects_mut().group.notes,
                        NotesOp::ToggleTodo {
                            note_id: todo_id.clone(),
                            index,
                        },
                    );
                })
                .child(
                    text(if done { "☑" } else { "☐" }.to_string())
                        .font(rune::tok::TextStyle::new(11.0, 400.0))
                        .color(if done { ink("#fbbf24") } else { ink("#71717a") }),
                )
                .child(
                    text(todo.text.clone())
                        .font(rune::tok::TextStyle::new(11.0, 400.0))
                        .color(if done { ink("#71717a") } else { ink("#d4d4d8") }),
                ),
        );
    }
    card = card.child(
        row().w_full().justify_end().child(
            text(if armed { "Confirm delete" } else { "Delete" }.to_string())
                .font(rune::tok::TextStyle::new(10.5, 600.0))
                .color(if armed {
                    ink("#fda4af")
                } else {
                    ink("#71717a")
                })
                .role("button")
                .label(format!("Delete {}", note.title))
                .on_press(move |s: &mut S| {
                    let notes = &mut s.projects_mut().group.notes;
                    if notes.confirm_delete.as_deref() == Some(del.as_str()) {
                        notes.confirm_delete = None;
                        let id = del.clone();
                        crate::notes::dispatch_notes(notes, NotesOp::Delete { note_id: id });
                    } else {
                        notes.confirm_delete = Some(del.clone());
                    }
                }),
        ),
    );
    let _ = id;
    card
}

pub fn group_notes_sheet<S: ProjectsHost + 'static>(notes: &NotesState, project_id: String) -> El {
    let mut list = col().key("gp-notes-list").w_full().gap(10.0);
    if notes.notes.is_empty() && !notes.adding {
        list = list.child(
            text("No notes yet. Ask Stark to leave a note for this project.".to_string())
                .font(rune::tok::TextStyle::new(12.0, 400.0))
                .color(ink("#71717a")),
        );
    }
    for note in &notes.notes {
        list = list.child(note_card::<S>(
            note,
            notes.confirm_delete.as_deref() == Some(note.id.as_str()),
        ));
    }
    let field = rune::tok::TextStyle::new(12.0, 400.0);
    let mut sheet = col()
        .key("gp-notes-card")
        .w(480.0)
        .gap(10.0)
        .radius(16.0)
        .border(1.0, ink("#ffffff14"))
        .border_layout()
        .fill(ink("#141417"))
        .px(16.0)
        .pt(12.0)
        .pb(14.0)
        .child(
            row()
                .w_full()
                .items_center()
                .gap(8.0)
                .child(
                    text("Notes".to_string())
                        .grow(1.0)
                        .font(rune::tok::TextStyle::new(12.0, 650.0))
                        .color(ink("#a1a1aa")),
                )
                .child(
                    text(if notes.adding { "Close" } else { "+ Add" }.to_string())
                        .px(10.0)
                        .py(6.0)
                        .radius(8.0)
                        .font(rune::tok::TextStyle::new(11.0, 600.0))
                        .fill(ink("#ffffff"))
                        .color(ink("#0a0a0d"))
                        .role("button")
                        .label("Add note")
                        .on_press(|s: &mut S| {
                            let notes = &mut s.projects_mut().group.notes;
                            notes.adding = !notes.adding;
                        }),
                )
                .child(
                    text("✕".to_string())
                        .key("gp-notes-close")
                        .size(16.0, 16.0)
                        .center()
                        .font(rune::tok::TextStyle::new(13.0, 400.0))
                        .color(ink("#8a8a93"))
                        .role("button")
                        .label("Close notes")
                        .on_press(|s: &mut S| {
                            s.projects_mut().group.notes.open = false;
                        }),
                ),
        );
    if notes.adding {
        let mut form = col()
            .key("gp-notes-form")
            .w_full()
            .gap(8.0)
            .radius(12.0)
            .border(1.0, ink("#fbbf2440"))
            .border_layout()
            .fill(ink("#151311"))
            .px(12.0)
            .py(10.0);
        form = form
            .child(
                text_input(&notes.title_edit, field)
                    .key("gp-notes-title")
                    .w_full()
                    .placeholder("Title", ink("#52525b"))
                    .caret(ink("#f2f2f4"), ink("#fbbf24"))
                    .on_edit(|s: &mut S| &mut s.projects_mut().group.notes.title_edit),
            )
            .child(
                text_input(&notes.body_edit, field)
                    .key("gp-notes-body")
                    .w_full()
                    .placeholder("Note", ink("#52525b"))
                    .caret(ink("#f2f2f4"), ink("#fbbf24"))
                    .on_edit(|s: &mut S| &mut s.projects_mut().group.notes.body_edit),
            )
            .child(
                text_input(&notes.todos_edit, field)
                    .key("gp-notes-todos")
                    .w_full()
                    .placeholder("Checklist, one item per line", ink("#52525b"))
                    .caret(ink("#f2f2f4"), ink("#fbbf24"))
                    .on_edit(|s: &mut S| &mut s.projects_mut().group.notes.todos_edit),
            )
            .child(
                row()
                    .w_full()
                    .justify_end()
                    .gap(8.0)
                    .child(
                        text("Cancel".to_string())
                            .font(rune::tok::TextStyle::new(11.0, 400.0))
                            .color(ink("#a1a1aa"))
                            .role("button")
                            .label("Cancel note")
                            .on_press(|s: &mut S| {
                                s.projects_mut().group.notes.adding = false;
                            }),
                    )
                    .child(
                        text(if notes.busy { "Saving…" } else { "Save note" }.to_string())
                            .px(10.0)
                            .py(6.0)
                            .radius(8.0)
                            .font(rune::tok::TextStyle::new(11.0, 600.0))
                            .fill(ink("#fbbf24"))
                            .color(ink("#0a0a0d"))
                            .role("button")
                            .label("Save note")
                            .on_press(|s: &mut S| {
                                let notes = &mut s.projects_mut().group.notes;
                                let title = notes.title_edit.text.trim().to_string();
                                let body = notes.body_edit.text.trim().to_string();
                                let todos: Vec<String> = notes
                                    .todos_edit
                                    .text
                                    .lines()
                                    .map(str::trim)
                                    .filter(|l| !l.is_empty())
                                    .map(str::to_string)
                                    .collect();
                                if title.is_empty() && body.is_empty() && todos.is_empty() {
                                    return;
                                }
                                notes.title_edit.text.clear();
                                notes.body_edit.text.clear();
                                notes.todos_edit.text.clear();
                                notes.adding = false;
                                crate::notes::dispatch_notes(
                                    notes,
                                    NotesOp::Create { title, body, todos },
                                );
                            }),
                    ),
            );
        sheet = sheet.child(form);
    }
    if let Some(error) = &notes.error {
        sheet = sheet.child(
            text(error.clone())
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .color(ink("#fda4af")),
        );
    }
    sheet = sheet.child(list);
    let _ = project_id;
    rune::el()
        .key("gp-notes-overlay")
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
        .modal(true, |s: &mut S| {
            s.projects_mut().group.notes.open = false;
        })
        .child(sheet)
}
