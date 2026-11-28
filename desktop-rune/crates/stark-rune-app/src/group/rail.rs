//! `GroupProjectRail`: progress, waiting approvals, board/team/details.

use crate::keys::EnterPress;
use rune::{col, row, text, El};

use super::data::{GroupDetail, GroupItem, GroupMilestone};
use super::ui::{avatar, display_actor, event_label, money, status_chip, time_ago};
use super::{GroupState, RailTab};
use crate::projects::ProjectsHost;

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

fn rgba(r: f32, g: f32, b: f32, a: f32) -> rune::Color {
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

fn card() -> El {
    col()
        .w_full()
        .radius(11.0)
        .border(1.0, rgba(255.0, 255.0, 255.0, 0.07))
        .border_layout()
        .fill(rgba(255.0, 255.0, 255.0, 0.018))
}

fn icon(size: f32, shape: rune::Shape) -> El {
    rune::el().size(size, size).inert().shape(shape)
}

fn section_title(label: &str, glyph: rune::Shape) -> El {
    row()
        .items_center()
        .gap(5.0)
        .px(12.0)
        .pt(11.0)
        .pb(8.0)
        .child(icon(12.0, glyph))
        .child(
            text(label.to_uppercase())
                .font({
                    let mut style = rune::tok::TextStyle::new(10.0, 650.0);
                    style.line_height = 15.0;
                    style.tracking = 0.8;
                    style
                })
                .color(ink("#71717a")),
        )
}

fn button(label: &str, primary: bool, height: f32, glyph: rune::Shape) -> El {
    let fg = if primary {
        ink("#0a0a0d")
    } else {
        ink("#d4d4d8")
    };
    let mut el = row()
        .items_center()
        .gap(5.0)
        .h(height)
        .px(9.0)
        .radius(7.0)
        .center()
        .child(icon(12.0, glyph))
        .child(
            text(label.to_string())
                .font(rune::tok::TextStyle::new(10.0, 550.0))
                .color(fg),
        );
    if primary {
        el = el.fill(ink("#ffffff"));
    } else {
        el = el
            .fill(rgba(255.0, 255.0, 255.0, 0.035))
            .border(1.0, rgba(255.0, 255.0, 255.0, 0.1))
            .border_layout();
    }
    el
}

fn resolve_button<S: ProjectsHost + 'static>(
    label: &str,
    item: &GroupItem,
    grant: bool,
    primary: bool,
    busy: bool,
) -> El {
    let item = item.clone();
    let fg = if primary {
        ink("#0a0a0d")
    } else {
        ink("#d4d4d8")
    };
    let glyph = if grant {
        crate::lucide_check(12.0, 2.0, fg)
    } else {
        crate::lucide_x(12.0, 2.0, fg)
    };
    let mut el = button(label, primary, 26.0, glyph)
        .role("button")
        .label(if grant {
            format!("Approve {}", item.title)
        } else {
            format!("Reject {}", item.title)
        })
        .focusable(true)
        .focus_ring(2.0, 2.0, rgba(255.0, 255.0, 255.0, 0.55));
    if !busy {
        el = el.on_press_enter(move |s: &mut S| {
            s.projects_mut().group.resolve(&item, grant);
        });
    }
    el
}

fn waiting_card<S: ProjectsHost + 'static>(g: &GroupState, detail: &GroupDetail) -> El {
    let waiting = detail.waiting();
    let mut card = card()
        .key("gp-waiting-card")
        .border(1.0, rgba(251.0, 191.0, 36.0, 0.18))
        .fill(rgba(251.0, 191.0, 36.0, 0.035))
        .pb(2.0);
    card = card.child(section_title(
        "Waiting on you",
        crate::icons::triangle_alert(12.0, ink("#71717a")),
    ));
    for item in waiting {
        let busy = g.busy_item.as_deref() == Some(item.id.as_str());
        card = card.child(
            col()
                .key(format!("gp-waiting-{}", item.id))
                .mx(10.0)
                .mb(8.0)
                .gap(2.0)
                .padding(9.0)
                .radius(8.0)
                .border(1.0, rgba(251.0, 191.0, 36.0, 0.14))
                .border_layout()
                .fill(rgba(251.0, 191.0, 36.0, 0.05))
                .child(
                    text(item.title.clone())
                        .font(rune::tok::TextStyle::new(11.5, 600.0))
                        .color(ink("#e4e4e7")),
                )
                .child(
                    text(item.id.clone())
                        .font(rune::tok::TextStyle::new(9.0, 400.0).mono())
                        .color(ink("#71717a")),
                )
                .child(
                    row()
                        .items_center()
                        .gap(5.0)
                        .mt(7.0)
                        .child(resolve_button::<S>("Approve", item, true, true, busy))
                        .child(resolve_button::<S>("Reject", item, false, false, busy)),
                ),
        );
    }
    card
}

fn progress_card(detail: &GroupDetail) -> El {
    let items_done = detail.items_done();
    let total = detail.items.len();
    let pct = if total == 0 {
        0
    } else {
        ((items_done as f32 / total as f32) * 100.0).round() as u32
    };
    let spent = detail.project.spent_usd.unwrap_or(0.0);
    let cap = detail.project.budget_usd;
    let mut card = card().key("gp-progress-card").pb(11.0).child(
        row()
            .w_full()
            .items_center()
            .justify_between()
            .gap(8.0)
            .px(12.0)
            .pt(11.0)
            .child(
                text("PROGRESS".to_string())
                    .font({
                        let mut style = rune::tok::TextStyle::new(10.0, 650.0);
                        style.line_height = 15.0;
                        style.tracking = 0.8;
                        style
                    })
                    .color(ink("#71717a")),
            )
            .child(status_chip(
                if detail.project.status.is_empty() {
                    "unknown"
                } else {
                    &detail.project.status
                },
                false,
            )),
    );
    card = card
        .child(
            text(format!("{pct}%"))
                .px(12.0)
                .pt(8.0)
                .font(crate::motion::tabular({
                    let mut style = rune::tok::TextStyle::new(27.0, 650.0);
                    style.line_height = 27.0;
                    style.tracking = -0.8;
                    style
                }))
                .color(ink("#f4f4f5")),
        )
        .child({
            let fill = (pct as f32 / 100.0).clamp(0.0, 1.0);
            row()
                .key("gp-progress-bar")
                .w_full()
                .mx(12.0)
                .mt(10.0)
                .h(4.0)
                .radius(2.0)
                .fill(rgba(255.0, 255.0, 255.0, 0.07))
                .clip_rounded(2.0)
                .child(
                    rune::el()
                        .key("gp-progress-fill")
                        .w_frac(fill)
                        .h_full()
                        .fill(ink("#d4d4d8")),
                )
        })
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .gap(10.0)
                .px(12.0)
                .pt(8.0)
                .child(
                    row()
                        .gap(3.0)
                        .child(
                            text(format!(
                                "{}/{}",
                                detail.milestones_done(),
                                detail.milestones.len()
                            ))
                            .font(crate::motion::tabular(rune::tok::TextStyle::new(
                                10.0, 600.0,
                            )))
                            .color(ink("#d4d4d8")),
                        )
                        .child(
                            text("milestones".to_string())
                                .font(rune::tok::TextStyle::new(10.0, 400.0))
                                .color(ink("#71717a")),
                        ),
                )
                .child(
                    row()
                        .gap(3.0)
                        .child(
                            text(format!("{items_done}/{total}"))
                                .font(crate::motion::tabular(rune::tok::TextStyle::new(
                                    10.0, 600.0,
                                )))
                                .color(ink("#d4d4d8")),
                        )
                        .child(
                            text("items".to_string())
                                .font(rune::tok::TextStyle::new(10.0, 400.0))
                                .color(ink("#71717a")),
                        ),
                ),
        )
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .gap(10.0)
                .mx(12.0)
                .mt(9.0)
                .pt(8.0)
                .border_sides([0.0, 1.0, 0.0, 0.0], rgba(255.0, 255.0, 255.0, 0.055))
                .border_layout()
                .child(
                    text("Spend".to_string())
                        .font(rune::tok::TextStyle::new(10.0, 400.0))
                        .color(ink("#71717a")),
                )
                .child(
                    row()
                        .gap(3.0)
                        .child(
                            text(money(spent))
                                .font(crate::motion::tabular(rune::tok::TextStyle::new(
                                    10.0, 600.0,
                                )))
                                .color(ink("#d4d4d8")),
                        )
                        .child(text(" / ".to_string()).color(ink("#52525b")))
                        .child(if let Some(cap) = cap {
                            text(money(cap))
                                .font(rune::tok::TextStyle::new(9.5, 400.0))
                                .color(ink("#52525b"))
                        } else {
                            rune::el().size(0.0, 0.0).inert()
                        }),
                ),
        );
    if let Some(deadline) = detail.project.deadline.filter(|d| *d > 0.0) {
        card = card.child(
            row()
                .items_center()
                .gap(4.0)
                .mx(12.0)
                .mt(6.0)
                .child(icon(11.0, crate::icons::clock(11.0, ink("#71717a"))))
                .child(
                    text(super::ui::deadline_label(deadline))
                        .font(rune::tok::TextStyle::new(9.5, 400.0))
                        .color(ink("#71717a")),
                ),
        );
    }
    card
}

fn tabs<S: ProjectsHost + 'static>(g: &GroupState, team: usize) -> El {
    let mut row = row()
        .key("gp-rail-tabs")
        .w_full()
        .items_center()
        .gap(2.0)
        .padding(3.0)
        .radius(8.0)
        .border(1.0, rgba(255.0, 255.0, 255.0, 0.07))
        .border_layout()
        .fill(rgba(255.0, 255.0, 255.0, 0.02))
        .role("tablist")
        .label("Project views")
        .focus_group();
    for (tab, label) in [
        (RailTab::Board, "Board".to_string()),
        (RailTab::Team, format!("Team {team}")),
        (RailTab::Activity, "Activity".to_string()),
    ] {
        let active = g.rail_tab == tab;
        row = row.child(
            text(label)
                .key(format!(
                    "gp-tab-{}",
                    match tab {
                        RailTab::Board => "board",
                        RailTab::Team => "team",
                        RailTab::Activity => "activity",
                    }
                ))
                .grow(1.0)
                .h(27.0)
                .center()
                .radius(6.0)
                .font(rune::tok::TextStyle::new(10.0, 600.0))
                .color(if active {
                    ink("#f4f4f5")
                } else {
                    ink("#71717a")
                })
                .fill(if active {
                    rgba(255.0, 255.0, 255.0, 0.08)
                } else {
                    rgba(255.0, 255.0, 255.0, 0.0)
                })
                .role("tab")
                .label(match tab {
                    RailTab::Board => "Board",
                    RailTab::Team => "Team",
                    RailTab::Activity => "Activity",
                })
                .focusable(true)
                .focus_selected(active)
                .focus_ring(2.0, 2.0, rgba(255.0, 255.0, 255.0, 0.55))
                .on_press_enter(move |s: &mut S| {
                    s.projects_mut().group.rail_tab = tab;
                }),
        );
    }
    row
}

fn status_controls<S: ProjectsHost + 'static>(milestone: &GroupMilestone) -> El {
    let mut row = row().w_full().gap(3.0).pt(2.0);
    for (status, label) in [
        ("todo", "New"),
        ("active", "Ongoing"),
        ("done", "Done"),
        ("blocked", "Blocked"),
    ] {
        let active =
            milestone.status == status || (status == "todo" && milestone.status == "pending");
        let id = milestone.id.clone();
        let next = status.to_string();
        row = row.child(
            text(label.to_string())
                .key(format!("gp-ms-{}-status-{status}", milestone.id))
                .px(6.0)
                .py(2.0)
                .radius(4.0)
                .font(rune::tok::TextStyle::new(8.5, 600.0))
                .color(if active {
                    ink("#0a0a0d")
                } else {
                    ink("#71717a")
                })
                .fill(if active {
                    ink("#e4e4e7")
                } else {
                    rgba(255.0, 255.0, 255.0, 0.05)
                })
                .role("button")
                .label(format!("Set {} {label}", milestone.title))
                .focusable(true)
                .on_press_enter(move |s: &mut S| {
                    s.projects_mut().group.op_with(
                        "update_milestone",
                        serde_json::json!({"milestone_id": id, "status": next}),
                    );
                }),
        );
    }
    row
}

fn item_action<S: ProjectsHost + 'static>(key: &str, label: &str, op: &str, item_id: &str) -> El {
    let op = op.to_string();
    let id = item_id.to_string();
    text(label.to_string())
        .key(key.to_string())
        .px(6.0)
        .py(2.0)
        .radius(4.0)
        .font(rune::tok::TextStyle::new(8.5, 600.0))
        .color(ink("#a1a1aa"))
        .fill(rgba(255.0, 255.0, 255.0, 0.045))
        .role("button")
        .focusable(true)
        .on_press_enter(move |s: &mut S| {
            let g = &mut s.projects_mut().group;
            if op == "delete_item" {
                let key = format!("delete-item-{id}");
                if g.status_busy.as_deref() == Some(key.as_str()) {
                    g.status_busy = None;
                    g.op_with("delete_item", serde_json::json!({"item_id": id}));
                } else {
                    g.status_busy = Some(key);
                }
            } else {
                g.op_with(&op, serde_json::json!({"item_id": id}));
            }
        })
}

fn add_item_row<S: ProjectsHost + 'static>(g: &GroupState, milestone_id: &str) -> El {
    let id = milestone_id.to_string();
    row()
        .w_full()
        .gap(5.0)
        .pt(2.0)
        .child(
            rune::text_input(&g.new_item, rune::tok::TextStyle::new(10.0, 400.0))
                .key(format!("gp-new-item-{milestone_id}"))
                .single_line()
                .grow(1.0)
                .radius(6.0)
                .px(7.0)
                .py(4.0)
                .fill(ink("#00000066"))
                .border(1.0, ink("#ffffff14"))
                .border_layout()
                .placeholder("New item", ink("#52525b"))
                .on_edit(|s: &mut S| &mut s.projects_mut().group.new_item)
                .on_submit(move |s: &mut S, _tx: &mut rune::Tx| {
                    add_item_from_draft(s, &id);
                }),
        )
        .child(
            text("Add".to_string())
                .px(7.0)
                .py(4.0)
                .radius(5.0)
                .fill(ink("#ffffff"))
                .font(rune::tok::TextStyle::new(9.0, 700.0))
                .color(ink("#0a0a0d"))
                .role("button")
                .focusable(true)
                .on_press_enter({
                    let id = milestone_id.to_string();
                    move |s: &mut S| add_item_from_draft(s, &id)
                }),
        )
}

fn add_item_from_draft<S: ProjectsHost + 'static>(s: &mut S, milestone_id: &str) {
    let g = &mut s.projects_mut().group;
    let title = g.new_item.text.trim().to_string();
    if title.is_empty() {
        return;
    }
    g.op_with(
        "add_item",
        serde_json::json!({
            "milestone_id": milestone_id,
            "title": title,
            "acceptance": ["The deliverable is complete and summarized"]
        }),
    );
    g.new_item.text.clear();
    g.new_item_milestone = None;
}

fn board<S: ProjectsHost + 'static>(g: &GroupState, detail: &GroupDetail) -> El {
    let mut out = col().key("gp-board").w_full().gap(10.0);
    let mut add = row().w_full().items_center().gap(5.0);
    if g.new_milestone_open {
        add = add
            .child(
                rune::text_input(&g.new_milestone, rune::tok::TextStyle::new(10.5, 400.0))
                    .key("gp-new-milestone")
                    .single_line()
                    .grow(1.0)
                    .radius(7.0)
                    .px(8.0)
                    .py(5.0)
                    .fill(ink("#00000066"))
                    .border(1.0, ink("#ffffff14"))
                    .border_layout()
                    .placeholder("Milestone title", ink("#52525b"))
                    .caret(ink("#e4e4e7"), crate::motion::TEXT_SELECTION)
                    .on_edit(|s: &mut S| &mut s.projects_mut().group.new_milestone)
                    .on_submit(|s: &mut S, _tx: &mut rune::Tx| {
                        let g = &mut s.projects_mut().group;
                        let title = g.new_milestone.text.trim().to_string();
                        if !title.is_empty() {
                            g.op_with("add_milestone", serde_json::json!({"title": title}));
                            g.new_milestone.text.clear();
                            g.new_milestone_open = false;
                        }
                    }),
            )
            .child(
                text("Add".to_string())
                    .px(8.0)
                    .py(5.0)
                    .radius(6.0)
                    .fill(ink("#ffffff"))
                    .font(rune::tok::TextStyle::new(9.5, 700.0))
                    .color(ink("#0a0a0d"))
                    .role("button")
                    .focusable(true)
                    .on_press_enter(|s: &mut S| {
                        let g = &mut s.projects_mut().group;
                        let title = g.new_milestone.text.trim().to_string();
                        if !title.is_empty() {
                            g.op_with("add_milestone", serde_json::json!({"title": title}));
                            g.new_milestone.text.clear();
                            g.new_milestone_open = false;
                        }
                    }),
            );
    } else {
        add = add.child(rune::el().grow(1.0)).child(
            text("+ Milestone".to_string())
                .key("gp-add-milestone")
                .px(8.0)
                .py(4.0)
                .radius(6.0)
                .fill(rgba(255.0, 255.0, 255.0, 0.06))
                .font(rune::tok::TextStyle::new(9.5, 600.0))
                .color(ink("#d4d4d8"))
                .role("button")
                .focusable(true)
                .on_press_enter(|s: &mut S| {
                    s.projects_mut().group.new_milestone_open = true;
                }),
        );
    }
    out = out.child(add);
    if detail.milestones.is_empty() {
        out = out.child(
            text("No milestones yet. Add one or ask the orchestrator to plan.".to_string())
                .padding(10.0)
                .font(rune::tok::TextStyle::new(10.0, 400.0))
                .color(ink("#52525b")),
        );
    }
    let current: Vec<&GroupMilestone> = detail.current_milestones().collect();
    for milestone in current {
        let open = g.board_open.as_deref() == Some(milestone.id.as_str());
        let mine = detail.items_for(&milestone.id);
        let done = mine.iter().filter(|item| item.status == "done").count();
        let total = mine.len();
        let mut section = col()
            .key(format!("gp-ms-{}", milestone.id))
            .w_full()
            .radius(9.0)
            .border(1.0, rgba(255.0, 255.0, 255.0, 0.065))
            .border_layout()
            .fill(rgba(255.0, 255.0, 255.0, 0.014))
            .child(
                row()
                    .key(format!("gp-ms-{}-head", milestone.id))
                    .w_full()
                    .items_center()
                    .gap(5.0)
                    .padding(8.0)
                    .role("button")
                    .focusable(true)
                    .hover(rgba(255.0, 255.0, 255.0, 0.04))
                    .on_press_enter({
                        let id = milestone.id.clone();
                        move |s: &mut S| {
                            let g = &mut s.projects_mut().group;
                            g.board_open = if g.board_open.as_deref() == Some(id.as_str()) {
                                None
                            } else {
                                Some(id.clone())
                            };
                        }
                    })
                    .child(icon(
                        12.0,
                        crate::icons::chevron_right(12.0, ink("#52525b")),
                    ))
                    .child(
                        text(milestone.title.clone())
                            .grow(1.0)
                            .min_w(0.0)
                            .truncate()
                            .font(rune::tok::TextStyle::new(11.0, 600.0))
                            .color(ink("#d4d4d8")),
                    )
                    .child(
                        text(format!("{done}/{total}"))
                            .font(rune::tok::TextStyle::new(8.5, 500.0).mono())
                            .color(ink("#71717a")),
                    )
                    .child(status_chip(&milestone.status, false)),
            );
        if open {
            let mut body = col()
                .w_full()
                .gap(6.0)
                .px(8.0)
                .pb(8.0)
                .border_sides([0.0, 1.0, 0.0, 0.0], rgba(255.0, 255.0, 255.0, 0.055))
                .border_layout();
            if !milestone.detail.is_empty() {
                body = body.child(
                    text(milestone.detail.clone())
                        .w_full()
                        .wrap()
                        .font(rune::tok::TextStyle::new(9.5, 400.0))
                        .color(ink("#a1a1aa")),
                );
            }
            body = body.child(status_controls::<S>(milestone));
            let item_id = milestone.id.clone();
            body = body.child(
                row()
                    .w_full()
                    .items_center()
                    .gap(5.0)
                    .child(
                        text("+ Item".to_string())
                            .px(6.0)
                            .py(2.0)
                            .radius(4.0)
                            .font(rune::tok::TextStyle::new(8.5, 600.0))
                            .color(ink("#c4c4c8"))
                            .fill(rgba(255.0, 255.0, 255.0, 0.05))
                            .role("button")
                            .focusable(true)
                            .on_press_enter(move |s: &mut S| {
                                let g = &mut s.projects_mut().group;
                                g.new_item_milestone = Some(item_id.clone());
                                g.new_item.text.clear();
                            }),
                    )
                    .child(rune::el().grow(1.0))
                    .child(
                        text("Delete".to_string())
                            .px(6.0)
                            .py(2.0)
                            .radius(4.0)
                            .font(rune::tok::TextStyle::new(8.5, 600.0))
                            .color(ink("#fda4af"))
                            .role("button")
                            .focusable(true)
                            .on_press_enter({
                                let id = milestone.id.clone();
                                move |s: &mut S| {
                                    let g = &mut s.projects_mut().group;
                                    let key = format!("delete-ms-{id}");
                                    if g.status_busy.as_deref() == Some(key.as_str()) {
                                        g.status_busy = None;
                                        g.op_with(
                                            "delete_milestone",
                                            serde_json::json!({"milestone_id": id}),
                                        );
                                    } else {
                                        g.status_busy = Some(key);
                                    }
                                }
                            }),
                    ),
            );
            if g.new_item_milestone.as_deref() == Some(milestone.id.as_str()) {
                body = body.child(add_item_row::<S>(g, &milestone.id));
            }
            let mut items = col().w_full().gap(3.0);
            if mine.is_empty() {
                items = items.child(
                    text("No items yet.".to_string())
                        .padding(6.0)
                        .font(rune::tok::TextStyle::new(9.5, 400.0))
                        .color(ink("#52525b")),
                );
            }
            for item in mine {
                let open_item = g.item_open.as_deref() == Some(item.id.as_str());
                let item_id = item.id.clone();
                let awaiting = item.awaiting_approval();
                items = items.child(
                    col()
                        .w_full()
                        .radius(6.0)
                        .border_layout()
                        .fill(rgba(255.0, 255.0, 255.0, 0.022))
                        .child(
                            row()
                                .key(format!("gp-board-item-{}", item.id))
                                .w_full()
                                .items_center()
                                .gap(6.0)
                                .min_h(30.0)
                                .px(7.0)
                                .py(4.0)
                                .role("button")
                                .label(format!("{} details", item.title))
                                .focusable(true)
                                .hover(rgba(255.0, 255.0, 255.0, 0.04))
                                .on_press_enter(move |s: &mut S| {
                                    let g = &mut s.projects_mut().group;
                                    g.item_open =
                                        if g.item_open.as_deref() == Some(item_id.as_str()) {
                                            None
                                        } else {
                                            Some(item_id.clone())
                                        };
                                })
                                .child(
                                    text(item.id.clone())
                                        .w(36.0)
                                        .truncate()
                                        .font(rune::tok::TextStyle::new(8.5, 400.0).mono())
                                        .color(ink("#52525b")),
                                )
                                .child(
                                    text(item.title.clone())
                                        .grow(1.0)
                                        .min_w(0.0)
                                        .truncate()
                                        .font(rune::tok::TextStyle::new(10.0, 450.0))
                                        .color(ink("#c4c4c8")),
                                )
                                .child(if !item.owner_label().is_empty() {
                                    text(item.owner_label())
                                        .font(rune::tok::TextStyle::new(8.5, 400.0))
                                        .color(ink("#71717a"))
                                } else {
                                    rune::el().size(0.0, 0.0).inert()
                                })
                                .child(status_chip(&item.status, awaiting)),
                        )
                        .child(if open_item {
                            item_detail::<S>(item)
                        } else {
                            rune::el().size(0.0, 0.0).inert()
                        }),
                );
            }
            body = body.child(items);
            section = section.child(body);
        }
        out = out.child(section);
    }
    let history: Vec<&GroupMilestone> = detail
        .milestones
        .iter()
        .filter(|milestone| milestone.cycle < detail.current_cycle)
        .collect();
    if !history.is_empty() {
        let mut past = col().w_full().gap(3.0);
        past = past.child(
            text("PAST CYCLES".to_string())
                .font(rune::tok::TextStyle::new(8.5, 700.0))
                .color(ink("#52525b")),
        );
        for milestone in history.iter().rev().take(20) {
            past = past.child(
                row()
                    .w_full()
                    .items_center()
                    .gap(6.0)
                    .px(7.0)
                    .py(3.0)
                    .radius(5.0)
                    .fill(rgba(255.0, 255.0, 255.0, 0.018))
                    .child(
                        text(format!("C{}", milestone.cycle))
                            .font(rune::tok::TextStyle::new(8.0, 700.0))
                            .color(ink("#52525b")),
                    )
                    .child(
                        text(milestone.title.clone())
                            .grow(1.0)
                            .truncate()
                            .font(rune::tok::TextStyle::new(9.0, 400.0))
                            .color(ink("#71717a")),
                    )
                    .child(status_chip(&milestone.status, false)),
            );
        }
        out = out.child(past);
    }
    out
}

fn item_detail<S: ProjectsHost + 'static>(item: &GroupItem) -> El {
    let mut detail = col()
        .w_full()
        .gap(6.0)
        .px(8.0)
        .pt(7.0)
        .pb(9.0)
        .border_sides([0.0, 1.0, 0.0, 0.0], rgba(255.0, 255.0, 255.0, 0.055))
        .border_layout();
    if !item.detail.is_empty() {
        detail = detail.child(
            text(item.detail.clone())
                .w_full()
                .wrap()
                .font(rune::tok::TextStyle::new(9.5, 400.0))
                .color(ink("#a1a1aa")),
        );
    }
    if let Some(error) = &item.error {
        detail = detail.child(
            text(format!("! {error}"))
                .w_full()
                .wrap()
                .font(rune::tok::TextStyle::new(9.0, 400.0))
                .color(ink("#fda4af")),
        );
    }
    if let Some(result) = &item.result {
        detail = detail.child(
            col()
                .w_full()
                .max_h(200.0)
                .radius(6.0)
                .border(1.0, rgba(255.0, 255.0, 255.0, 0.055))
                .border_layout()
                .fill(rgba(0.0, 0.0, 0.0, 0.12))
                .padding(7.0)
                .child(super::super::rune_transcript::render_markdown_standalone(
                    &format!("gp-item-{}-result", item.id),
                    result,
                    "#c4c4c8",
                    {
                        let mut style = rune::tok::TextStyle::new(10.0, 400.0);
                        style.line_height = 15.0;
                        style
                    },
                )),
        );
    }
    let mut actions = row().w_full().gap(4.0).flex_wrap();
    if matches!(
        item.status.as_str(),
        "failed" | "cancelled" | "invalidated" | "blocked"
    ) {
        actions = actions.child(item_action::<S>(
            &format!("gp-retry-{}", item.id),
            "Retry",
            "retry_item",
            &item.id,
        ));
    }
    if item.status != "done" && item.status != "doing" {
        actions = actions.child(item_action::<S>(
            &format!("gp-complete-{}", item.id),
            "Mark done",
            "complete_item",
            &item.id,
        ));
    }
    if item.status != "cancelled" && item.status != "doing" {
        actions = actions.child(item_action::<S>(
            &format!("gp-cancel-{}", item.id),
            "Cancel",
            "cancel_item",
            &item.id,
        ));
    }
    if item.status == "cancelled" || item.status == "done" {
        actions = actions.child(item_action::<S>(
            &format!("gp-reopen-{}", item.id),
            "Reopen",
            "reopen_item",
            &item.id,
        ));
    }
    actions = actions.child(item_action::<S>(
        &format!("gp-delete-{}", item.id),
        "Delete",
        "delete_item",
        &item.id,
    ));
    detail = detail.child(actions);
    if item.acceptance.as_array().is_some_and(|a| !a.is_empty()) {
        detail = detail.child(
            col()
                .gap(3.0)
                .child(
                    text("DONE MEANS".to_string())
                        .font(rune::tok::TextStyle::new(8.5, 650.0))
                        .color(ink("#71717a")),
                )
                .child(
                    text(serde_json::to_string_pretty(&item.acceptance).unwrap_or_default())
                        .w_full()
                        .wrap()
                        .font(rune::tok::TextStyle::new(8.5, 400.0).mono())
                        .color(ink("#a1a1aa")),
                ),
        );
    }
    detail.child(
        row()
            .flex_wrap()
            .gap(8.0)
            .child(
                text(format!(
                    "{}/{} attempts",
                    item.attempts,
                    item.max_attempts.max(1)
                ))
                .font(rune::tok::TextStyle::new(8.5, 400.0))
                .color(ink("#52525b")),
            )
            .child(if !item.owner_label().is_empty() {
                text(format!("owner {}", item.owner_label()))
                    .font(rune::tok::TextStyle::new(8.5, 400.0))
                    .color(ink("#52525b"))
            } else {
                rune::el().size(0.0, 0.0).inert()
            }),
    )
}

fn team<S: ProjectsHost + 'static>(g: &GroupState, detail: &GroupDetail) -> El {
    let mut out = col().key("gp-team").w_full().gap(7.0);
    if detail.members.is_empty() {
        return text("No team slots yet. Open settings to add agents.".to_string())
            .padding(10.0)
            .font(rune::tok::TextStyle::new(10.0, 400.0))
            .color(ink("#52525b"));
    }
    out = out.child(
        row()
            .w_full()
            .items_center()
            .child(
                text("TEAM USAGE".to_string())
                    .grow(1.0)
                    .font(rune::tok::TextStyle::new(9.0, 700.0))
                    .color(ink("#71717a")),
            )
            .child(
                text("Manage".to_string())
                    .px(6.0)
                    .py(2.0)
                    .radius(4.0)
                    .font(rune::tok::TextStyle::new(8.5, 600.0))
                    .color(ink("#a1a1aa"))
                    .role("button")
                    .focusable(true)
                    .on_press_enter(|s: &mut S| {
                        super::settings::open(&mut s.projects_mut().group);
                    }),
            ),
    );
    for member in &detail.members {
        let active = member.running;
        out = out.child(
            col()
                .key(format!("gp-member-{}", member.id))
                .w_full()
                .gap(4.0)
                .padding(8.0)
                .radius(8.0)
                .fill(rgba(255.0, 255.0, 255.0, 0.022))
                .child(
                    row()
                        .w_full()
                        .items_center()
                        .gap(7.0)
                        .child(avatar(&member.label(), true, false))
                        .child(
                            col()
                                .grow(1.0)
                                .min_w(0.0)
                                .gap(1.0)
                                .child(
                                    text(member.label())
                                        .font(rune::tok::TextStyle::new(10.5, 600.0))
                                        .color(ink("#d4d4d8")),
                                )
                                .child(
                                    text(
                                        member
                                            .current_item
                                            .clone()
                                            .unwrap_or_else(|| member.agent_key.clone()),
                                    )
                                    .truncate()
                                    .font(rune::tok::TextStyle::new(8.5, 400.0))
                                    .color(if active {
                                        ink("#7dd3fc")
                                    } else {
                                        ink("#71717a")
                                    }),
                                ),
                        )
                        .child(rune::el().size(5.0, 5.0).radius(2.5).fill(if active {
                            ink("#7dd3fc")
                        } else {
                            ink("#3f3f46")
                        })),
                )
                .child(
                    row()
                        .w_full()
                        .gap(9.0)
                        .child(
                            text(format!("{}/{} done", member.done, member.assigned))
                                .font(rune::tok::TextStyle::new(8.5, 500.0))
                                .color(ink("#a1a1aa")),
                        )
                        .child(
                            text(format!("{} tokens", member.tokens))
                                .font(rune::tok::TextStyle::new(8.5, 400.0).mono())
                                .color(ink("#71717a")),
                        )
                        .child(
                            text(money(member.cost_usd))
                                .font(rune::tok::TextStyle::new(8.5, 400.0).mono())
                                .color(ink("#71717a")),
                        ),
                ),
        );
    }
    let _ = g;
    out
}

fn details<S: ProjectsHost + 'static>(g: &GroupState, detail: &GroupDetail) -> El {
    let status = if detail.project.status.is_empty() {
        "unknown"
    } else {
        detail.project.status.as_str()
    };
    let path = if detail.workspace_path.is_empty() {
        detail.path.clone()
    } else {
        detail.workspace_path.clone()
    };
    let mut out = card().key("gp-details").pb(9.0).child(section_title(
        "Project",
        crate::icons::users(12.0, ink("#71717a")),
    ));
    if !detail.project.goal.is_empty() {
        out = out.child(
            text(detail.project.goal.clone())
                .mx(12.0)
                .mb(8.0)
                .font(rune::tok::TextStyle::new(11.0, 400.0))
                .color(ink("#a1a1aa")),
        );
    }
    for (label, value) in [
        ("Status", status.to_uppercase()),
        (
            "Milestones",
            format!(
                "{}/{}",
                detail.milestones_done(),
                detail.current_milestones().count()
            ),
        ),
        (
            "Items",
            format!("{}/{}", detail.items_done(), detail.current_items().count()),
        ),
        (
            "Schedule",
            match detail.project.schedule.repeat_label().as_str() {
                "No repeat" => "One cycle".to_string(),
                other => other.to_string(),
            },
        ),
        (
            "Spend",
            format!(
                "{}{}",
                money(detail.project.spent_usd.unwrap_or(detail.usage.cost_usd)),
                detail
                    .project
                    .budget_usd
                    .map(|cap| format!(" / {}", money(cap)))
                    .unwrap_or_default()
            ),
        ),
        (
            "Usage",
            format!(
                "{} runs · {} tokens",
                detail.usage.runs,
                detail.usage.tokens()
            ),
        ),
    ] {
        out = out.child(
            row()
                .w_full()
                .justify_between()
                .gap(10.0)
                .px(12.0)
                .py(4.0)
                .child(
                    text(label.to_string())
                        .font(rune::tok::TextStyle::new(10.0, 400.0))
                        .color(ink("#71717a")),
                )
                .child(
                    text(value)
                        .font(crate::motion::tabular(rune::tok::TextStyle::new(
                            10.0, 600.0,
                        )))
                        .color(ink("#d4d4d8")),
                ),
        );
    }
    if detail.shell_enabled {
        out = out.child(
            row()
                .items_center()
                .gap(6.0)
                .mx(12.0)
                .my(7.0)
                .child(icon(12.0, crate::icons::shield(12.0, ink("#71717a"))))
                .child(
                    text("Members can run shell commands.".to_string())
                        .font(rune::tok::TextStyle::new(9.5, 400.0))
                        .color(ink("#71717a")),
                ),
        );
    }
    if !path.is_empty() {
        out = out.child(
            row()
                .items_center()
                .gap(6.0)
                .mx(12.0)
                .my(7.0)
                .padding(6.0)
                .radius(6.0)
                .border(1.0, rgba(255.0, 255.0, 255.0, 0.07))
                .border_layout()
                .role("button")
                .label("Open project folder")
                .focusable(true)
                .focus_ring(2.0, 2.0, rgba(255.0, 255.0, 255.0, 0.55))
                .on_press_tx_enter({
                    let path = path.clone();
                    move |s: &mut S, _tx: &mut rune::Tx| {
                        #[cfg(target_os = "windows")]
                        {
                            let _ = std::process::Command::new("explorer").arg(&path).spawn();
                            s.projects_mut()
                                .group
                                .set_path_note("Opened folder.".to_string());
                        }
                        #[cfg(not(target_os = "windows"))]
                        {
                            _tx.copy(path.clone());
                            s.projects_mut()
                                .group
                                .set_path_note("Path copied.".to_string());
                        }
                    }
                })
                .child(icon(12.0, crate::icons::folder_open(12.0, ink("#a1a1aa"))))
                .child(
                    text(path)
                        .font(rune::tok::TextStyle::new(9.5, 400.0))
                        .color(ink("#a1a1aa")),
                ),
        );
    }
    let show_activity = g.show_activity;
    out = out.child(
        row()
            .key("gp-activity-toggle")
            .items_center()
            .gap(4.0)
            .mx(12.0)
            .mt(7.0)
            .role("button")
            .label("Toggle activity")
            .focusable(true)
            .focus_ring(2.0, 2.0, rgba(255.0, 255.0, 255.0, 0.55))
            .on_press_enter(move |s: &mut S| {
                s.projects_mut().group.show_activity = !show_activity;
            })
            .child(
                icon(12.0, crate::icons::chevron_right(12.0, ink("#71717a"))).rotate(
                    if show_activity {
                        std::f32::consts::FRAC_PI_2
                    } else {
                        0.0
                    },
                ),
            )
            .child(
                text(format!("Activity · {}", detail.events.len()))
                    .font(rune::tok::TextStyle::new(9.5, 600.0))
                    .color(ink("#71717a")),
            ),
    );
    if g.show_activity {
        let mut chips = row().w_full().gap(3.0).flex_wrap().mt(5.0).px(12.0);
        for filter in ["all", "items", "milestones", "approvals", "system"] {
            let active = g.activity_filter == filter;
            let label = match filter {
                "items" => "Items",
                "milestones" => "Milestones",
                "approvals" => "Approvals",
                "system" => "System",
                _ => "All",
            };
            let value = filter.to_string();
            chips = chips.child(
                text(label.to_string())
                    .px(6.0)
                    .py(1.0)
                    .radius(4.0)
                    .font(rune::tok::TextStyle::new(8.0, 600.0))
                    .color(if active {
                        ink("#0a0a0d")
                    } else {
                        ink("#a1a1aa")
                    })
                    .fill(if active {
                        ink("#e4e4e7")
                    } else {
                        rgba(255.0, 255.0, 255.0, 0.045)
                    })
                    .role("button")
                    .focusable(true)
                    .on_press_enter(move |s: &mut S| {
                        s.projects_mut().group.activity_filter = value.clone();
                    }),
            );
        }
        out = out.child(chips);
        let mut list = col().w_full().gap(1.0).mt(6.0).px(12.0);
        if detail.events.is_empty() {
            list = list.child(
                text("Nothing logged yet.".to_string())
                    .font(rune::tok::TextStyle::new(10.0, 400.0))
                    .color(ink("#52525b")),
            );
        }
        for event in detail
            .events
            .iter()
            .rev()
            .filter(|event| match g.activity_filter.as_str() {
                "items" => event.kind.starts_with("item."),
                "milestones" => {
                    event.kind.starts_with("milestone.") || event.kind.starts_with("plan.")
                }
                "approvals" => event.kind.starts_with("approval."),
                "system" => {
                    event.kind.starts_with("project.")
                        || event.kind.starts_with("cycle.")
                        || event.kind.starts_with("run.")
                        || event.kind.starts_with("workspace.")
                }
                _ => true,
            })
            .take(80)
        {
            list = list.child(
                row()
                    .w_full()
                    .gap(6.0)
                    .child(
                        text(time_ago(event.ts))
                            .font(rune::tok::TextStyle::new(9.0, 400.0))
                            .color(ink("#52525b")),
                    )
                    .child(
                        text(event_label(&event.kind))
                            .grow(1.0)
                            .min_w(0.0)
                            .truncate()
                            .font(rune::tok::TextStyle::new(9.0, 400.0))
                            .color(ink("#71717a")),
                    )
                    .child(
                        text(display_actor(&event.actor))
                            .font(rune::tok::TextStyle::new(9.0, 400.0))
                            .color(ink("#52525b")),
                    ),
            );
        }
        out = out.child(list);
    }
    if !detail.runs.is_empty() {
        let mut runs = col().w_full().gap(3.0).mt(6.0).px(12.0);
        runs = runs.child(
            text("RUNS".to_string())
                .font(rune::tok::TextStyle::new(8.5, 700.0))
                .color(ink("#52525b")),
        );
        for run in detail.runs.iter().take(12) {
            let run_id = run.id.clone();
            let run_clone = run.clone();
            runs = runs.child(
                row()
                    .key(format!("gp-run-{}", run.id))
                    .w_full()
                    .items_center()
                    .gap(6.0)
                    .py(2.0)
                    .role("button")
                    .focusable(true)
                    .hover(rgba(255.0, 255.0, 255.0, 0.04))
                    .on_press_enter(move |s: &mut S| {
                        let g = &mut s.projects_mut().group;
                        g.selected_run = if g.selected_run.as_ref().is_some_and(|r| r.id == run_id)
                        {
                            None
                        } else {
                            Some(run_clone.clone())
                        };
                    })
                    .child(
                        text(run.kind.clone())
                            .w(42.0)
                            .font(rune::tok::TextStyle::new(8.0, 600.0))
                            .color(if run.status == "failed" {
                                ink("#fda4af")
                            } else {
                                ink("#71717a")
                            }),
                    )
                    .child(
                        text(
                            run.item_title
                                .clone()
                                .or_else(|| run.member.clone())
                                .unwrap_or_else(|| run.id.clone()),
                        )
                        .grow(1.0)
                        .truncate()
                        .font(rune::tok::TextStyle::new(8.5, 400.0))
                        .color(ink("#a1a1aa")),
                    )
                    .child(
                        text(format!("{}t", run.tokens()))
                            .font(rune::tok::TextStyle::new(8.0, 400.0).mono())
                            .color(ink("#52525b")),
                    )
                    .child(
                        text(money(run.cost_usd))
                            .font(rune::tok::TextStyle::new(8.0, 400.0).mono())
                            .color(ink("#52525b")),
                    ),
            );
        }
        out = out.child(runs);
    }
    if let Some(run) = &g.selected_run {
        let mut card = card().key("gp-run-detail").pb(8.0).child(section_title(
            "Run detail",
            crate::icons::terminal(12.0, ink("#71717a")),
        ));
        card = card.child(
            row()
                .w_full()
                .gap(8.0)
                .px(12.0)
                .py(3.0)
                .child(
                    text(format!("{} · {}", run.kind, run.status))
                        .font(rune::tok::TextStyle::new(9.0, 600.0))
                        .color(if run.status == "failed" {
                            ink("#fda4af")
                        } else {
                            ink("#a1a1aa")
                        }),
                )
                .child(
                    text(format!("{} tokens", run.tokens()))
                        .font(rune::tok::TextStyle::new(9.0, 400.0).mono())
                        .color(ink("#71717a")),
                )
                .child(
                    text(money(run.cost_usd))
                        .font(rune::tok::TextStyle::new(9.0, 400.0).mono())
                        .color(ink("#71717a")),
                )
                .child(rune::el().grow(1.0))
                .child(
                    text("Close".to_string())
                        .px(6.0)
                        .py(2.0)
                        .radius(4.0)
                        .font(rune::tok::TextStyle::new(8.5, 600.0))
                        .color(ink("#a1a1aa"))
                        .role("button")
                        .focusable(true)
                        .on_press_enter(|s: &mut S| {
                            s.projects_mut().group.selected_run = None;
                        }),
                ),
        );
        if let Some(report) = &run.report {
            card = card.child(
                col()
                    .w_full()
                    .max_h(260.0)
                    .scroll(rune::ScrollSpec {
                        stick_bottom: false,
                    })
                    .px(12.0)
                    .pt(5.0)
                    .child(
                        text(report.clone())
                            .w_full()
                            .wrap()
                            .font(rune::tok::TextStyle::new(9.0, 400.0).mono())
                            .color(ink("#a1a1aa")),
                    ),
            );
        }
        if let Some(error) = &run.error {
            card = card.child(
                text(error.clone())
                    .w_full()
                    .wrap()
                    .px(12.0)
                    .pt(5.0)
                    .font(rune::tok::TextStyle::new(9.0, 400.0))
                    .color(ink("#fda4af")),
            );
        }
        out = out.child(card);
    }
    out
}

pub fn rail_panel<S: ProjectsHost + 'static>(
    g: &GroupState,
    detail: &GroupDetail,
    _panel_w: f32,
    compact: bool,
) -> El {
    let members = if detail.members.is_empty() {
        detail
            .items
            .iter()
            .filter_map(|i| i.owner_member.clone().or(i.lease_owner.clone()))
            .collect::<HashSet<String>>()
            .len()
    } else {
        detail.members.len()
    };
    let mut rail = col().key("gp-rail").min_h(0.0).fill(ink("#111111"));
    rail = if compact {
        rail.w_full().grow(1.0)
    } else {
        rail.w(350.0)
            .border_sides([1.0, 0.0, 0.0, 0.0], rgba(255.0, 255.0, 255.0, 0.07))
            .border_layout()
    };
    let rail = rail.child(
        col()
            .key("gp-rail-scroll")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .gap(10.0)
            .padding(14.0)
            .scroll(rune::ScrollSpec {
                stick_bottom: false,
            })
            .scroll_margin(56.0)
            .child({
                let mut top = progress_card(detail);
                if !detail.waiting().is_empty() {
                    top = top.child(waiting_card::<S>(g, detail));
                }
                top
            })
            .child(tabs::<S>(g, members))
            .child(match g.rail_tab {
                RailTab::Board => board::<S>(g, detail),
                RailTab::Team => team::<S>(g, detail),
                RailTab::Activity => details::<S>(g, detail),
            }),
    );
    rail
}

use std::collections::HashSet;
