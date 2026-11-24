//! Group-projects board — port of `GroupProjectModal` list half.
//!
//! Live contract (verified against `stark-server`):
//! `GET /api/groups` → `{projects:[{id,title,status,spent_usd,
//! milestones_done,milestones_total,items_done,items_total,
//! awaiting_approval,goal,…}], scoped}`.
//! Read-only: detail/ops (`pause`/`resume`/`close`) stay server-side until
//! The modal port. Empty board is the honest state when none exist.

use crate::keys::EnterPress;
use rune::{col, row, text, El};

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

fn rgba(r: f32, g: f32, b: f32, a: f32) -> rune::Color {
    rune::Color { r, g, b, a }
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

/// The two-step `NewProjectDialog`'s working mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NewProjectMode {
    #[default]
    Group,
    Solo,
}

/// `NewProjectDialog` state: step 1 is the folder + name, step 2 is the
/// task + group/solo choice. Nothing is created until step 2, so Back never
/// leaves a stray workspace behind.
pub struct NewProjectDialog {
    pub open: bool,
    /// 1 = folder/name, 2 = task/mode.
    pub step: u8,
    pub mode: NewProjectMode,
    pub name: rune::TextEdit,
    pub path: rune::TextEdit,
    pub task: rune::TextEdit,
    pub busy: bool,
    pub error: Option<String>,
    pub inbox: Option<std::sync::mpsc::Receiver<Result<NewProjectCreated, String>>>,
    /// Paint-only hover tracks for the dialog's borders and text colours.
    pub hover: crate::motion::HoverSet,
}

impl Default for NewProjectDialog {
    fn default() -> Self {
        Self {
            open: false,
            step: 1,
            mode: NewProjectMode::Group,
            name: rune::TextEdit::new(""),
            path: rune::TextEdit::new(""),
            task: rune::TextEdit::new(""),
            busy: false,
            error: None,
            inbox: None,
            hover: crate::motion::HoverSet::default(),
        }
    }
}

impl Clone for NewProjectDialog {
    fn clone(&self) -> Self {
        Self {
            open: self.open,
            step: self.step,
            mode: self.mode,
            name: self.name.clone(),
            path: self.path.clone(),
            task: self.task.clone(),
            busy: self.busy,
            error: self.error.clone(),
            inbox: None,
            hover: self.hover.clone(),
        }
    }
}

/// What step 2 created: the workspace + its first chat, plus the group
/// project when the team mode was chosen. The shell opens the chat and the
/// project modal after the worker sends this.
#[derive(Debug, Clone, PartialEq)]
pub struct NewProjectCreated {
    pub workspace: crate::chats::ChatWorkspace,
    pub chat_id: String,
    pub chat_title: String,
    pub project_id: Option<String>,
    /// Solo mode seeds this prompt into the new chat's dock.
    pub prompt: String,
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
    /// `NewProjectDialog`: the full two-step folder/name → task/mode flow.
    pub dialog: NewProjectDialog,
    /// Creation finished; the shell takes it once and opens the chat/project.
    pub created: Option<NewProjectCreated>,
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
            dialog: self.dialog.clone(),
            created: self.created.clone(),
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
            dialog: NewProjectDialog::default(),
            created: None,
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
        changed = true;
    }
    let (created, dead) = crate::rooms::drain_results(&mut s.dialog.inbox);
    for result in created {
        s.dialog.busy = false;
        match result {
            Ok(done) => {
                s.dialog.open = false;
                s.dialog.error = None;
                s.created = Some(done);
            }
            Err(message) => {
                s.dialog.error = Some(message);
            }
        }
        changed = true;
    }
    if dead && s.dialog.busy {
        // The worker dropped without an answer: never leave the modal stuck.
        s.dialog.busy = false;
        if s.dialog.error.is_none() {
            s.dialog.error = Some("Could not create the project.".to_string());
        }
        changed = true;
    }
    if s.auto && !s.loading && clock - s.last_ok > 15.0 {
        refresh_projects(s);
        changed = true;
    }
    changed
}

/// Take the workspace + chat + project a finished dialog created. The shell
/// opens them once, so polling can never reopen a chat the user closed.
pub fn take_new_project(s: &mut ProjectsState) -> Option<NewProjectCreated> {
    s.created.take()
}

fn reset_focus(d: &mut NewProjectDialog) {
    d.name.focused = false;
    d.path.focused = false;
    d.task.focused = false;
}

/// Open `NewProjectDialog` on step 1, as a fresh mount: the reference UI's dialog owns
/// its name/path/task state, so reopening starts empty.
pub fn open_new_project(s: &mut ProjectsState) {
    let d = &mut s.dialog;
    if d.busy {
        return;
    }
    d.open = true;
    d.step = 1;
    d.mode = NewProjectMode::Group;
    d.error = None;
    // Fresh mount: no stale hover track from the previous open.
    d.hover.retain(|_| false);
    d.name.text.clear();
    d.path.text.clear();
    d.task.text.clear();
    d.name.anchor = 0;
    d.name.focus = 0;
    d.path.anchor = 0;
    d.path.focus = 0;
    d.task.anchor = 0;
    d.task.focus = 0;
    reset_focus(d);
    d.name.focused = true;
}

/// Close the dialog; busy creations keep it up (the reference UI disables close/cancel
/// until the create settles).
pub fn close_new_project(s: &mut ProjectsState) {
    let d = &mut s.dialog;
    if d.busy {
        return;
    }
    d.open = false;
    reset_focus(d);
}

/// Step 1 → 2. Enter in the name or path input calls this; an empty path
/// keeps step 1 and reports the same message as the reference UI.
pub fn new_project_next(s: &mut ProjectsState) {
    let d = &mut s.dialog;
    if d.busy || !d.open {
        return;
    }
    if d.path.text.trim().is_empty() {
        // The reference UI keeps the current focus and only shows the note.
        d.error = Some("Choose a folder first.".to_string());
        return;
    }
    d.error = None;
    d.step = 2;
    reset_focus(d);
    d.task.focused = true;
}

/// Group/solo radiogroup selection; clears the validation note like the reference UI.
pub fn new_project_mode(s: &mut ProjectsState, mode: NewProjectMode) {
    let d = &mut s.dialog;
    if d.busy {
        return;
    }
    d.mode = mode;
    d.error = None;
}

/// Step 2 → 1 without creating anything.
pub fn new_project_back(s: &mut ProjectsState) {
    let d = &mut s.dialog;
    if d.busy || !d.open {
        return;
    }
    d.step = 1;
    d.error = None;
    // The reference UI remounts step 1 without autofocus; the Back button simply goes.
    reset_focus(d);
}

fn json_id(v: &serde_json::Value) -> Option<String> {
    for key in ["id", "chat", "project"] {
        if let Some(id) = v
            .get(key)
            .and_then(|x| if key == "id" { Some(x) } else { x.get("id") })
            .and_then(|x| x.as_str())
            .filter(|id| !id.is_empty())
        {
            return Some(id.to_string());
        }
    }
    None
}

fn workspace_from_json(
    v: &serde_json::Value,
    path: &str,
    requested_name: &str,
) -> Result<crate::chats::ChatWorkspace, String> {
    let id = json_id(v)
        .ok_or_else(|| "Could not create the project — check the folder path.".to_string())?;
    let fallback_name = std::path::Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("Project")
        .to_string();
    let name = v
        .get("name")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if requested_name.is_empty() {
                fallback_name
            } else {
                requested_name.to_string()
            }
        });
    Ok(crate::chats::ChatWorkspace {
        id,
        name,
        kind: v
            .get("workspaceKind")
            .or_else(|| v.get("kind"))
            .and_then(|x| x.as_str())
            .unwrap_or("project")
            .to_string(),
        path: v
            .get("path")
            .and_then(|x| x.as_str())
            .unwrap_or(path)
            .to_string(),
        chat_count: v.get("chatCount").and_then(|x| x.as_i64()).unwrap_or(0),
        exists: v.get("exists").and_then(|x| x.as_bool()).unwrap_or(true),
    })
}

/// `POST /api/workspaces` → `POST /api/workspaces/{id}/chats` → optional
/// `POST /api/groups`, the exact sequence behind the reference UI's two-step dialog.
/// Nothing is created before this point, so Back has no side effects.
pub fn create_new_project(s: &mut ProjectsState) {
    let d = &mut s.dialog;
    if d.busy || !d.open {
        return;
    }
    let path = d.path.text.trim().to_string();
    let name = d.name.text.trim().to_string();
    let task = d.task.text.trim().to_string();
    if path.is_empty() {
        d.step = 1;
        d.error = Some("Choose a folder first.".to_string());
        return;
    }
    if d.mode == NewProjectMode::Group && task.is_empty() {
        d.error = Some("Tell the team what to build.".to_string());
        return;
    }
    d.busy = true;
    d.error = None;
    let (tx, rx) = std::sync::mpsc::channel();
    d.inbox = Some(rx);
    let (host, port) = (s.host.clone(), s.port);
    let mode = d.mode;
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let _ = tx.send(complete_new_project(&host, port, &name, &path, &task, mode));
    });
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (host, port);
        wasm_bindgen_futures::spawn_local(async move {
            let _ = tx.send(complete_new_project_wasm(name, path, task, mode).await);
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn complete_new_project(
    host: &str,
    port: u16,
    name: &str,
    path: &str,
    task: &str,
    mode: NewProjectMode,
) -> Result<NewProjectCreated, String> {
    let workspace = crate::transport::send_json(
        host,
        port,
        "POST",
        "/api/workspaces",
        &serde_json::json!({ "path": path, "name": name }),
    )
    .map_err(|_| "Could not create the project — check the folder path.".to_string())
    .and_then(|v| workspace_from_json(&v, path, name))?;
    let chat = crate::transport::send_json(
        host,
        port,
        "POST",
        &format!("/api/workspaces/{}/chats", workspace.id),
        &serde_json::json!({ "title": "New Chat", "model": "Automatic Fallback" }),
    )
    // The reference UI's `handleCreateProject` returns null on any failure, so the
    // dialog reports the folder-path copy for a failed chat too.
    .map_err(|_| "Could not create the project — check the folder path.".to_string())?;
    let chat_id = json_id(&chat)
        .ok_or_else(|| "Could not create the project — check the folder path.".to_string())?;
    let chat_title = chat
        .get("title")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("New Chat")
        .to_string();
    let project_id = if mode == NewProjectMode::Group {
        let project = crate::transport::send_json(
            host,
            port,
            "POST",
            "/api/groups",
            &serde_json::json!({ "goal": task, "workspace_id": workspace.id }),
        )
        .map_err(|error| format!("Could not create the group project: {error}"))?;
        Some(
            json_id(&project)
                .ok_or_else(|| "The backend did not return a project id.".to_string())?,
        )
    } else {
        None
    };
    Ok(NewProjectCreated {
        workspace,
        chat_id,
        chat_title,
        project_id,
        prompt: if mode == NewProjectMode::Solo {
            task.to_string()
        } else {
            String::new()
        },
    })
}

#[cfg(target_arch = "wasm32")]
async fn complete_new_project_wasm(
    name: String,
    path: String,
    task: String,
    mode: NewProjectMode,
) -> Result<NewProjectCreated, String> {
    let workspace = crate::transport::fetch_send(
        "POST",
        "/api/workspaces",
        Some(&serde_json::json!({ "path": &path, "name": &name })),
    )
    .await
    .map_err(|_| "Could not create the project — check the folder path.".to_string())
    .and_then(|v| workspace_from_json(&v, &path, &name))?;
    let chat = crate::transport::fetch_send(
        "POST",
        &format!("/api/workspaces/{}/chats", workspace.id),
        Some(&serde_json::json!({ "title": "New Chat", "model": "Automatic Fallback" })),
    )
    .await
    .map_err(|_| "Could not create the project — check the folder path.".to_string())?;
    let chat_id = json_id(&chat)
        .ok_or_else(|| "Could not create the project — check the folder path.".to_string())?;
    let chat_title = chat
        .get("title")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("New Chat")
        .to_string();
    let project_id = if mode == NewProjectMode::Group {
        let project = crate::transport::fetch_send(
            "POST",
            "/api/groups",
            Some(&serde_json::json!({ "goal": &task, "workspace_id": &workspace.id })),
        )
        .await
        .map_err(|error| format!("Could not create the group project: {error}"))?;
        Some(
            json_id(&project)
                .ok_or_else(|| "The backend did not return a project id.".to_string())?,
        )
    } else {
        None
    };
    Ok(NewProjectCreated {
        workspace,
        chat_id,
        chat_title,
        project_id,
        prompt: if mode == NewProjectMode::Solo {
            task
        } else {
            String::new()
        },
    })
}

/// Projects sidebar section: the Group panel's project list.
///
/// Scoped by `ProjectsState.workspace_id` (empty = global); the `scoped`
/// badge shows what the server applied. Cards keep their live keys so the
/// shell tests and hit targets stay stable.
pub fn projects_section<S: ProjectsHost + 'static>(s: &ProjectsState) -> El {
    let count = s.projects.len();
    let refresh_glyph = crate::icons::lucide(
        &["M21 12a9 9 0 1 1-2.64-6.36", "M21 3v6h-6"],
        13.0,
        2.0,
        ink(if s.loading { "#52525b" } else { "#a1a1aa" }),
    );
    let mut body = col()
        .key("side-projects")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .gap(10.0)
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        })
        .child(
            row()
                .key("side-projects-head")
                .w_full()
                .items_center()
                .gap(6.0)
                .child(
                    text("Projects".to_string())
                        .key("side-projects-title")
                        .font(rune::tok::TextStyle::new(12.5, 650.0))
                        .color(ink("#f4f4f5")),
                )
                .child(
                    text(count.to_string())
                        .key("side-projects-count")
                        .font(rune::tok::TextStyle::new(9.0, 650.0))
                        .px(5.0)
                        .py(1.0)
                        .radius(5.0)
                        .fill(ink("#ffffff0d"))
                        .color(ink("#a1a1aa")),
                )
                .child(rune::el().grow(1.0))
                .child(
                    text(if s.scoped { "Scoped" } else { "All" }.to_string())
                        .key("side-projects-scope")
                        .font(rune::tok::TextStyle::new(8.5, 600.0))
                        .color(ink("#52525b")),
                )
                .child(
                    rune::el()
                        .key("side-projects-refresh")
                        .role("button")
                        .focusable(true)
                        .label("Refresh projects")
                        .help("Refresh projects")
                        .size(22.0, 22.0)
                        .radius(6.0)
                        .center()
                        .hover(ink("#ffffff10"))
                        .on_press_enter(|s: &mut S| {
                            refresh_projects(s.projects_mut());
                        })
                        .child(rune::el().size(13.0, 13.0).inert().shape(refresh_glyph)),
                ),
        )
        .child(
            text("Split work across agents.".to_string())
                .font(rune::tok::TextStyle::new(10.5, 400.0))
                .color(ink("#71717a")),
        )
        .child(
            row()
                .key("side-projects-new")
                .w_full()
                .items_center()
                .justify_center()
                .gap(7.0)
                .py(9.0)
                .radius(10.0)
                .fill(ink("#ffffff0a"))
                .border(1.0, ink("#ffffff1a"))
                .border_layout()
                .role("button")
                .focusable(true)
                .label("New project")
                .help("New group project")
                .hover(ink("#ffffff14"))
                .on_press_enter(|s: &mut S| open_new_project(s.projects_mut()))
                .child(
                    rune::el()
                        .size(12.0, 12.0)
                        .inert()
                        .shape(crate::icons::plus(12.0, ink("#e4e4e7"))),
                )
                .child(
                    text("New project".to_string())
                        .font(rune::tok::TextStyle::new(11.0, 600.0))
                        .color(ink("#e4e4e7")),
                ),
        );

    if s.projects.is_empty() && !s.loading {
        body = body.child(
            col()
                .key("side-projects-empty")
                .w_full()
                .items_center()
                .gap(6.0)
                .py(18.0)
                .child(
                    rune::el()
                        .size(22.0, 22.0)
                        .inert()
                        .shape(crate::icons::bot(22.0, ink("#3f3f46"))),
                )
                .child(
                    text("No projects yet".to_string())
                        .font(rune::tok::TextStyle::new(11.0, 600.0))
                        .color(ink("#a1a1aa")),
                )
                .child(
                    text("Create one to hand a goal to a team.".to_string())
                        .font(rune::tok::TextStyle::new(9.5, 400.0))
                        .color(ink("#52525b")),
                ),
        );
    } else if !s.projects.is_empty() {
        body = body.child(
            text("ACTIVE".to_string())
                .font(rune::tok::TextStyle::new(8.5, 700.0))
                .color(ink("#52525b")),
        );
    }

    // Waiting projects first, otherwise server order (stable sort).
    let mut ordered: Vec<&GroupProject> = s.projects.iter().collect();
    ordered.sort_by_key(|p| std::cmp::Reverse(p.waiting));
    for p in ordered {
        body = body.child(project_card::<S>("side-projects", p, true));
    }
    if p_waiting_note(s) {
        body = body.child(
            text("Waiting on your approval".to_string())
                .key("side-projects-waiting")
                .font(rune::tok::TextStyle::new(9.5, 500.0))
                .color(ink("#a1a1aa")),
        );
    }
    if let Some(err) = &s.error {
        body = body.child(
            text(err.clone())
                .key("side-projects-error")
                .role("alert")
                .live_region(rune::LiveRegion::Assertive)
                .font(rune::tok::TextStyle::new(10.0, 400.0))
                .color(ink("#f87171")),
        );
    }
    body
}

fn dialog_style(size: f32, weight: f32) -> rune::tok::TextStyle {
    let mut style = rune::tok::TextStyle::new(size, weight);
    style.line_height = size * 1.5;
    style
}

fn dialog_snug(size: f32, weight: f32) -> rune::tok::TextStyle {
    let mut style = rune::tok::TextStyle::new(size, weight);
    style.line_height = size * 1.375;
    style
}

fn pick_project_folder<S: ProjectsHost + 'static>(_s: &mut S, tx: &mut rune::Tx) {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = tx;
        _s.projects_mut().dialog.error = Some(
            "Choosing a folder is available in the desktop app. Paste a path instead.".to_string(),
        );
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let options = rune::FileDialogOptions {
            title: "Choose a folder".to_string(),
            ..Default::default()
        };
        tx.open_folder(options, |s: &mut S, result, _tx| match result {
            Ok(Some(path)) => {
                let d = &mut s.projects_mut().dialog;
                d.path.text = path.to_string_lossy().into_owned();
                d.path.anchor = d.path.text.len();
                d.path.focus = d.path.text.len();
            }
            Ok(None) => {}
            Err(error) => s.projects_mut().dialog.error = Some(error.to_string()),
        });
    }
}

/// The live New Project dialog from the original design:
/// a 390 pt two-step card over a 60% blurred backdrop. Step 1 creates
/// nothing; step 2 registers the workspace, creates its first chat, and (in
/// group mode) the durable team board.
pub fn new_project_dialog<S: ProjectsHost + 'static>(s: &ProjectsState, w: f32, _h: f32) -> El {
    let d = &s.dialog;
    if !d.open {
        return col().key("new-project-empty");
    }
    let busy = d.busy;
    let step = d.step.clamp(1, 2);
    let panel_w = 390.0f32.min((w - 32.0).max(0.0));
    let path = d.path.text.trim().to_string();
    let path_empty = path.is_empty();
    let task_empty = d.task.text.trim().is_empty();
    let group = d.mode == NewProjectMode::Group;

    let step_icon = if step == 1 {
        crate::icons::folder_open(12.0, ink("#a1a1aa"))
    } else {
        crate::icons::sparkles(12.0, ink("#a1a1aa"))
    };
    let mut close = rune::el()
        .key("new-project-close")
        .size(24.0, 24.0)
        .radius(6.0)
        .center()
        .role("button")
        .focusable(!busy)
        .label("Close")
        .hover(ink("#ffffff14"))
        .hover_group()
        .on_press_enter(|s: &mut S| close_new_project(s.projects_mut()))
        .child(
            rune::el()
                .key("new-project-close-icon")
                .size(13.0, 13.0)
                .inert()
                .group_color(ink("#ffffff"))
                .shape(crate::icons::x(13.0, ink("#71717a"))),
        );
    if busy {
        close = close.opacity(0.4);
    }
    let head = row()
        .key("new-project-head")
        .w_full()
        .items_center()
        .justify_between()
        .px(16.0)
        .pt(14.0)
        .pb(8.0)
        .child(
            row()
                .key("new-project-head-copy")
                .items_center()
                .gap(8.0)
                .child(
                    rune::el()
                        .key("new-project-step-icon")
                        .size(24.0, 24.0)
                        .radius(8.0)
                        .border(1.0, ink("#ffffff14"))
                        .border_layout()
                        .fill(ink("#ffffff09"))
                        .center()
                        .child(
                            rune::el()
                                .key("new-project-step-glyph")
                                .size(12.0, 12.0)
                                .inert()
                                .shape(step_icon),
                        ),
                )
                .child(
                    text(
                        if step == 1 {
                            "New project · Folder"
                        } else {
                            "New project · Task"
                        }
                        .to_string(),
                    )
                    .key("new-project-eyebrow")
                    .font({
                        let mut style = dialog_style(10.0, 600.0);
                        style.tracking = 1.0;
                        style
                    })
                    .color(ink("#71717a")),
                ),
        )
        .child(close);

    let bar = |key: &str, on: bool| {
        rune::el().key(key).h(2.0).grow(1.0).radius(1.0).fill(rgba(
            1.0,
            1.0,
            1.0,
            if on { 0.55 } else { 0.10 },
        ))
    };
    let progress = row()
        .key("new-project-progress")
        .w_full()
        .items_center()
        .gap(6.0)
        .px(16.0)
        .pb(12.0)
        .child(bar("new-project-bar-1", step >= 1))
        .child(bar("new-project-bar-2", step >= 2))
        .child(
            text(format!("{step}/2"))
                .key("new-project-step-count")
                .ml(4.0)
                .font(crate::motion::tabular(dialog_style(9.0, 400.0)))
                .color(ink("#52525b")),
        );

    let name = rune::text_input(&d.name, dialog_style(12.5, 400.0))
        .key("new-project-name")
        .single_line()
        .w_full()
        .radius(8.0)
        .px(12.0)
        .py(8.0)
        .fill(ink("#00000066"))
        .border(1.0, ink("#ffffff1a"))
        .border_layout()
        // `focus:border-white/25`, drawn inside the border like the CSS box.
        .focus_within_ring(1.0, -1.0, ink("#ffffff40"))
        .placeholder("Project name", ink("#52525b"))
        .caret(ink("#e4e4e7"), crate::motion::TEXT_SELECTION)
        .on_edit(|s: &mut S| &mut s.projects_mut().dialog.name)
        .on_submit(|s: &mut S, _tx: &mut rune::Tx| new_project_next(s.projects_mut()));
    let path_input = rune::text_input(&d.path, dialog_style(11.5, 400.0).mono())
        .key("new-project-path")
        .single_line()
        .w_full()
        .radius(8.0)
        .px(12.0)
        .py(8.0)
        .fill(ink("#00000066"))
        .border(1.0, ink("#ffffff1a"))
        .border_layout()
        .focus_within_ring(1.0, -1.0, ink("#ffffff40"))
        .placeholder("Or paste a folder path", ink("#52525b"))
        .caret(ink("#e4e4e7"), crate::motion::TEXT_SELECTION)
        .on_edit(|s: &mut S| &mut s.projects_mut().dialog.path)
        .on_submit(|s: &mut S, _tx: &mut rune::Tx| new_project_next(s.projects_mut()));
    let browse_border = d
        .hover
        .border("new-project-browse-border", ink("#ffffff1a"));
    let set_browse_border = browse_border.clone();
    let browse = row()
        .key("new-project-browse")
        .w_full()
        .items_center()
        .gap(8.0)
        .px(12.0)
        .py(8.0)
        .radius(8.0)
        .fill(ink("#ffffff08"))
        .border(1.0, ink("#ffffff1a"))
        .border_layout()
        .live_border(browse_border.smooth(crate::motion::HOVER_SPRING))
        .role("button")
        .focusable(true)
        .label("Choose a folder")
        .help("Choose a folder")
        .hover_group()
        .on_hover_change(move |_s: &mut S, inside, tx| {
            set_browse_border.set(if inside {
                ink("#ffffff40")
            } else {
                ink("#ffffff1a")
            });
            tx.live_only();
        })
        .on_press_tx_enter(|s: &mut S, tx: &mut rune::Tx| pick_project_folder::<S>(s, tx))
        .child(
            rune::el()
                .key("new-project-browse-icon")
                .size(13.0, 13.0)
                .inert()
                .group_color(ink("#e4e4e7"))
                .shape(crate::icons::folder_open(13.0, ink("#a1a1aa"))),
        )
        .child(
            text(if path_empty {
                "Choose folder…".to_string()
            } else {
                d.path.text.clone()
            })
            .key("new-project-browse-label")
            .grow(1.0)
            .min_w(0.0)
            .truncate()
            .font(dialog_style(12.0, 400.0))
            .color(ink("#a1a1aa"))
            .group_color(ink("#e4e4e7")),
        );

    let cancel = row()
        .key("new-project-cancel")
        .role("button")
        .focusable(!busy)
        .label("Cancel")
        .px(12.0)
        .py(6.0)
        .radius(8.0)
        .hover(ink("#ffffff0d"))
        .hover_group()
        .on_press_enter(|s: &mut S| close_new_project(s.projects_mut()))
        .child(
            text("Cancel".to_string())
                .key("new-project-cancel-label")
                .font(dialog_style(12.0, 400.0))
                .color(ink("#71717a"))
                .group_color(ink("#e4e4e7")),
        );
    let mut next = row()
        .key("new-project-next")
        .role("button")
        .focusable(!busy && !path_empty)
        .label("Next")
        .items_center()
        .gap(6.0)
        .px(14.0)
        .py(6.0)
        .radius(8.0)
        .fill(ink("#ffffff"))
        .hover(ink("#e4e4e7"))
        .child(
            text("Next".to_string())
                .key("new-project-next-label")
                .font(dialog_style(12.0, 600.0))
                .color(ink("#0a0a0d")),
        )
        .child(
            rune::el()
                .key("new-project-next-icon")
                .size(12.0, 12.0)
                .inert()
                .shape(crate::icons::arrow_right(12.0, ink("#0a0a0d"))),
        );
    if busy || path_empty {
        // The reference UI's disabled Next takes no press; the inputs' Enter still calls
        // `next()` and reports the missing folder.
        next = next.opacity(0.4);
    } else {
        next = next.on_press_enter(|s: &mut S| new_project_next(s.projects_mut()));
    }

    let mut step_one = col()
        .key("new-project-step-1")
        .w_full()
        .gap(10.0)
        .px(16.0)
        .pb(16.0)
        .transition(crate::motion::project_step_enter())
        .child(name)
        .child(browse)
        .child(path_input);
    if let Some(error) = &d.error {
        step_one = step_one.child(
            text(error.clone())
                .key("new-project-error")
                .role("alert")
                .live_region(rune::LiveRegion::Assertive)
                .wrap()
                .font(dialog_style(11.0, 400.0))
                .color(ink("#f87171e6")),
        );
    }
    step_one = step_one.child(
        row()
            .key("new-project-step-1-actions")
            .w_full()
            .items_center()
            .justify_end()
            .gap(8.0)
            .pt(2.0)
            .child(if busy { cancel.opacity(0.4) } else { cancel })
            .child(next),
    );

    let mode_button = |mode: NewProjectMode, key: &str| {
        let selected = d.mode == mode;
        let group_mode = mode == NewProjectMode::Group;
        let title = if group_mode {
            "Group project"
        } else {
            "Solo chat"
        };
        let desc = if group_mode {
            "Team board, agents, approvals"
        } else {
            "One chat, prompt ready"
        };
        let glyph = if group_mode {
            crate::icons::users(13.0, ink("#a1a1aa"))
        } else {
            crate::icons::sparkles(13.0, ink("#a1a1aa"))
        };
        let rest = if selected {
            ink("#ffffff40")
        } else {
            ink("#ffffff14")
        };
        let mut card = row()
            .key(key)
            .grow(1.0)
            .min_w(0.0)
            .items_start()
            .gap(8.0)
            .px(10.0)
            .py(8.0)
            .radius(8.0)
            .border(1.0, rest)
            .border_layout()
            .fill(if selected {
                ink("#ffffff13")
            } else {
                ink("#ffffff05")
            })
            .role("radio")
            .focusable(!busy)
            .focus_selected(selected)
            .label(title)
            .on_press_enter(move |s: &mut S| new_project_mode(s.projects_mut(), mode))
            .child(
                rune::el()
                    .key(format!("{key}-icon"))
                    .size(13.0, 13.0)
                    .mt(2.0)
                    .inert()
                    .shape(glyph),
            )
            .child(
                col()
                    .key(format!("{key}-copy"))
                    .grow(1.0)
                    .min_w(0.0)
                    .gap(0.0)
                    .child(
                        text(title.to_string())
                            .key(format!("{key}-title"))
                            .font(dialog_style(11.0, 600.0))
                            .color(ink("#e4e4e7")),
                    )
                    .child(
                        text(desc.to_string())
                            .key(format!("{key}-desc"))
                            .font(dialog_snug(9.0, 400.0))
                            .color(ink("#52525b")),
                    ),
            );
        if !selected {
            // `hover:border-white/15`; the selected card keeps its .25 edge.
            let live = d.hover.border(format!("{key}-border"), ink("#ffffff14"));
            let set = live.clone();
            card = card
                .live_border(live.smooth(crate::motion::HOVER_SPRING))
                .on_hover_change(move |_s: &mut S, inside, tx| {
                    set.set(if inside {
                        ink("#ffffff26")
                    } else {
                        ink("#ffffff14")
                    });
                    tx.live_only();
                });
        }
        card
    };
    let modes = row()
        .key("new-project-mode")
        .w_full()
        .gap(6.0)
        .role("radiogroup")
        .label("Project mode")
        .focus_group()
        .child(mode_button(NewProjectMode::Group, "new-project-mode-group"))
        .child(mode_button(NewProjectMode::Solo, "new-project-mode-solo"));

    let summary = col()
        .key("new-project-summary")
        .w_full()
        .gap(0.0)
        .radius(8.0)
        .border(1.0, ink("#ffffff12"))
        .border_layout()
        .fill(ink("#ffffff06"))
        .px(12.0)
        .py(8.0)
        .child(
            text(if d.name.text.trim().is_empty() {
                "Untitled project".to_string()
            } else {
                d.name.text.trim().to_string()
            })
            .key("new-project-summary-name")
            .truncate()
            .font(dialog_style(12.0, 500.0))
            .color(ink("#e4e4e7")),
        )
        .child(
            text(path.clone())
                .key("new-project-summary-path")
                .truncate()
                .font(dialog_style(9.5, 400.0).mono())
                .color(ink("#52525b")),
        );

    let mut task_style = dialog_style(12.0, 400.0);
    task_style.line_height = 19.5;
    let task = rune::text_input(&d.task, task_style)
        .key("new-project-task")
        .w_full()
        .h(119.5)
        .radius(8.0)
        .px(12.0)
        .py(10.0)
        .fill(ink("#00000066"))
        .border(1.0, ink("#ffffff1a"))
        .border_layout()
        .focus_within_ring(1.0, -1.0, ink("#ffffff40"))
        .placeholder(
            if group {
                "What should the team build?"
            } else {
                "What should Stark work on? (optional)"
            },
            ink("#52525b"),
        )
        .caret(ink("#e4e4e7"), crate::motion::TEXT_SELECTION)
        .on_edit(|s: &mut S| &mut s.projects_mut().dialog.task);
    let info = text(
        if group {
            "The board is created in this folder. The team plans milestones, runs them, and pings you for approvals."
        } else {
            "Opens the workspace with a normal chat and this prompt already in the dock."
        }
        .to_string(),
    )
    .key("new-project-info")
    .w_full()
    .wrap()
    .radius(8.0)
    .border(1.0, ink("#ffffff0f"))
    .border_layout()
    .fill(ink("#ffffff05"))
    .px(10.0)
    .py(8.0)
    .font(dialog_snug(9.5, 400.0))
    .color(ink("#52525b"));

    let mut back = row()
        .key("new-project-back")
        .role("button")
        .focusable(!busy)
        .label("Back")
        .items_center()
        .gap(4.0)
        .px(10.0)
        .py(6.0)
        .radius(8.0)
        .hover(ink("#ffffff0d"))
        .hover_group()
        .on_press_enter(|s: &mut S| new_project_back(s.projects_mut()))
        .child(
            rune::el()
                .key("new-project-back-icon")
                .size(12.0, 12.0)
                .inert()
                .group_color(ink("#e4e4e7"))
                .shape(crate::icons::arrow_left(12.0, ink("#71717a"))),
        )
        .child(
            text("Back".to_string())
                .key("new-project-back-label")
                .font(dialog_style(11.5, 400.0))
                .color(ink("#71717a"))
                .group_color(ink("#e4e4e7")),
        );
    if busy {
        back = back.opacity(0.4);
    }
    let create_label = if group {
        "Create project"
    } else {
        "Create chat"
    };
    let disabled = busy || (group && task_empty);
    let mut create_icon = rune::el()
        .key("new-project-create-icon")
        .size(12.0, 12.0)
        .inert()
        .shape(if busy {
            crate::icons::loader(12.0, ink("#0a0a0d"))
        } else {
            crate::icons::plus(12.0, ink("#0a0a0d"))
        });
    if busy {
        // The reference UI's `Loader2 animate-spin`: one linear turn per second.
        create_icon = create_icon.looping(rune::LoopSpec::rotation(1.0));
    }
    let mut create = row()
        .key("new-project-create")
        .role("button")
        .focusable(!disabled)
        .label(create_label)
        .items_center()
        .gap(6.0)
        .px(14.0)
        .py(6.0)
        .radius(8.0)
        .fill(ink("#ffffff"))
        .hover(ink("#e4e4e7"))
        .child(create_icon)
        .child(
            text(create_label.to_string())
                .key("new-project-create-label")
                .font(dialog_style(12.0, 600.0))
                .color(ink("#0a0a0d")),
        );
    if disabled {
        create = create.opacity(0.4);
    } else {
        create = create.on_press_enter(|s: &mut S| create_new_project(s.projects_mut()));
    }

    let mut step_two = col()
        .key("new-project-step-2")
        .w_full()
        .gap(10.0)
        .px(16.0)
        .pb(16.0)
        .transition(crate::motion::project_step_enter())
        .child(summary)
        .child(modes)
        .child(task)
        .child(info);
    if let Some(error) = &d.error {
        step_two = step_two.child(
            text(error.clone())
                .key("new-project-error")
                .role("alert")
                .live_region(rune::LiveRegion::Assertive)
                .wrap()
                .font(dialog_style(11.0, 400.0))
                .color(ink("#f87171e6")),
        );
    }
    step_two = step_two.child(
        row()
            .key("new-project-step-2-actions")
            .w_full()
            .items_center()
            .justify_between()
            .gap(8.0)
            .pt(2.0)
            .child(back)
            .child(create),
    );

    let card = col()
        .key("new-project-card")
        .w(panel_w)
        .radius(16.0)
        .fill(ink("#141419f2"))
        .border(1.0, ink("#ffffff14"))
        .border_layout()
        .clip_rounded(16.0)
        .backdrop_blur(24.0)
        .box_shadow(rune::Shadow {
            x: 0.0,
            y: 25.0,
            blur: 50.0,
            spread: -12.0,
            color: rgba(0.0, 0.0, 0.0, 0.25),
            inset: false,
        })
        .transition(crate::motion::session_flow_enter())
        .hover_motion_within(crate::motion::HOVER, crate::motion::HOVER)
        .role("dialog")
        .label("New project")
        // The card is the modal root: the backdrop sits outside it, so a
        // backdrop press dismisses (the reference UI's `onMouseDown` check) while a press
        // on the card's own padding stays inside.
        .modal(true, |s: &mut S| close_new_project(s.projects_mut()))
        .child(head)
        .child(progress)
        .child(if step == 1 { step_one } else { step_two });

    rune::el()
        .key("new-project-backdrop")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(rgba(0.0, 0.0, 0.0, 0.60))
        .backdrop_blur(4.0)
        .center()
        .px(16.0)
        .py(16.0)
        .transition(crate::motion::backdrop_enter())
        .child(card)
}

/// One project card. `prefix` namespaces keys (`projects` board vs
/// `side-projects` section); `compact` tightens padding/type for the rail.
fn project_card<S: ProjectsHost + 'static>(prefix: &str, p: &GroupProject, compact: bool) -> El {
    let (pad_x, pad_y, title_size) = if compact {
        (11.0, 10.0, 11.5)
    } else {
        (14.0, 12.0, 13.0)
    };
    let open_id = p.id.clone();
    let status = p.status.to_ascii_lowercase();
    let dot = match status.as_str() {
        "running" | "doing" | "active" => "#6ee7b7",
        "done" | "complete" | "completed" => "#6ee7b7",
        "failed" | "error" => "#f87171",
        _ => "#52525b",
    };
    let waiting = p.waiting > 0;
    let mut card = col()
        .key(format!("{prefix}-{id}", id = p.id))
        .w_full()
        .gap(7.0)
        .radius(11.0)
        .fill(ink(if waiting { "#ffffff0a" } else { "#ffffff05" }))
        .border(1.0, ink(if waiting { "#ffffff24" } else { "#ffffff10" }))
        .px(pad_x)
        .py(pad_y)
        .border_layout()
        .role("button")
        .focusable(true)
        .label(format!("Open project {}", p.title))
        .focus_ring(2.0, 2.0, ink("#ffffff8c"))
        .hover(ink("#ffffff0f"))
        .on_press_enter(move |s: &mut S| {
            let projects = s.projects_mut();
            let (host, port) = (projects.host.clone(), projects.port);
            projects.group.open(host, port, open_id.clone());
        })
        .child(
            row()
                .key(format!("{prefix}-{id}-head", id = p.id))
                .w_full()
                .items_center()
                .gap(7.0)
                .child(rune::el().size(6.0, 6.0).radius(3.0).fill(ink(dot)))
                .child(
                    text(p.title.clone())
                        .key(format!("{prefix}-{id}-title", id = p.id))
                        .grow(1.0)
                        .min_w(0.0)
                        .truncate()
                        .font(rune::tok::TextStyle::new(title_size, 600.0))
                        .color(ink("#e4e4e7")),
                )
                .child(
                    text(p.status.clone())
                        .key(format!("{prefix}-{id}-status", id = p.id))
                        .font(rune::tok::TextStyle::new(9.0, 600.0))
                        .color(ink("#71717a")),
                ),
        )
        .child(progress_bar(
            &format!("{prefix}-{id}", id = p.id),
            p.fraction(),
            dot,
        ))
        .child(
            row()
                .w_full()
                .items_center()
                .gap(6.0)
                .child(
                    text(if p.total == 0 {
                        "not started".to_string()
                    } else {
                        format!("{} of {} items", p.done, p.total)
                    })
                    .key(format!("{prefix}-{id}-counts", id = p.id))
                    .grow(1.0)
                    .font(rune::tok::TextStyle::new(9.5, 400.0))
                    .color(ink("#71717a")),
                )
                .child(
                    text(format!("{}%", (p.fraction() * 100.0).round() as i64))
                        .font(rune::tok::TextStyle::new(9.5, 600.0).mono())
                        .color(ink("#a1a1aa")),
                ),
        );
    if p.waiting > 0 {
        card = card.child(
            text(format!("{} waiting", p.waiting))
                .key(format!("{prefix}-{id}-waiting", id = p.id))
                .font(rune::tok::TextStyle::new(9.0, 600.0))
                .px(6.0)
                .py(2.0)
                .radius(5.0)
                .fill(ink("#ffffff10"))
                .color(ink("#e4e4e7")),
        );
    }
    card
}

fn p_waiting_note(s: &ProjectsState) -> bool {
    s.projects.iter().any(|p| p.waiting > 0)
}

/// 4pt progress track with a fractional fill. The fill width is a fraction of
/// The track: implemented as a row where the fill grows and a spacer takes
/// The rest, so no absolute measuring is needed. `stem` is the full key stem
/// (`projects-p1`, `side-projects-p1`) — the bar/fill/rest keys hang off it.
fn progress_bar(stem: &str, fraction: f32, color: &'static str) -> El {
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
                .fill(ink(color))
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
