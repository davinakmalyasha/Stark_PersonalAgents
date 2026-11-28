//! `GroupProjectRail`: progress, waiting approvals, board/team/details.

use crate::keys::EnterPress;
use rune::{col, row, text, El};

use super::data::{GroupDetail, GroupItem, GroupMember};
use super::ui::{
    active_item_for, avatar, display_actor, event_label, money, status_chip, time_ago,
};
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
        (RailTab::Details, "Details".to_string()),
    ] {
        let active = g.rail_tab == tab;
        row = row.child(
            text(label)
                .key(format!(
                    "gp-tab-{}",
                    match tab {
                        RailTab::Board => "board",
                        RailTab::Team => "team",
                        RailTab::Details => "details",
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
                    RailTab::Details => "Details",
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

fn board<S: ProjectsHost + 'static>(g: &GroupState, detail: &GroupDetail) -> El {
    if detail.milestones.is_empty() {
        return text("No milestones yet.".to_string())
            .padding(10.0)
            .font(rune::tok::TextStyle::new(10.0, 400.0))
            .color(ink("#52525b"));
    }
    let mut out = col().key("gp-board").w_full().gap(10.0);
    for milestone in &detail.milestones {
        let mine: Vec<&GroupItem> = detail
            .items
            .iter()
            .filter(|i| i.milestone_id.as_deref() == Some(milestone.id.as_str()))
            .collect();
        let mut section = col()
            .w_full()
            .radius(9.0)
            .border(1.0, rgba(255.0, 255.0, 255.0, 0.065))
            .border_layout()
            .fill(rgba(255.0, 255.0, 255.0, 0.014))
            .child(
                row()
                    .w_full()
                    .items_center()
                    .gap(5.0)
                    .padding(8.0)
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
                    .child(status_chip(&milestone.status, false)),
            );
        let mut items = col().w_full().gap(3.0).px(6.0).pb(7.0);
        if mine.is_empty() {
            items = items.child(
                text("No items yet.".to_string())
                    .padding(8.0)
                    .font(rune::tok::TextStyle::new(10.0, 400.0))
                    .color(ink("#52525b")),
            );
        }
        for item in mine {
            let open = g.board_open.as_deref() == Some(item.id.as_str());
            let id = item.id.clone();
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
                            .focus_ring(2.0, 2.0, rgba(255.0, 255.0, 255.0, 0.55))
                            .hover(rgba(255.0, 255.0, 255.0, 0.04))
                            .on_press_enter(move |s: &mut S| {
                                let g = &mut s.projects_mut().group;
                                g.board_open = if g.board_open.as_deref() == Some(id.as_str()) {
                                    None
                                } else {
                                    Some(id.clone())
                                };
                            })
                            .child(
                                text(item.id.clone())
                                    .w(36.0)
                                    .truncate()
                                    .font(rune::tok::TextStyle::new(9.0, 400.0).mono())
                                    .color(ink("#52525b")),
                            )
                            .child(
                                text(item.title.clone())
                                    .grow(1.0)
                                    .min_w(0.0)
                                    .truncate()
                                    .font(rune::tok::TextStyle::new(10.5, 400.0))
                                    .color(ink("#c4c4c8")),
                            )
                            .child(if let Some(owner) = &item.lease_owner {
                                text(display_actor(owner))
                                    .font(rune::tok::TextStyle::new(9.0, 400.0))
                                    .color(ink("#71717a"))
                            } else {
                                rune::el().size(0.0, 0.0).inert()
                            })
                            .child(status_chip(&item.status, awaiting)),
                    )
                    .child(if open {
                        item_detail(item)
                    } else {
                        rune::el().size(0.0, 0.0).inert()
                    }),
            );
        }
        section = section.child(items);
        out = out.child(section);
    }
    out
}

fn item_detail(item: &GroupItem) -> El {
    let mut detail = col()
        .w_full()
        .gap(7.0)
        .px(9.0)
        .pt(8.0)
        .pb(10.0)
        .border_sides([0.0, 1.0, 0.0, 0.0], rgba(255.0, 255.0, 255.0, 0.055))
        .border_layout()
        .font(rune::tok::TextStyle::new(10.5, 400.0));
    if !item.detail.is_empty() {
        detail = detail.child(text(item.detail.clone()).color(ink("#a1a1aa")));
    }
    if let Some(error) = &item.error {
        detail = detail.child(text(format!("! {error}")).color(ink("#fda4af")));
    }
    if let Some(result) = &item.result {
        detail = detail.child(
            col()
                .w_full()
                .max_h(220.0)
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
                        let mut style = rune::tok::TextStyle::new(10.5, 400.0);
                        style.line_height = 15.75;
                        style
                    },
                )),
        );
    }
    if item.acceptance.as_array().is_some_and(|a| !a.is_empty()) {
        detail = detail.child(
            col()
                .gap(4.0)
                .child(
                    text("DONE MEANS".to_string())
                        .font(rune::tok::TextStyle::new(9.0, 650.0))
                        .color(ink("#71717a")),
                )
                .child(
                    text(serde_json::to_string_pretty(&item.acceptance).unwrap_or_default())
                        .font(rune::tok::TextStyle::new(9.0, 400.0).mono())
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
                    item.max_attempts.max(3)
                ))
                .font(rune::tok::TextStyle::new(9.0, 400.0))
                .color(ink("#52525b")),
            )
            .child(if let Some(owner) = &item.owner_member {
                text(format!("owner {}", display_actor(owner)))
                    .font(rune::tok::TextStyle::new(9.0, 400.0))
                    .color(ink("#52525b"))
            } else {
                rune::el().size(0.0, 0.0).inert()
            })
            .child(if item.cost_usd.unwrap_or(0.0) > 0.0 {
                text(money(item.cost_usd.unwrap_or(0.0)))
                    .font(rune::tok::TextStyle::new(9.0, 400.0))
                    .color(ink("#52525b"))
            } else {
                rune::el().size(0.0, 0.0).inert()
            }),
    )
}

fn team(detail: &GroupDetail) -> El {
    let members: Vec<GroupMember> = if detail.members.is_empty() {
        let mut names: Vec<(String, String)> = Vec::new();
        for item in &detail.items {
            for name in [&item.owner_member, &item.lease_owner]
                .into_iter()
                .flatten()
            {
                if !names.iter().any(|(n, _)| n == name) {
                    names.push((name.clone(), display_actor(name)));
                }
            }
        }
        names
            .into_iter()
            .map(|(name, display)| GroupMember {
                name,
                display,
                kind: "member".to_string(),
                model: String::new(),
            })
            .collect()
    } else {
        detail.members.clone()
    };
    let mut out = col().key("gp-team").w_full().gap(3.0);
    if members.is_empty() {
        return text("No named members.".to_string())
            .padding(10.0)
            .font(rune::tok::TextStyle::new(10.0, 400.0))
            .color(ink("#52525b"));
    }
    for member in &members {
        let active = active_item_for(&detail.items, &member.name);
        out = out.child(
            row()
                .w_full()
                .items_center()
                .gap(8.0)
                .padding(8.0)
                .radius(8.0)
                .fill(rgba(255.0, 255.0, 255.0, 0.02))
                .child(avatar(&member.name, true, false))
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(1.0)
                        .child(
                            text(if member.display.is_empty() {
                                display_actor(&member.name)
                            } else {
                                member.display.clone()
                            })
                            .font(rune::tok::TextStyle::new(11.0, 600.0))
                            .color(ink("#d4d4d8")),
                        )
                        .child(
                            text(active.map(|item| item.title.clone()).unwrap_or_else(|| {
                                if !member.model.is_empty() {
                                    member.model.clone()
                                } else if !member.kind.is_empty() {
                                    member.kind.clone()
                                } else {
                                    "member".to_string()
                                }
                            }))
                            .font(rune::tok::TextStyle::new(9.5, 400.0))
                            .color(ink("#71717a")),
                        ),
                )
                .child(
                    rune::el()
                        .size(5.0, 5.0)
                        .radius(2.5)
                        .fill(if active.is_some() {
                            ink("#e4e4e7")
                        } else {
                            ink("#3f3f46")
                        }),
                ),
        );
    }
    out
}

fn details<S: ProjectsHost + 'static>(g: &GroupState, detail: &GroupDetail) -> El {
    let status = if detail.project.status.is_empty() {
        "unknown"
    } else {
        detail.project.status.as_str()
    };
    let path = detail.path.clone();
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
            format!("{}/{}", detail.milestones_done(), detail.milestones.len()),
        ),
        (
            "Items",
            format!("{}/{}", detail.items_done(), detail.items.len()),
        ),
        (
            "Spend",
            format!(
                "{}{}",
                money(detail.project.spent_usd.unwrap_or(0.0)),
                detail
                    .project
                    .budget_usd
                    .map(|cap| format!(" / {}", money(cap)))
                    .unwrap_or_default()
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
        let mut list = col().w_full().gap(1.0).mt(6.0).px(12.0);
        if detail.events.is_empty() {
            list = list.child(
                text("Nothing logged yet.".to_string())
                    .font(rune::tok::TextStyle::new(10.0, 400.0))
                    .color(ink("#52525b")),
            );
        }
        for event in detail.events.iter().rev().take(30) {
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
                RailTab::Team => team(detail),
                RailTab::Details => details::<S>(g, detail),
            }),
    );
    rail
}

use std::collections::HashSet;
