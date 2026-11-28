//! Group-project settings sheet: general details, team slots, automation,
//! schedule/repeat, and destructive actions. Team changes are immediate; the
//! General/Automation tabs save in one PATCH-shaped update.

use crate::keys::EnterPress;
use chrono::TimeZone;
use rune::{col, row, text, El, TextEdit};

use super::data::GroupDetail;
use super::GroupState;
use crate::projects::ProjectsHost;

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

fn style(size: f32, weight: f32) -> rune::tok::TextStyle {
    let mut style = rune::tok::TextStyle::new(size, weight);
    style.line_height = size * 1.45;
    style
}

fn button(label: &str, primary: bool) -> El {
    let fg = if primary {
        ink("#0a0a0d")
    } else {
        ink("#d4d4d8")
    };
    let mut body = row()
        .items_center()
        .justify_center()
        .gap(5.0)
        .h(28.0)
        .px(11.0)
        .radius(8.0)
        .role("button")
        .label(label.to_string())
        .focusable(true)
        .focus_ring(2.0, 2.0, ink("#ffffff8c"))
        .child(text(label.to_string()).font(style(11.0, 600.0)).color(fg));
    if primary {
        body = body.fill(ink("#ffffff"));
    } else {
        body = body
            .fill(ink("#ffffff09"))
            .border(1.0, ink("#ffffff14"))
            .border_layout();
    }
    body
}

fn section(title: &str, hint: &str) -> El {
    col()
        .w_full()
        .gap(3.0)
        .child(
            text(title.to_string())
                .font(style(10.0, 700.0))
                .color(ink("#a1a1aa")),
        )
        .child(
            text(hint.to_string())
                .font(style(9.5, 400.0))
                .color(ink("#52525b")),
        )
}

fn field(label: &str, input: El) -> El {
    col()
        .w_full()
        .gap(5.0)
        .child(
            text(label.to_string())
                .font(style(10.0, 600.0))
                .color(ink("#a1a1aa")),
        )
        .child(input)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
    #[default]
    General,
    Team,
    Automation,
}

pub struct GroupSettingsState {
    pub open: bool,
    pub tab: SettingsTab,
    pub busy: bool,
    pub error: Option<String>,
    pub title: TextEdit,
    pub goal: TextEdit,
    pub workspace: TextEdit,
    pub planner: TextEdit,
    pub start_mode: String,
    pub start_date: TextEdit,
    pub start_time: TextEdit,
    pub recurrence: String,
    pub repeat_active: bool,
    pub max_parallel: i64,
    pub plan_review: bool,
    pub custom_agent: TextEdit,
    pub confirm_trash: bool,
}

impl std::fmt::Debug for GroupSettingsState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GroupSettingsState")
            .field("open", &self.open)
            .field("tab", &self.tab)
            .finish()
    }
}

impl Clone for GroupSettingsState {
    fn clone(&self) -> Self {
        Self {
            open: self.open,
            tab: self.tab,
            busy: self.busy,
            error: self.error.clone(),
            title: self.title.clone(),
            goal: self.goal.clone(),
            workspace: self.workspace.clone(),
            planner: self.planner.clone(),
            start_mode: self.start_mode.clone(),
            start_date: self.start_date.clone(),
            start_time: self.start_time.clone(),
            recurrence: self.recurrence.clone(),
            repeat_active: self.repeat_active,
            max_parallel: self.max_parallel,
            plan_review: self.plan_review,
            custom_agent: self.custom_agent.clone(),
            confirm_trash: self.confirm_trash,
        }
    }
}

impl Default for GroupSettingsState {
    fn default() -> Self {
        Self {
            open: false,
            tab: SettingsTab::General,
            busy: false,
            error: None,
            title: TextEdit::new(""),
            goal: TextEdit::new(""),
            workspace: TextEdit::new(""),
            planner: TextEdit::new("orchestrator"),
            start_mode: "now".to_string(),
            start_date: TextEdit::new(""),
            start_time: TextEdit::new("09:00"),
            recurrence: "none".to_string(),
            repeat_active: false,
            max_parallel: 1,
            plan_review: true,
            custom_agent: TextEdit::new(""),
            confirm_trash: false,
        }
    }
}

pub fn open(g: &mut GroupState) {
    let Some(detail) = g.detail.as_ref() else {
        return;
    };
    let settings = &mut g.settings;
    settings.open = true;
    settings.tab = SettingsTab::General;
    settings.error = None;
    settings.busy = false;
    settings.confirm_trash = false;
    settings.title.text = detail.project.title.clone();
    settings.goal.text = detail.project.goal.clone();
    settings.workspace.text = detail.workspace_path.clone();
    settings.planner.text = detail.project.planner_agent.clone();
    settings.start_mode = detail.project.schedule.start_mode.clone();
    if settings.start_mode.is_empty() {
        settings.start_mode = "now".to_string();
    }
    settings.recurrence = detail.project.schedule.recurrence.clone();
    if settings.recurrence.is_empty() {
        settings.recurrence = "none".to_string();
    }
    settings.repeat_active = detail.project.schedule.repeat_active;
    settings.max_parallel = detail.project.max_parallel.clamp(1, 8);
    settings.plan_review = detail.project.plan_review;
    if let Some(at) = detail.project.schedule.start_at {
        if let Some(local) = chrono::Local.timestamp_opt(at as i64, 0).single() {
            settings.start_date.text = local.format("%Y-%m-%d").to_string();
            settings.start_time.text = local.format("%H:%M").to_string();
        }
    } else {
        settings.start_date.text.clear();
    }
    settings.custom_agent.text.clear();
}

fn start_at(settings: &GroupSettingsState) -> Result<Option<f64>, String> {
    if settings.start_mode != "scheduled" {
        return Ok(None);
    }
    let date = settings.start_date.text.trim();
    if date.is_empty() {
        return Err("Choose a start date.".to_string());
    }
    let time = settings.start_time.text.trim();
    let combined = format!("{date} {time}");
    let parsed = chrono::NaiveDateTime::parse_from_str(&combined, "%Y-%m-%d %H:%M")
        .map_err(|_| "Use date YYYY-MM-DD and time HH:MM.".to_string())?;
    let local = chrono::Local
        .from_local_datetime(&parsed)
        .single()
        .ok_or_else(|| "That local time is ambiguous or invalid.".to_string())?;
    Ok(Some(local.timestamp() as f64))
}

pub fn save(g: &mut GroupState) {
    let settings = &g.settings;
    if settings.title.text.trim().is_empty() {
        g.settings.error = Some("Project title is required.".to_string());
        return;
    }
    let at = match start_at(settings) {
        Ok(value) => value,
        Err(error) => {
            g.settings.error = Some(error);
            return;
        }
    };
    let workspace = settings.workspace.text.trim().to_string();
    if !workspace.is_empty() && !std::path::Path::new(&workspace).is_dir() {
        g.settings.error = Some("That work folder does not exist.".to_string());
        return;
    }
    let payload = serde_json::json!({
        "title": settings.title.text.trim(),
        "goal": settings.goal.text.trim(),
        "workspace_path": workspace,
        "planner_agent": settings.planner.text.trim(),
        "start_mode": settings.start_mode,
        "start_at": at,
        "recurrence": settings.recurrence,
        "repeat_active": settings.repeat_active,
        "max_parallel": settings.max_parallel.clamp(1, 8),
        "plan_review": settings.plan_review,
    });
    g.settings.error = None;
    g.op_with("set_settings", payload);
}

pub fn add_agent(g: &mut GroupState, agent_key: &str) {
    let key = agent_key.trim().to_ascii_lowercase();
    if key.is_empty() {
        return;
    }
    g.op_with(
        "add_member",
        serde_json::json!({"agent_key": key, "amount": 1}),
    );
}

pub fn add_custom(g: &mut GroupState) {
    let key = g.settings.custom_agent.text.trim().to_string();
    if key.is_empty() {
        return;
    }
    add_agent(g, &key);
    g.settings.custom_agent.text.clear();
}

pub fn remove_agent(g: &mut GroupState, agent_key: &str) {
    let Some(detail) = g.detail.as_ref() else {
        return;
    };
    let member = detail
        .members
        .iter()
        .filter(|member| member.agent_key.eq_ignore_ascii_case(agent_key))
        .max_by_key(|member| member.slot_index);
    if let Some(member) = member {
        let id = member.id.clone();
        g.op_with("remove_member", serde_json::json!({"member_id": id}));
    }
}

pub fn remove_member(g: &mut GroupState, member_id: &str) {
    g.op_with("remove_member", serde_json::json!({"member_id": member_id}));
}

fn builtin_agents() -> &'static [(&'static str, &'static str)] {
    &[
        ("developer", "Developer"),
        ("general", "General"),
        ("ui", "UI"),
        ("devops", "DevOps"),
        ("explore", "Explore"),
        ("house", "House"),
        ("dfd", "DFD"),
    ]
}

fn tab_button<S: ProjectsHost + 'static>(label: &str, active: bool, tab: SettingsTab) -> El {
    text(label.to_string())
        .key(format!("gp-settings-tab-{label}"))
        .grow(1.0)
        .h(28.0)
        .center()
        .radius(7.0)
        .font(style(10.5, 600.0))
        .color(if active {
            ink("#f4f4f5")
        } else {
            ink("#71717a")
        })
        .fill(if active {
            ink("#ffffff10")
        } else {
            ink("#00000000")
        })
        .role("tab")
        .label(label)
        .focusable(true)
        .focus_selected(active)
        .on_press_enter(move |s: &mut S| {
            s.projects_mut().group.settings.tab = tab;
        })
}

fn toggle<S: ProjectsHost + 'static>(
    key: &str,
    label: &str,
    value: bool,
    on_toggle: impl Fn(&mut S) + 'static,
) -> El {
    row()
        .key(key.to_string())
        .w_full()
        .items_center()
        .justify_between()
        .gap(10.0)
        .px(10.0)
        .py(7.0)
        .radius(8.0)
        .fill(ink("#ffffff06"))
        .child(
            text(label.to_string())
                .font(style(10.5, 500.0))
                .color(ink("#d4d4d8")),
        )
        .child(
            text(if value { "On" } else { "Off" }.to_string())
                .font(style(9.5, 700.0))
                .px(8.0)
                .py(2.0)
                .radius(6.0)
                .fill(if value {
                    ink("#ffffff18")
                } else {
                    ink("#ffffff08")
                })
                .color(if value {
                    ink("#e4e4e7")
                } else {
                    ink("#71717a")
                }),
        )
        .role("switch")
        .checked(value)
        .focusable(true)
        .on_press_enter(on_toggle)
}

fn team_tab<S: ProjectsHost + 'static>(g: &GroupState, detail: &GroupDetail) -> El {
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for member in &detail.members {
        *counts.entry(member.agent_key.clone()).or_insert(0) += 1;
    }
    let mut body = col().key("gp-settings-team").w_full().gap(10.0);
    body = body.child(section(
        "Agent slots",
        "Each slot is one independent worker. Increase a count to add capacity.",
    ));
    for (key, label) in builtin_agents() {
        let count = counts.get(*key).copied().unwrap_or(0);
        let key_owned = key.to_string();
        let key_remove = key.to_string();
        body = body.child(
            row()
                .w_full()
                .items_center()
                .gap(8.0)
                .px(10.0)
                .py(7.0)
                .radius(8.0)
                .fill(ink("#ffffff05"))
                .child(
                    col()
                        .grow(1.0)
                        .gap(1.0)
                        .child(
                            text(label.to_string())
                                .font(style(10.5, 600.0))
                                .color(ink("#e4e4e7")),
                        )
                        .child(
                            text(format!("{key} · {count} active"))
                                .font(style(9.0, 400.0))
                                .color(ink("#71717a")),
                        ),
                )
                .child(
                    text("−".to_string())
                        .key(format!("gp-settings-minus-{key}"))
                        .size(24.0, 24.0)
                        .center()
                        .radius(6.0)
                        .fill(ink("#ffffff0a"))
                        .role("button")
                        .label(format!("Remove {label}"))
                        .focusable(true)
                        .on_press_enter(move |s: &mut S| {
                            remove_agent(&mut s.projects_mut().group, &key_remove);
                        }),
                )
                .child(
                    text(count.to_string())
                        .w(22.0)
                        .center()
                        .font(style(10.5, 700.0))
                        .color(ink("#f4f4f5")),
                )
                .child(
                    text("+".to_string())
                        .key(format!("gp-settings-plus-{key}"))
                        .size(24.0, 24.0)
                        .center()
                        .radius(6.0)
                        .fill(ink("#ffffff12"))
                        .role("button")
                        .label(format!("Add {label}"))
                        .focusable(true)
                        .on_press_enter(move |s: &mut S| {
                            add_agent(&mut s.projects_mut().group, &key_owned);
                        }),
                ),
        );
    }
    body = body.child(section(
        "Custom agent",
        "Type the key of a custom agent from the Agents panel.",
    ));
    body = body.child(
        row()
            .w_full()
            .gap(6.0)
            .child(
                rune::text_input(&g.settings.custom_agent, style(11.0, 400.0))
                    .key("gp-settings-custom-agent")
                    .single_line()
                    .grow(1.0)
                    .radius(7.0)
                    .px(9.0)
                    .py(6.0)
                    .fill(ink("#00000066"))
                    .border(1.0, ink("#ffffff14"))
                    .border_layout()
                    .placeholder("agent-key", ink("#52525b"))
                    .caret(ink("#e4e4e7"), crate::motion::TEXT_SELECTION)
                    .on_edit(|s: &mut S| &mut s.projects_mut().group.settings.custom_agent)
                    .on_submit(|s: &mut S, _tx: &mut rune::Tx| {
                        add_custom(&mut s.projects_mut().group)
                    }),
            )
            .child(
                button("Add", true)
                    .key("gp-settings-custom-add")
                    .on_press_enter(|s: &mut S| {
                        add_custom(&mut s.projects_mut().group);
                    }),
            ),
    );
    if !detail.members.is_empty() {
        body = body.child(section(
            "Current slots",
            "Remove a slot only when its current item is idle.",
        ));
        for member in &detail.members {
            let id = member.id.clone();
            body = body.child(
                row()
                    .w_full()
                    .items_center()
                    .gap(7.0)
                    .px(9.0)
                    .py(6.0)
                    .radius(7.0)
                    .fill(ink("#ffffff04"))
                    .child(
                        text(member.label())
                            .grow(1.0)
                            .font(style(10.0, 550.0))
                            .color(ink("#d4d4d8")),
                    )
                    .child(
                        text(format!("{}/{} done", member.done, member.assigned))
                            .font(style(9.0, 400.0))
                            .color(ink("#71717a")),
                    )
                    .child(
                        text("Remove".to_string())
                            .px(7.0)
                            .py(3.0)
                            .radius(5.0)
                            .font(style(9.0, 600.0))
                            .color(ink("#fda4af"))
                            .role("button")
                            .label(format!("Remove {}", member.label()))
                            .focusable(true)
                            .on_press_enter(move |s: &mut S| {
                                remove_member(&mut s.projects_mut().group, &id);
                            }),
                    ),
            );
        }
    }
    body
}

fn automation_tab<S: ProjectsHost + 'static>(g: &GroupState) -> El {
    let settings = &g.settings;
    let start_modes = row()
        .key("gp-settings-start-mode")
        .w_full()
        .gap(4.0)
        .children(["now", "scheduled", "manual"].into_iter().map(|mode| {
            let active = settings.start_mode == mode;
            let label = match mode {
                "scheduled" => "Scheduled",
                "manual" => "Manual",
                _ => "Start now",
            };
            let mode_owned = mode.to_string();
            text(label.to_string())
                .grow(1.0)
                .h(27.0)
                .center()
                .radius(6.0)
                .font(style(10.0, 600.0))
                .color(if active {
                    ink("#f4f4f5")
                } else {
                    ink("#71717a")
                })
                .fill(if active {
                    ink("#ffffff12")
                } else {
                    ink("#ffffff04")
                })
                .role("radio")
                .focusable(true)
                .focus_selected(active)
                .on_press_enter(move |s: &mut S| {
                    s.projects_mut().group.settings.start_mode = mode_owned.clone();
                })
        }));
    let date = rune::text_input(&settings.start_date, style(11.0, 400.0))
        .key("gp-settings-date")
        .single_line()
        .grow(1.0)
        .radius(7.0)
        .px(9.0)
        .py(6.0)
        .fill(ink("#00000066"))
        .border(1.0, ink("#ffffff14"))
        .border_layout()
        .placeholder("YYYY-MM-DD", ink("#52525b"))
        .caret(ink("#e4e4e7"), crate::motion::TEXT_SELECTION)
        .on_edit(|s: &mut S| &mut s.projects_mut().group.settings.start_date);
    let time = rune::text_input(&settings.start_time, style(11.0, 400.0))
        .key("gp-settings-time")
        .single_line()
        .w(92.0)
        .radius(7.0)
        .px(9.0)
        .py(6.0)
        .fill(ink("#00000066"))
        .border(1.0, ink("#ffffff14"))
        .border_layout()
        .placeholder("HH:MM", ink("#52525b"))
        .caret(ink("#e4e4e7"), crate::motion::TEXT_SELECTION)
        .on_edit(|s: &mut S| &mut s.projects_mut().group.settings.start_time);
    let repeat = ["none", "daily", "weekly", "monthly"].into_iter().fold(
        row().w_full().gap(4.0),
        |row, value| {
            let active = settings.recurrence == value;
            let label = match value {
                "daily" => "Daily",
                "weekly" => "Weekly",
                "monthly" => "Monthly",
                _ => "Never",
            };
            let value_owned = value.to_string();
            row.child(
                text(label.to_string())
                    .grow(1.0)
                    .h(26.0)
                    .center()
                    .radius(6.0)
                    .font(style(9.5, 600.0))
                    .color(if active {
                        ink("#f4f4f5")
                    } else {
                        ink("#71717a")
                    })
                    .fill(if active {
                        ink("#ffffff12")
                    } else {
                        ink("#ffffff04")
                    })
                    .role("radio")
                    .focusable(true)
                    .focus_selected(active)
                    .on_press_enter(move |s: &mut S| {
                        let st = &mut s.projects_mut().group.settings;
                        st.recurrence = value_owned.clone();
                        st.repeat_active = value_owned != "none";
                    }),
            )
        },
    );
    let max_parallel = row()
        .items_center()
        .gap(8.0)
        .child(
            text("Max parallel workers".to_string())
                .grow(1.0)
                .font(style(10.5, 500.0))
                .color(ink("#d4d4d8")),
        )
        .child(
            text("−".to_string())
                .size(24.0, 24.0)
                .center()
                .radius(6.0)
                .fill(ink("#ffffff0a"))
                .role("button")
                .focusable(true)
                .on_press_enter(|s: &mut S| {
                    let st = &mut s.projects_mut().group.settings;
                    st.max_parallel = (st.max_parallel - 1).max(1);
                }),
        )
        .child(
            text(settings.max_parallel.to_string())
                .w(22.0)
                .center()
                .font(style(10.5, 700.0))
                .color(ink("#f4f4f5")),
        )
        .child(
            text("+".to_string())
                .size(24.0, 24.0)
                .center()
                .radius(6.0)
                .fill(ink("#ffffff0a"))
                .role("button")
                .focusable(true)
                .on_press_enter(|s: &mut S| {
                    let st = &mut s.projects_mut().group.settings;
                    st.max_parallel = (st.max_parallel + 1).min(8);
                }),
        );
    let planner = rune::text_input(&settings.planner, style(11.0, 400.0))
        .key("gp-settings-planner")
        .single_line()
        .w_full()
        .radius(7.0)
        .px(9.0)
        .py(6.0)
        .fill(ink("#00000066"))
        .border(1.0, ink("#ffffff14"))
        .border_layout()
        .placeholder("orchestrator", ink("#52525b"))
        .caret(ink("#e4e4e7"), crate::motion::TEXT_SELECTION)
        .on_edit(|s: &mut S| &mut s.projects_mut().group.settings.planner);
    col()
        .w_full()
        .gap(12.0)
        .child(section("Start", "When should this project begin?"))
        .child(start_modes)
        .child(row().w_full().gap(6.0).child(date).child(time))
        .child(section(
            "Repeat",
            "A repeat starts a fresh copy of the plan and keeps old cycles visible.",
        ))
        .child(repeat)
        .child(toggle(
            "gp-settings-repeat-active",
            "Repeat active",
            settings.repeat_active,
            |s: &mut S| {
                let st = &mut s.projects_mut().group.settings;
                st.repeat_active = !st.repeat_active;
            },
        ))
        .child(section(
            "Execution",
            "Capacity and review before the team starts.",
        ))
        .child(max_parallel)
        .child(toggle(
            "gp-settings-plan-review",
            "Review plan before starting",
            settings.plan_review,
            |s: &mut S| {
                let st = &mut s.projects_mut().group.settings;
                st.plan_review = !st.plan_review;
            },
        ))
        .child(field("Planner agent", planner))
}

pub fn settings_sheet<S: ProjectsHost + 'static>(g: &GroupState, w: f32, h: f32) -> El {
    let detail = g.detail.as_ref();
    let panel_w = 560.0f32.min((w - 28.0).max(300.0));
    let panel_h = (h - 36.0).clamp(320.0, 820.0);
    let settings = &g.settings;
    let title = detail
        .map(|detail| detail.project.display_title())
        .unwrap_or_else(|| "Project settings".to_string());
    let tabs = row()
        .key("gp-settings-tabs")
        .w_full()
        .gap(3.0)
        .padding(3.0)
        .radius(8.0)
        .fill(ink("#ffffff05"))
        .child(tab_button::<S>(
            "General",
            settings.tab == SettingsTab::General,
            SettingsTab::General,
        ))
        .child(tab_button::<S>(
            "Team",
            settings.tab == SettingsTab::Team,
            SettingsTab::Team,
        ))
        .child(tab_button::<S>(
            "Automation",
            settings.tab == SettingsTab::Automation,
            SettingsTab::Automation,
        ));

    let mut body = col().w_full().gap(11.0).pb(8.0);
    match settings.tab {
        SettingsTab::General => {
            let title_input = rune::text_input(&settings.title, style(12.0, 400.0))
                .key("gp-settings-title")
                .single_line()
                .w_full()
                .radius(7.0)
                .px(10.0)
                .py(7.0)
                .fill(ink("#00000066"))
                .border(1.0, ink("#ffffff14"))
                .border_layout()
                .on_edit(|s: &mut S| &mut s.projects_mut().group.settings.title);
            let goal_input = rune::text_input(&settings.goal, style(11.5, 400.0))
                .key("gp-settings-goal")
                .w_full()
                .h(104.0)
                .radius(7.0)
                .px(10.0)
                .py(8.0)
                .fill(ink("#00000066"))
                .border(1.0, ink("#ffffff14"))
                .border_layout()
                .on_edit(|s: &mut S| &mut s.projects_mut().group.settings.goal);
            let workspace = rune::text_input(&settings.workspace, style(10.5, 400.0).mono())
                .key("gp-settings-workspace")
                .single_line()
                .w_full()
                .radius(7.0)
                .px(10.0)
                .py(7.0)
                .fill(ink("#00000066"))
                .border(1.0, ink("#ffffff14"))
                .border_layout()
                .on_edit(|s: &mut S| &mut s.projects_mut().group.settings.workspace);
            body = body
                .child(section(
                    "Identity",
                    "The title is the project name; the goal is the outcome.",
                ))
                .child(field("Title", title_input))
                .child(field("Goal", goal_input))
                .child(field("Work folder", workspace));
            if let Some(detail) = detail {
                body = body.child(
                    text(format!(
                        "{} · cycle {} · {} runs · {} tokens",
                        display_status(&detail.project.status),
                        detail.current_cycle,
                        detail.usage.runs,
                        detail.usage.tokens()
                    ))
                    .font(style(9.5, 400.0))
                    .color(ink("#71717a")),
                );
            }
        }
        SettingsTab::Team => {
            if let Some(detail) = detail {
                body = body.child(team_tab::<S>(g, detail));
            }
        }
        SettingsTab::Automation => {
            body = body.child(automation_tab::<S>(g));
        }
    }
    if let Some(error) = &settings.error {
        body = body.child(
            text(error.clone())
                .w_full()
                .wrap()
                .font(style(10.0, 400.0))
                .color(ink("#fda4af")),
        );
    }
    let mut card = col()
        .key("gp-settings")
        .w(panel_w)
        .h(panel_h)
        .radius(15.0)
        .fill(ink("#121216"))
        .border(1.0, ink("#2a2a31"))
        .border_layout()
        .clip_rounded(15.0)
        .box_shadow(rune::Shadow {
            x: 0.0,
            y: 24.0,
            blur: 60.0,
            spread: -16.0,
            color: ink("#00000066"),
            inset: false,
        })
        .role("dialog")
        .label("Project settings")
        .modal(true, |s: &mut S| {
            s.projects_mut().group.settings.open = false;
        })
        .child(
            row()
                .w_full()
                .items_center()
                .gap(8.0)
                .px(16.0)
                .pt(14.0)
                .pb(10.0)
                .child(
                    text(title)
                        .grow(1.0)
                        .truncate()
                        .font(style(14.0, 650.0))
                        .color(ink("#f4f4f5")),
                )
                .child(
                    text("Close".to_string())
                        .px(8.0)
                        .py(4.0)
                        .radius(6.0)
                        .font(style(10.0, 550.0))
                        .color(ink("#a1a1aa"))
                        .role("button")
                        .focusable(true)
                        .on_press_enter(|s: &mut S| {
                            s.projects_mut().group.settings.open = false;
                        }),
                ),
        )
        .child(
            col()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .px(16.0)
                .gap(10.0)
                .scroll(rune::ScrollSpec {
                    stick_bottom: false,
                })
                .child(tabs)
                .child(body),
        );
    // Save / danger footer
    let mut footer = row()
        .w_full()
        .items_center()
        .gap(7.0)
        .px(16.0)
        .py(10.0)
        .border_sides([1.0, 0.0, 0.0, 0.0], ink("#ffffff10"))
        .border_layout()
        .child(
            text("Move to Trash".to_string())
                .px(7.0)
                .py(4.0)
                .radius(6.0)
                .font(style(9.5, 600.0))
                .color(if settings.confirm_trash {
                    ink("#fda4af")
                } else {
                    ink("#71717a")
                })
                .role("button")
                .focusable(true)
                .on_press_enter(|s: &mut S| {
                    let g = &mut s.projects_mut().group;
                    if g.settings.confirm_trash {
                        g.settings.confirm_trash = false;
                        g.trash_project();
                    } else {
                        g.settings.confirm_trash = true;
                    }
                }),
        )
        .child(rune::el().grow(1.0));
    if settings.tab != SettingsTab::Team {
        footer = footer.child(
            button("Save changes", true)
                .key("gp-settings-save")
                .on_press_enter(|s: &mut S| {
                    let g = &mut s.projects_mut().group;
                    save(g);
                }),
        );
    }
    footer = footer.child(
        button("Close", false)
            .key("gp-settings-close")
            .on_press_enter(|s: &mut S| {
                s.projects_mut().group.settings.open = false;
            }),
    );
    card = card.child(footer);
    rune::el()
        .key("gp-settings-backdrop")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(ink("#000000aa"))
        .backdrop_blur(3.0)
        .center()
        .px(14.0)
        .py(14.0)
        .transition(crate::motion::backdrop_enter())
        .child(card)
}

fn display_status(status: &str) -> String {
    match status {
        "planning" => "Planning".to_string(),
        "plan_ready" => "Plan ready".to_string(),
        "scheduled" => "Scheduled".to_string(),
        "running" => "Running".to_string(),
        "paused" => "Paused".to_string(),
        "blocked" => "Blocked".to_string(),
        "done" => "Done".to_string(),
        "draft" => "Draft".to_string(),
        other => other.to_string(),
    }
}
