//! Group-projects board — port of `GroupProjectModal` list half.
//!
//! Live contract (verified against `stark-server`):
//! `GET /api/groups` → `{projects:[{id,title,status,spent_usd,
//! milestones_done,milestones_total,items_done,items_total,
//! awaiting_approval,goal,…}], scoped}`.
//! Read-only: detail/ops (`pause`/`resume`/`close`) stay server-side until
//! the modal port. Empty board is the honest state when none exist.

use rune::{col, row, text, El};

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

/// One board row. Progress prefers item counts, falls back to milestones.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GroupProject {
    pub id: String,
    pub title: String,
    pub status: String,
    pub done: u64,
    pub total: u64,
    pub waiting: u64,
}

impl GroupProject {
    pub fn from_json(v: &serde_json::Value) -> Self {
        let u64_of = |k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
        let title = v
            .get("title")
            .and_then(|x| x.as_str())
            .map(str::to_string)
            .filter(|t| !t.is_empty())
            .or_else(|| v.get("goal").and_then(|x| x.as_str()).map(str::to_string))
            .unwrap_or_default();
        let (mut done, mut total) = (u64_of("items_done"), u64_of("items_total"));
        if total == 0 {
            done = u64_of("milestones_done");
            total = u64_of("milestones_total");
        }
        Self {
            id: v
                .get("id")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            title,
            status: v
                .get("status")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            done,
            total,
            waiting: u64_of("awaiting_approval"),
        }
    }

    /// Completion fraction 0..1 (1 when the denominator is unknown-but-done).
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            return if self.done > 0 { 1.0 } else { 0.0 };
        }
        (self.done as f32 / self.total as f32).clamp(0.0, 1.0)
    }
}

/// Parsed `/api/groups` list.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProjectsSnapshot {
    pub projects: Vec<GroupProject>,
    /// Server-side scoping (`fetchGroupProjects(workspaceId)`): a coding
    /// workspace sees only its own folder's projects, everything else sees
    /// them all. The hook only carries the id; the server decides.
    pub scoped: bool,
}

impl ProjectsSnapshot {
    pub fn from_json(v: &serde_json::Value) -> Self {
        Self {
            projects: v
                .get("projects")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().map(GroupProject::from_json).collect())
                .unwrap_or_default(),
            scoped: v.get("scoped").and_then(|x| x.as_bool()).unwrap_or(false),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn fetch(host: &str, port: u16, workspace_id: &str) -> Result<Self, String> {
        // Mirrors `fetchGroupProjects`: server-side scoping, empty = global.
        let path = if workspace_id.trim().is_empty() {
            "/api/groups".to_string()
        } else {
            format!("/api/groups?workspace_id={}", workspace_id.trim())
        };
        let v = crate::transport::get_json(host, port, &path)?;
        Ok(Self::from_json(&v))
    }
}

pub struct ProjectsState {
    pub host: String,
    pub port: u16,
    /// Server-side scope (`fetchGroupProjects(workspaceId)`); empty = global.
    /// Chats are the only type now, so this stays empty (global) — the field
    /// remains because the server still scopes when a caller sets it.
    pub workspace_id: String,
    /// Last `scoped` flag the server reported (drives the rail badge).
    pub scoped: bool,
    pub projects: Vec<GroupProject>,
    pub loading: bool,
    pub error: Option<String>,
    pub last_ok: f64,
    pub auto: bool,
    pub inbox: Option<std::sync::mpsc::Receiver<Result<ProjectsSnapshot, String>>>,
    /// New-project form (`NewProjectDialog` inline half): open flag, goal
    /// draft, and the in-flight create (explicit press = go-ahead).
    pub new_open: bool,
    pub goal_edit: rune::TextEdit,
    pub creating: bool,
    /// Detail modal/chat/rail state (`GroupProjectModal`).
    pub group: crate::group::GroupState,
}

impl std::fmt::Debug for ProjectsState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectsState")
            .field("projects", &self.projects.len())
            .field("loading", &self.loading)
            .field("error", &self.error)
            .finish()
    }
}

impl Clone for ProjectsState {
    fn clone(&self) -> Self {
        Self {
            host: self.host.clone(),
            port: self.port,
            workspace_id: self.workspace_id.clone(),
            scoped: self.scoped,
            projects: self.projects.clone(),
            loading: self.loading,
            error: self.error.clone(),
            last_ok: self.last_ok,
            auto: self.auto,
            inbox: None,
            new_open: self.new_open,
            goal_edit: self.goal_edit.clone(),
            creating: self.creating,
            group: self.group.clone(),
        }
    }
}

impl Default for ProjectsState {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8124,
            workspace_id: String::new(),
            scoped: false,
            projects: Vec::new(),
            loading: false,
            error: None,
            last_ok: 0.0,
            auto: true,
            inbox: None,
            new_open: false,
            goal_edit: rune::TextEdit::new(""),
            creating: false,
            group: crate::group::GroupState::default(),
        }
    }
}

/// Host access for the board.
pub trait ProjectsHost {
    fn projects_mut(&mut self) -> &mut ProjectsState;
}

/// Spawn a worker refresh. No-op while one is in flight.
pub fn refresh_projects(s: &mut ProjectsState) {
    if s.loading {
        return;
    }
    s.loading = true;
    s.error = None;
    let (tx, rx) = std::sync::mpsc::channel();
    s.inbox = Some(rx);
    let ws = s.workspace_id.clone();
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let path = if ws.trim().is_empty() {
            "/api/groups".to_string()
        } else {
            format!("/api/groups?workspace_id={}", ws.trim())
        };
        let result = match crate::transport::fetch_json(&path).await {
            Ok(v) => Ok(ProjectsSnapshot::from_json(&v)),
            Err(e) => Err(e),
        };
        let _ = tx.send(result);
    });
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (host, port) = (s.host.clone(), s.port);
        std::thread::spawn(move || {
            let _ = tx.send(ProjectsSnapshot::fetch(&host, port, &ws));
        });
    }
}

/// Drain the inbox and auto-refresh at most every 15s (the supervisor ticks
/// every 15s; polling faster is waste — `useGroupProjects` POLL_MS). Shared
/// by every host so the board, the rail and the shell drain identically.
pub fn poll_projects_state(s: &mut ProjectsState, clock: f64) -> bool {
    let (msgs, _) = crate::rooms::drain_results(&mut s.inbox);
    let mut changed = false;
    for msg in msgs {
        match msg {
            Ok(snap) => {
                s.projects = snap.projects;
                s.scoped = snap.scoped;
                s.error = None;
                s.last_ok = clock;
            }
            Err(message) => {
                s.error = Some(message);
            }
        }
        s.loading = false;
        s.creating = false;
        changed = true;
    }
    if s.auto && !s.loading && clock - s.last_ok > 15.0 {
        refresh_projects(s);
        changed = true;
    }
    changed
}
/// Create a project (`createGroupProject`: `POST /api/groups
/// {goal, workspace_id}`), then re-fetch the list into the same inbox.
/// Explicit press = user go-ahead. No-op on empty goal or while busy.
pub fn create_project(s: &mut ProjectsState) {
    let goal = s.goal_edit.text.trim().to_string();
    if goal.is_empty() || s.loading || s.creating {
        return;
    }
    s.creating = true;
    s.error = None;
    let (tx, rx) = std::sync::mpsc::channel();
    s.inbox = Some(rx);
    #[cfg(not(target_arch = "wasm32"))]
    let (host, port, ws) = (s.host.clone(), s.port, s.workspace_id.clone());
    #[cfg(target_arch = "wasm32")]
    let ws = s.workspace_id.clone();
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let body = serde_json::json!({ "goal": goal, "workspace_id": ws });
        let result = match crate::transport::fetch_send("POST", "/api/groups", Some(&body)).await {
            Err(e) => Err(e),
            Ok(_) => {
                let path = if ws.trim().is_empty() {
                    "/api/groups".to_string()
                } else {
                    format!("/api/groups?workspace_id={}", ws.trim())
                };
                match crate::transport::fetch_json(&path).await {
                    Ok(v) => Ok(ProjectsSnapshot::from_json(&v)),
                    Err(e) => Err(e),
                }
            }
        };
        let _ = tx.send(result);
    });
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let body = serde_json::json!({ "goal": goal, "workspace_id": ws });
        let created =
            crate::transport::send_json(&host, port, "POST", "/api/groups", &body).map(|_| ());
        let result = created.and_then(|_| ProjectsSnapshot::fetch(&host, port, &ws));
        let _ = tx.send(result);
    });
}

/// Projects sidebar section: the Group panel's project list
/// (`useGroupProjects` + `GroupProjectModal` list half). Scoped by
/// `ProjectsState.workspace_id` (empty = global, the only mode now — chats
/// are the only type); the `scoped` badge shows what the server applied.
pub fn projects_section<S: ProjectsHost + 'static>(s: &ProjectsState) -> El {
    let mut body = col().key("side-projects").w_full().gap(6.0).pt(10.0).child(
        row()
            .key("side-projects-head")
            .w_full()
            .items_center()
            .gap(6.0)
            .child(
                text("Projects".to_string())
                    .key("side-projects-title")
                    .grow(1.0)
                    .font(rune::tok::TextStyle::new(11.0, 600.0))
                    .color(ink("#8a8a93")),
            )
            .child(
                text(if s.scoped { "Scoped" } else { "All" }.to_string())
                    .key("side-projects-scope")
                    .font(rune::tok::TextStyle::new(9.0, 600.0))
                    .px(6.0)
                    .py(2.0)
                    .radius(5.0)
                    .fill(ink("#ffffff0d"))
                    .color(ink("#71717a")),
            )
            .child(
                text(if s.loading { "…" } else { "↻" }.to_string())
                    .key("side-projects-refresh")
                    .role("button")
                    .focusable(true)
                    .label("Refresh projects")
                    .help("Refresh projects")
                    .font(rune::tok::TextStyle::new(12.0, 600.0))
                    .color(ink("#a1a1aa"))
                    .on_press(|s: &mut S| {
                        refresh_projects(s.projects_mut());
                    }),
            ),
    );
    if s.projects.is_empty() && !s.loading {
        body = body.child(
            text("No projects".to_string())
                .key("side-projects-empty")
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .color(ink("#52525b")),
        );
    }
    for p in &s.projects {
        body = body.child(project_card::<S>("side-projects", p, true));
    }
    if p_waiting_note(s) {
        body = body.child(
            text("waiting on you".to_string())
                .key("side-projects-waiting")
                .font(rune::tok::TextStyle::new(10.0, 600.0))
                .color(ink("#fcd34d")),
        );
    }
    if let Some(err) = &s.error {
        body = body.child(
            text(err.clone())
                .key("side-projects-error")
                .role("alert")
                .live_region(rune::LiveRegion::Assertive)
                .font(rune::tok::TextStyle::new(10.0, 400.0))
                .color(ink("#fda4af")),
        );
    }
    // New project (`NewProjectDialog` inline half): toggle + goal + create.
    body = body.child(
        text(if s.new_open { "Close" } else { "+ New project" }.to_string())
            .key("side-projects-new")
            .role("button")
            .focusable(true)
            .label("New project")
            .help("New group project")
            .font(rune::tok::TextStyle::new(11.0, 600.0))
            .px(10.0)
            .py(6.0)
            .radius(8.0)
            .fill(ink("#ffffff0d"))
            .color(ink("#d4d4d8"))
            .on_press(|s: &mut S| {
                s.projects_mut().new_open = !s.projects_mut().new_open;
            }),
    );
    if s.new_open {
        body = body.child(
            col()
                .key("side-projects-form")
                .w_full()
                .gap(8.0)
                .radius(12.0)
                .fill(ink("#141417"))
                .border(1.0, ink("#ffffff14"))
                .px(10.0)
                .py(8.0)
                .child(
                    rune::text_input(&s.goal_edit, rune::tok::TextStyle::new(12.0, 400.0))
                        .key("side-projects-goal")
                        .w_full()
                        .placeholder("Project goal…", ink("#52525b"))
                        .caret(ink("#f2f2f4"), ink("#3b82f6"))
                        .on_edit(move |s: &mut S| &mut s.projects_mut().goal_edit)
                        // Enter in the goal submits the form, like the dialog.
                        .on_submit(|s: &mut S, _tx: &mut rune::Tx| {
                            let p = s.projects_mut();
                            if !p.creating {
                                create_project(p);
                                p.new_open = false;
                                p.goal_edit.text.clear();
                            }
                        }),
                )
                .child(
                    text(if s.creating { "Creating…" } else { "Create" }.to_string())
                        .key("side-projects-create")
                        .role("button")
                        .focusable(true)
                        .label("Create project")
                        .font(rune::tok::TextStyle::new(11.0, 600.0))
                        .px(10.0)
                        .py(6.0)
                        .radius(8.0)
                        .fill(ink("#ffffff"))
                        .color(ink("#0a0a0d"))
                        .on_press(move |s: &mut S| {
                            let p = s.projects_mut();
                            create_project(p);
                            p.new_open = false;
                            p.goal_edit.text.clear();
                        }),
                ),
        );
    }
    body
}

/// One project card. `prefix` namespaces keys (`projects` board vs
/// `side-projects` section); `compact` tightens padding/type for the rail.
fn project_card<S: ProjectsHost + 'static>(prefix: &str, p: &GroupProject, compact: bool) -> El {
    let (pad_x, pad_y, title_size) = if compact {
        (10.0, 8.0, 12.0)
    } else {
        (14.0, 12.0, 13.0)
    };
    let open_id = p.id.clone();
    let mut card = col()
        .key(format!("{prefix}-{id}", id = p.id))
        .w_full()
        .gap(if compact { 6.0 } else { 8.0 })
        .radius(12.0)
        .fill(ink("#141417"))
        .border(1.0, ink("#ffffff14"))
        .px(pad_x)
        .py(pad_y)
        .role("button")
        .focusable(true)
        .label(format!("Open project {}", p.title))
        .focusable(true)
        .focus_ring(2.0, 2.0, ink("#ffffff8c"))
        .hover(ink("#ffffff0a"))
        .on_press(move |s: &mut S| {
            let projects = s.projects_mut();
            let (host, port) = (projects.host.clone(), projects.port);
            projects.group.open(host, port, open_id.clone());
        })
        .child(
            row()
                .key(format!("{prefix}-{id}-head", id = p.id))
                .w_full()
                .items_center()
                .gap(8.0)
                .child(
                    text(p.title.clone())
                        .key(format!("{prefix}-{id}-title", id = p.id))
                        .grow(1.0)
                        .font(rune::tok::TextStyle::new(title_size, 600.0))
                        .color(ink("#e4e4e7")),
                )
                .child(
                    text(p.status.clone())
                        .key(format!("{prefix}-{id}-status", id = p.id))
                        .font(rune::tok::TextStyle::new(10.0, 600.0))
                        .px(8.0)
                        .py(3.0)
                        .radius(6.0)
                        .fill(ink("#ffffff0d"))
                        .color(ink("#a1a1aa")),
                ),
        )
        .child(progress_bar(
            &format!("{prefix}-{id}", id = p.id),
            p.fraction(),
        ))
        .child(
            text(format!("{} of {} items", p.done, p.total))
                .key(format!("{prefix}-{id}-counts", id = p.id))
                .font(rune::tok::TextStyle::new(10.0, 400.0))
                .color(ink("#71717a")),
        );
    if p.waiting > 0 {
        card = card.child(
            text(format!("{} waiting", p.waiting))
                .key(format!("{prefix}-{id}-waiting", id = p.id))
                .font(rune::tok::TextStyle::new(10.0, 600.0))
                .color(ink("#fcd34d")),
        );
    }
    card
}

fn p_waiting_note(s: &ProjectsState) -> bool {
    s.projects.iter().any(|p| p.waiting > 0)
}

/// 4pt progress track with a fractional fill. The fill width is a fraction of
/// the track: implemented as a row where the fill grows and a spacer takes
/// the rest, so no absolute measuring is needed. `stem` is the full key stem
/// (`projects-p1`, `side-projects-p1`) — the bar/fill/rest keys hang off it.
fn progress_bar(stem: &str, fraction: f32) -> El {
    let fill = (fraction.clamp(0.0, 1.0) * 1000.0).round() / 10.0;
    row()
        .key(format!("{stem}-bar"))
        .w_full()
        .h(4.0)
        .radius(2.0)
        .fill(ink("#ffffff12"))
        .child(
            rune::el()
                .key(format!("{stem}-fill"))
                .h_full()
                .radius(2.0)
                .fill(ink("#d4d4d8"))
                // Fractional width via flex weight out of 1000.
                .grow(fill.max(1.0))
                .min_h(0.0),
        )
        .child(
            rune::el()
                .key(format!("{stem}-rest"))
                .grow((1000.0 - fill).max(1.0)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_parses_live_shape_and_fraction() {
        let v = serde_json::json!({
            "projects": [
                {"id": "p1", "title": "Website", "status": "doing",
                 "items_done": 3, "items_total": 4, "awaiting_approval": 1,
                 "milestones_done": 0, "milestones_total": 0},
                {"id": "p2", "goal": "Untitled goal", "status": "paused",
                 "items_done": 0, "items_total": 0,
                 "milestones_done": 1, "milestones_total": 2,
                 "awaiting_approval": 0},
            ],
            "scoped": false,
        });
        let snap = ProjectsSnapshot::from_json(&v);
        assert_eq!(snap.projects.len(), 2);
        assert_eq!(snap.projects[0].title, "Website");
        assert!((snap.projects[0].fraction() - 0.75).abs() < 1e-6);
        // Title falls back to goal; progress to milestones.
        assert_eq!(snap.projects[1].title, "Untitled goal");
        assert!((snap.projects[1].fraction() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn empty_board_parses() {
        let snap =
            ProjectsSnapshot::from_json(&serde_json::json!({"projects": [], "scoped": false}));
        assert!(snap.projects.is_empty());
    }
}
