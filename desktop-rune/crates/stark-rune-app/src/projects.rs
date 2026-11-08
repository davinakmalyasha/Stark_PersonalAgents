//! Group-projects board — port of `GroupProjectModal` list half.
//!
//! Live contract (verified against `stark-server`):
//! `GET /api/groups` → `{projects:[{id,title,status,spent_usd,
//! milestones_done,milestones_total,items_done,items_total,
//! awaiting_approval,goal,…}], scoped}`.
//! Read-only: detail/ops (`pause`/`resume`/`close`) stay server-side until
//! the modal port. Empty board is the honest state when none exist.

use rune::{El, col, row, text};

use crate::rooms::{HouseHost, MemoryHost};

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

/// One board row. Progress prefers item counts, falls back to milestones.
#[derive(Debug, Clone, PartialEq)]
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
            .or_else(|| {
                v.get("goal")
                    .and_then(|x| x.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let (mut done, mut total) = (u64_of("items_done"), u64_of("items_total"));
        if total == 0 {
            done = u64_of("milestones_done");
            total = u64_of("milestones_total");
        }
        Self {
            id: v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string(),
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
}

impl ProjectsSnapshot {
    pub fn from_json(v: &serde_json::Value) -> Self {
        Self {
            projects: v
                .get("projects")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().map(GroupProject::from_json).collect())
                .unwrap_or_default(),
        }
    }

    pub fn fetch(host: &str, port: u16) -> Result<Self, String> {
        let v = crate::transport::get_json(host, port, "/api/groups")?;
        Ok(Self::from_json(&v))
    }
}

pub struct ProjectsState {
    pub host: String,
    pub port: u16,
    pub projects: Vec<GroupProject>,
    pub loading: bool,
    pub error: Option<String>,
    pub last_ok: f64,
    pub auto: bool,
    pub inbox: Option<std::sync::mpsc::Receiver<Result<ProjectsSnapshot, String>>>,
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
            projects: self.projects.clone(),
            loading: self.loading,
            error: self.error.clone(),
            last_ok: self.last_ok,
            auto: self.auto,
            inbox: None,
        }
    }
}

impl Default for ProjectsState {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8124,
            projects: Vec::new(),
            loading: false,
            error: None,
            last_ok: 0.0,
            auto: true,
            inbox: None,
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
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let result = match crate::transport::fetch_json("/api/groups").await {
            Ok(v) => Ok(ProjectsSnapshot::from_json(&v)),
            Err(e) => Err(e),
        };
        let _ = tx.send(result);
    });
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (host, port) = (s.host.clone(), s.port);
        std::thread::spawn(move || {
            let _ = tx.send(ProjectsSnapshot::fetch(&host, port));
        });
    }
}

/// Projects board: title + refresh, then one card per project with a
/// progress bar, status chip, and waiting badge.
pub fn projects_view<S: ProjectsHost + 'static>(s: &ProjectsState) -> El {
    let mut body = col()
        .key("projects-board")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(ink("#0a0a0a"))
        .px(24.0)
        .pt(18.0)
        .gap(10.0)
        .child(
            row()
                .key("projects-head")
                .w_full()
                .items_center()
                .child(
                    text("Projects".to_string())
                        .key("projects-title")
                        .grow(1.0)
                        .font(rune::tok::TextStyle::new(15.0, 650.0))
                        .color(ink("#f2f2f5")),
                )
                .child(
                    text(if s.loading { "…" } else { "Refresh" }.to_string())
                        .key("projects-refresh")
                        .role("button")
                        .label("Refresh projects")
                        .font(rune::tok::TextStyle::new(11.0, 600.0))
                        .px(12.0)
                        .py(8.0)
                        .radius(8.0)
                        .fill(ink("#ffffff0d"))
                        .color(ink("#d4d4d8"))
                        .on_press(|s: &mut S| {
                            refresh_projects(s.projects_mut());
                        }),
                ),
        );
    if s.projects.is_empty() && !s.loading {
        body = body.child(
            text("no projects yet — start one from a workspace".to_string())
                .key("projects-empty")
                .font(rune::tok::TextStyle::new(12.0, 400.0))
                .color(ink("#52525b")),
        );
    }
    for p in &s.projects {
        body = body.child(
            col()
                .key(format!("projects-{id}", id = p.id))
                .w_full()
                .gap(8.0)
                .radius(12.0)
                .fill(ink("#141417"))
                .border(1.0, ink("#ffffff14"))
                .px(14.0)
                .py(12.0)
                .child(
                    row()
                        .key(format!("projects-{id}-head", id = p.id))
                        .w_full()
                        .items_center()
                        .gap(8.0)
                        .child(
                            text(p.title.clone())
                                .key(format!("projects-{id}-title", id = p.id))
                                .grow(1.0)
                                .font(rune::tok::TextStyle::new(13.0, 600.0))
                                .color(ink("#e4e4e7")),
                        )
                        .child(
                            text(p.status.clone())
                                .key(format!("projects-{id}-status", id = p.id))
                                .font(rune::tok::TextStyle::new(10.0, 600.0))
                                .px(8.0)
                                .py(3.0)
                                .radius(6.0)
                                .fill(ink("#ffffff0d"))
                                .color(ink("#a1a1aa")),
                        ),
                )
                .child(progress_bar(&p.id, p.fraction()))
                .child(
                    text(format!("{} of {} items", p.done, p.total))
                        .key(format!("projects-{id}-counts", id = p.id))
                        .font(rune::tok::TextStyle::new(10.0, 400.0))
                        .color(ink("#71717a")),
                ),
        );
    }
    if p_waiting_note(s) {
        body = body.child(
            text("some projects are waiting on you".to_string())
                .key("projects-waiting")
                .font(rune::tok::TextStyle::new(11.0, 600.0))
                .color(ink("#fcd34d")),
        );
    }
    if let Some(err) = &s.error {
        body = body.child(
            text(err.clone())
                .key("projects-error")
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .color(ink("#fda4af")),
        );
    }
    body
}

fn p_waiting_note(s: &ProjectsState) -> bool {
    s.projects.iter().any(|p| p.waiting > 0)
}

/// 4pt progress track with a fractional fill. The fill width is a fraction of
/// the track: implemented as a row where the fill grows and a spacer takes
/// the rest, so no absolute measuring is needed.
fn progress_bar(id: &str, fraction: f32) -> El {
    let fill = (fraction.clamp(0.0, 1.0) * 1000.0).round() / 10.0;
    row()
        .key(format!("projects-{id}-bar"))
        .w_full()
        .h(4.0)
        .radius(2.0)
        .fill(ink("#ffffff12"))
        .child(
            rune::el()
                .key(format!("projects-{id}-fill"))
                .h_full()
                .radius(2.0)
                .fill(ink("#d4d4d8"))
                // Fractional width via flex weight out of 1000.
                .grow(fill.max(1.0))
                .min_h(0.0),
        )
        .child(rune::el().key(format!("projects-{id}-rest")).grow((1000.0 - fill).max(1.0)))
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
