//! `GroupProjectChat`: participants, messages, approvals and the composer.

use std::collections::{HashMap, HashSet};

use rune::{col, row, text, El};

use super::data::{GroupDetail, GroupItem};
use super::ui::{
    actor_label, avatar, chat_entries, initials, is_owner, is_supervisor, participant_names,
    same_actor, short_time, target_label, ChatEntry,
};
use super::{GroupState, Menu};
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

fn icon(size: f32, shape: rune::Shape) -> El {
    rune::el().size(size, size).inert().shape(shape)
}

fn participant_avatar(name: &str) -> El {
    rune::el()
        .size(16.0, 16.0)
        .radius(4.0)
        .center()
        .fill(rgba(255.0, 255.0, 255.0, 0.06))
        .child(
            text(initials(name))
                .font(rune::tok::TextStyle::new(7.0, 750.0))
                .color(ink("#d4d4d8")),
        )
}

fn filter_button(
    label: &str,
    active: bool,
    mouth: Option<&str>,
    count: Option<usize>,
    glyph: Option<rune::Shape>,
) -> El {
    let mut el = row()
        .items_center()
        .gap(4.0)
        .h(24.0)
        .px(7.0)
        .radius(6.0)
        .role("tab")
        .label(label.to_string());
    if active {
        el = el.fill(rgba(255.0, 255.0, 255.0, 0.07));
    }
    let mut body = row().items_center().gap(4.0);
    if let Some(name) = mouth {
        body = body.child(participant_avatar(name));
    }
    if let Some(shape) = glyph {
        body = body.child(icon(11.0, shape));
    }
    body = body.child(
        text(label.to_string())
            .font(rune::tok::TextStyle::new(10.0, 500.0))
            .color(if active {
                ink("#f4f4f5")
            } else {
                ink("#71717a")
            }),
    );
    if let Some(count) = count.filter(|c| *c > 0) {
        body = body.child(
            text(count.to_string())
                .font(rune::tok::TextStyle::new(8.5, 650.0))
                .color(ink("#71717a")),
        );
    }
    el.child(body)
}

fn live_chip(status: &str) -> El {
    let live = status == "running" || status == "active";
    let mut dot = rune::el().size(4.0, 4.0).radius(2.0).fill(if live {
        ink("#7ee0a8")
    } else {
        ink("#6f727b")
    });
    if live {
        dot = dot.looping(rune::LoopSpec::opacity(1.4, 0.0, 0.55, 1.0));
    }
    row()
        .key("gp-chat-live")
        .items_center()
        .gap(5.0)
        .px(7.0)
        .py(2.0)
        .radius(9.0)
        .border(
            1.0,
            if live {
                rgba(126.0, 224.0, 168.0, 0.18)
            } else {
                rgba(255.0, 255.0, 255.0, 0.07)
            },
        )
        .border_layout()
        .child(dot)
        .child(
            text(if live {
                "Live".to_string()
            } else if status.is_empty() {
                "Offline".to_string()
            } else {
                status.to_string()
            })
            .font(rune::tok::TextStyle::new(9.0, 650.0))
            .color(if live { ink("#86efac") } else { ink("#6f727b") }),
        )
}

fn approval_card<S: ProjectsHost + 'static>(item: &GroupItem, busy: bool) -> El {
    let resolve_id = item.id.clone();
    let reject_id = item.id.clone();
    let copy = if item.blocked_reason.as_deref() == Some("awaiting_approval") {
        "This work item is paused until you approve or reject it.".to_string()
    } else {
        item.blocked_reason
            .clone()
            .unwrap_or_else(|| "Waiting for your decision.".to_string())
    };
    let mut approve = row()
        .key(format!("gp-approve-{}", item.id))
        .items_center()
        .gap(5.0)
        .h(26.0)
        .px(9.0)
        .radius(7.0)
        .center()
        .fill(ink("#ffffff"))
        .role("button")
        .label(format!("Approve {}", item.title))
        .child(icon(13.0, crate::lucide_check(13.0, 2.0, ink("#0a0a0d"))))
        .child(
            text(if busy {
                "Working…".to_string()
            } else {
                "Approve".to_string()
            })
            .font(rune::tok::TextStyle::new(10.0, 550.0))
            .color(ink("#0a0a0d")),
        );
    if !busy {
        approve = approve.on_press(move |s: &mut S| {
            let g = &mut s.projects_mut().group;
            if let Some(item) = g.detail.as_ref().and_then(|d| d.item(&resolve_id).cloned()) {
                g.resolve(&item, true);
            }
        });
    }
    let mut reject = row()
        .key(format!("gp-reject-{}", item.id))
        .items_center()
        .gap(5.0)
        .h(26.0)
        .px(9.0)
        .radius(7.0)
        .center()
        .fill(rgba(255.0, 255.0, 255.0, 0.035))
        .border(1.0, rgba(255.0, 255.0, 255.0, 0.1))
        .border_layout()
        .role("button")
        .label(format!("Reject {}", item.title))
        .child(icon(13.0, crate::lucide_x(13.0, 2.0, ink("#d4d4d8"))))
        .child(
            text("Reject".to_string())
                .font(rune::tok::TextStyle::new(10.0, 550.0))
                .color(ink("#d4d4d8")),
        );
    if !busy {
        reject = reject.on_press(move |s: &mut S| {
            let g = &mut s.projects_mut().group;
            if let Some(item) = g.detail.as_ref().and_then(|d| d.item(&reject_id).cloned()) {
                g.resolve(&item, false);
            }
        });
    }
    row()
        .key(format!("gp-approval-{}", item.id))
        .w_full()
        .items_start()
        .gap(9.0)
        .padding(11.0)
        .radius(10.0)
        .border(1.0, rgba(251.0, 191.0, 36.0, 0.22))
        .border_layout()
        .fill(rgba(251.0, 191.0, 36.0, 0.055))
        .child(
            rune::el()
                .size(25.0, 25.0)
                .radius(7.0)
                .center()
                .fill(rgba(251.0, 191.0, 36.0, 0.12))
                .child(icon(
                    15.0,
                    crate::icons::triangle_alert(15.0, ink("#fcd34d")),
                )),
        )
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(2.0)
                .child(
                    text(item.title.clone())
                        .font(rune::tok::TextStyle::new(12.0, 600.0))
                        .color(ink("#f4f4f5")),
                )
                .child(
                    text(copy)
                        .font(rune::tok::TextStyle::new(11.0, 400.0))
                        .color(ink("#a1a1aa")),
                )
                .child(
                    row()
                        .items_center()
                        .gap(6.0)
                        .mt(6.0)
                        .child(approve)
                        .child(reject),
                ),
        )
}

fn message_head(entry: &ChatEntry) -> El {
    let mut head = row()
        .w_full()
        .items_center()
        .flex_wrap()
        .gap(5.0)
        .mb(4.0)
        .child(
            text(actor_label(&entry.from))
                .font(rune::tok::TextStyle::new(10.5, 650.0))
                .color(ink("#dedee2")),
        )
        .child(
            text(format!(
                "to {}",
                if is_owner(&entry.to) {
                    "owner".to_string()
                } else {
                    target_label(&entry.to)
                }
            ))
            .font(rune::tok::TextStyle::new(8.5, 400.0))
            .px(5.0)
            .py(1.0)
            .radius(4.0)
            .border(1.0, rgba(255.0, 255.0, 255.0, 0.065))
            .border_layout()
            .color(ink("#777b85")),
        );
    if entry.derived {
        head = head.child(
            text("log".to_string())
                .font(rune::tok::TextStyle::new(8.0, 650.0))
                .px(5.0)
                .py(1.0)
                .radius(4.0)
                .fill(rgba(255.0, 255.0, 255.0, 0.05))
                .color(ink("#71717a")),
        );
    }
    head.child(rune::el().grow(1.0)).child(
        text(short_time(entry.ts))
            .font(rune::tok::TextStyle::new(9.0, 400.0))
            .color(ink("#4f525a")),
    )
}

fn message_body<S: ProjectsHost + 'static>(g: &GroupState, entry: &ChatEntry) -> El {
    let long = entry.body.len() > 700 || entry.body.lines().count() > 10;
    let expanded = g.expanded.contains(&entry.id);
    let mut body = col()
        .w_full()
        .gap(0.0)
        .px(12.0)
        .py(10.0)
        .radius(10.0)
        .border(
            1.0,
            if is_owner(&entry.from) {
                rgba(255.0, 255.0, 255.0, 0.12)
            } else {
                rgba(255.0, 255.0, 255.0, 0.07)
            },
        )
        .border_layout()
        .fill(if is_owner(&entry.from) {
            rgba(255.0, 255.0, 255.0, 0.045)
        } else {
            ink("#141414")
        });
    if let Some(item_id) = &entry.item_id {
        body = body.child(
            row().self_start().mb(6.0).child(
                text(item_id.clone())
                    .font(rune::tok::TextStyle::new(9.0, 400.0).mono())
                    .px(6.0)
                    .py(1.0)
                    .radius(5.0)
                    .border(1.0, rgba(255.0, 255.0, 255.0, 0.07))
                    .border_layout()
                    .color(ink("#71717a")),
            ),
        );
    }
    let mut inner = super::super::rune_transcript::render_markdown_standalone(
        &format!("gp-{}-body", entry.id),
        &entry.body,
        "#d6d7dc",
        {
            let mut style = rune::tok::TextStyle::new(12.5, 400.0);
            style.line_height = 20.0;
            style
        },
    );
    if long && !expanded {
        inner = inner.max_h(260.0).clip_fade(0.0, 0.0, 0.0, 40.0);
    }
    body = body.child(inner);
    let mut out = col().w_full().gap(0.0).child(body);
    if long {
        let id = entry.id.clone();
        out = out.child(
            text(
                if expanded {
                    "Show less"
                } else {
                    "Show full report"
                }
                .to_string(),
            )
            .key(format!("gp-{}-expand", entry.id))
            .mt(7.0)
            .font(rune::tok::TextStyle::new(10.5, 600.0))
            .color(ink("#b9bbc2"))
            .role("button")
            .label(if expanded {
                "Show less"
            } else {
                "Show full report"
            })
            .on_press(move |s: &mut S| {
                let g = &mut s.projects_mut().group;
                if !g.expanded.remove(&id) {
                    g.expanded.insert(id.clone());
                }
            }),
        );
    }
    out
}

fn message_entry<S: ProjectsHost + 'static>(
    g: &GroupState,
    entry: &ChatEntry,
    previous: Option<&ChatEntry>,
) -> El {
    let continuation = previous.is_some_and(|prev| {
        prev.is_message() && same_actor(&prev.from, &entry.from) && entry.ts - prev.ts < 300
    });
    let mut row = row()
        .key(format!("gp-{}", entry.id))
        .w_full()
        .items_start()
        .gap(10.0)
        .max_w(724.0);
    if !continuation {
        row = row.child(avatar(&entry.from, false, is_owner(&entry.from)));
    } else {
        row = row.child(rune::el().w(28.0));
    }
    let mut main = col().grow(1.0).min_w(0.0);
    if !continuation {
        main = main.child(message_head(entry));
    }
    main = if let Some(error) = &entry.error {
        main.child(
            text(error.clone())
                .font(rune::tok::TextStyle::new(11.5, 400.0))
                .px(10.0)
                .py(8.0)
                .radius(9.0)
                .border(1.0, rgba(251.0, 113.0, 133.0, 0.2))
                .border_layout()
                .fill(rgba(251.0, 113.0, 133.0, 0.07))
                .color(ink("#fda4af")),
        )
    } else {
        main.child(message_body::<S>(g, entry))
    };
    row.child(main)
}

fn mention_query(draft: &str) -> Option<String> {
    let tail = draft.rsplit('\n').next().unwrap_or("");
    let at = tail.rfind('@')?;
    if at > 0 {
        let before = tail[..at].chars().last();
        if before.is_some_and(|c| !c.is_whitespace()) {
            return None;
        }
    }
    Some(tail[at + 1..].to_lowercase())
}

struct TargetOption {
    value: String,
    label: String,
    hint: String,
}

pub fn chat_panel<S: ProjectsHost + 'static>(
    g: &GroupState,
    detail: &GroupDetail,
    compact: bool,
) -> El {
    let pad = if compact { 14.0 } else { 22.0 };
    let entries = chat_entries(Some(detail));
    let participants = participant_names(&entries, &detail.members);
    let status = if detail.project.status.is_empty() {
        "unknown".to_string()
    } else {
        detail.project.status.clone()
    };
    let waiting = detail.waiting();
    let approval_ids: HashSet<String> = entries
        .iter()
        .filter(|e| e.kind == "approval")
        .filter_map(|e| e.item_id.clone())
        .collect();
    let pending: Vec<&GroupItem> = waiting
        .iter()
        .copied()
        .filter(|item| !approval_ids.contains(&item.id))
        .collect();

    let filtered: Vec<&ChatEntry> = entries
        .iter()
        .filter(|entry| match g.filter.as_str() {
            "everyone" => true,
            "you" => is_owner(&entry.from) || is_owner(&entry.to),
            name => same_actor(&entry.from, name) || same_actor(&entry.to, name),
        })
        .collect();

    let mut counts: HashMap<String, usize> = HashMap::new();
    for entry in &entries {
        if entry.kind != "message" {
            continue;
        }
        for raw in [&entry.from, &entry.to] {
            let name = super::ui::raw_name(raw);
            if name.is_empty() {
                continue;
            }
            let lower = name.to_lowercase();
            if super::ui::NON_PARTICIPANTS.contains(&lower.as_str()) {
                continue;
            }
            *counts.entry(lower).or_insert(0) += 1;
        }
    }

    let mention = mention_query(&g.draft.text);
    let show_menu = g.menu != Menu::None || mention.is_some();
    let target_options: Vec<TargetOption> = {
        let query = mention.as_deref().unwrap_or("").to_string();
        let mut out = Vec::new();
        for (value, label, hint) in [
            ("all", "Everyone".to_string(), "The whole team"),
            (
                "supervisor",
                "Orchestrator".to_string(),
                "Runs the project board",
            ),
        ] {
            out.push(TargetOption {
                value: value.to_string(),
                label,
                hint: hint.to_string(),
            });
        }
        for name in &participants {
            out.push(TargetOption {
                value: name.clone(),
                label: actor_label(name),
                hint: "Project member".to_string(),
            });
        }
        out.retain(|option| query.is_empty() || option.label.to_lowercase().contains(&query));
        out
    };

    let top = col()
        .key("gp-chat-top")
        .w_full()
        .shrink()
        .fill(ink("#0e0e0e"))
        .child(
            row()
                .key("gp-chat-header")
                .w_full()
                .items_start()
                .justify_between()
                .gap(12.0)
                .px(pad)
                .pt(14.0)
                .pb(12.0)
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .child(
                            row()
                                .items_center()
                                .gap(6.0)
                                .child(icon(
                                    12.0,
                                    crate::icons::message_square(12.0, ink("#a1a1aa")),
                                ))
                                .child(
                                    text("GROUP CONVERSATION".to_string())
                                        .font({
                                            let mut style = rune::tok::TextStyle::new(10.0, 700.0);
                                            style.line_height = 15.0;
                                            style.tracking = 1.2;
                                            style
                                        })
                                        .color(ink("#a1a1aa")),
                                ),
                        )
                        .child(
                            text(if detail.messages.is_empty() && !entries.is_empty() {
                                "Activity from the project event log".to_string()
                            } else {
                                "Messages from Stark and the project members".to_string()
                            })
                            .font(rune::tok::TextStyle::new(10.5, 400.0))
                            .color(ink("#52525b"))
                            .mt(3.0),
                        ),
                )
                .child(
                    row()
                        .key("gp-chat-header-side")
                        .items_center()
                        .justify_end()
                        .flex_wrap()
                        .gap(6.0)
                        .child(if waiting.is_empty() {
                            rune::el().size(0.0, 0.0).inert()
                        } else {
                            row()
                                .items_center()
                                .gap(4.0)
                                .px(7.0)
                                .py(2.0)
                                .radius(9.0)
                                .fill(rgba(251.0, 191.0, 36.0, 0.12))
                                .child(icon(
                                    11.0,
                                    crate::icons::triangle_alert(11.0, ink("#fcd34d")),
                                ))
                                .child(
                                    text(format!("{} waiting", waiting.len()))
                                        .font(rune::tok::TextStyle::new(9.5, 650.0))
                                        .color(ink("#fcd34d")),
                                )
                        })
                        .child(live_chip(&status)),
                ),
        )
        .child(
            row()
                .key("gp-participants")
                .w_full()
                .items_center()
                .flex_wrap()
                .gap(4.0)
                .px(pad)
                .pb(16.0)
                .child(
                    filter_button(
                        "Everyone",
                        g.filter == "everyone",
                        None,
                        None,
                        Some(crate::icons::users(
                            11.0,
                            if g.filter == "everyone" {
                                ink("#f4f4f5")
                            } else {
                                ink("#71717a")
                            },
                        )),
                    )
                    .on_press(|s: &mut S| {
                        s.projects_mut().group.filter = "everyone".to_string();
                    }),
                )
                .child(
                    filter_button("You", g.filter == "you", Some("owner"), None, None).on_press(
                        |s: &mut S| {
                            s.projects_mut().group.filter = "you".to_string();
                        },
                    ),
                )
                .children(participants.iter().map(|name| {
                    let count = counts
                        .get(&super::ui::raw_name(name).to_lowercase())
                        .copied();
                    filter_button(
                        &actor_label(name),
                        g.filter == *name || same_actor(&g.filter, name),
                        Some(name),
                        count,
                        None,
                    )
                    .on_press({
                        let name = name.clone();
                        move |s: &mut S| {
                            s.projects_mut().group.filter = name.clone();
                        }
                    })
                })),
        );

    let mut scroll_list = col()
        .key("gp-chat-list")
        .w_full()
        .gap(11.0)
        .px(pad)
        .pt(14.0)
        .pb(28.0);
    let show_empty = filtered.is_empty() && pending.is_empty();
    if show_empty {
        let title = if g.filter == "you" {
            "No messages from you yet".to_string()
        } else {
            "No messages yet".to_string()
        };
        let copy = if g.filter == "you" {
            "Your messages to the team will appear here.".to_string()
        } else if g.filter == "everyone" {
            "Assignments, reports, and teammate messages will appear here.".to_string()
        } else {
            format!(
                "Messages involving {} will appear here.",
                actor_label(&g.filter)
            )
        };
        scroll_list = scroll_list.child(
            col()
                .key("gp-chat-empty")
                .w_full()
                .h(360.0)
                .center()
                .gap(9.0)
                .child(
                    rune::el()
                        .size(44.0, 44.0)
                        .radius(13.0)
                        .border(1.0, rgba(255.0, 255.0, 255.0, 0.08))
                        .border_layout()
                        .fill(rgba(255.0, 255.0, 255.0, 0.035))
                        .center()
                        .child(icon(20.0, crate::icons::sparkles(20.0, ink("#a1a1aa")))),
                )
                .child(
                    text(title)
                        .font(rune::tok::TextStyle::new(14.0, 600.0))
                        .color(ink("#e4e4e7")),
                )
                .child(
                    text(copy)
                        .max_w(330.0)
                        .font({
                            let mut style = rune::tok::TextStyle::new(11.5, 400.0);
                            style.line_height = 17.8;
                            style
                        })
                        .color(ink("#71717a")),
                ),
        );
    } else {
        for (index, entry) in filtered.iter().enumerate() {
            if entry.kind == "activity" {
                scroll_list = scroll_list.child(
                    row()
                        .key(format!("gp-{}", entry.id))
                        .w_full()
                        .items_center()
                        .justify_center()
                        .gap(8.0)
                        .child(
                            text(entry.body.clone())
                                .font(rune::tok::TextStyle::new(10.0, 400.0))
                                .color(ink("#52525b")),
                        )
                        .child(
                            text(short_time(entry.ts))
                                .font(rune::tok::TextStyle::new(10.0, 400.0))
                                .color(ink("#3f3f46")),
                        ),
                );
                continue;
            }
            if entry.kind == "approval" {
                let item = entry
                    .item_id
                    .as_deref()
                    .and_then(|id| detail.item(id))
                    .filter(|item| item.awaiting_approval());
                scroll_list = scroll_list.child(if let Some(item) = item {
                    approval_card::<S>(item, g.busy_item.as_deref() == Some(item.id.as_str()))
                } else {
                    let supervisor = is_supervisor(&entry.from);
                    row()
                        .key(format!("gp-{}", entry.id))
                        .w_full()
                        .items_start()
                        .gap(9.0)
                        .padding(11.0)
                        .radius(11.0)
                        .border(1.0, rgba(251.0, 191.0, 36.0, 0.2))
                        .border_layout()
                        .fill(rgba(251.0, 191.0, 36.0, 0.055))
                        .child(avatar(&entry.from, true, is_owner(&entry.from)))
                        .child(
                            col()
                                .grow(1.0)
                                .min_w(0.0)
                                .child(message_head(entry))
                                .child(message_body::<S>(g, entry))
                                .child(if supervisor {
                                    rune::el().size(0.0, 0.0).inert()
                                } else {
                                    text(short_time(entry.ts))
                                        .font(rune::tok::TextStyle::new(9.0, 400.0))
                                        .color(ink("#4f525a"))
                                }),
                        )
                });
                continue;
            }
            let previous = index.checked_sub(1).and_then(|i| filtered.get(i)).copied();
            scroll_list = scroll_list.child(message_entry::<S>(g, entry, previous));
        }
        for item in pending {
            scroll_list = scroll_list.child(approval_card::<S>(
                item,
                g.busy_item.as_deref() == Some(item.id.as_str()),
            ));
        }
    }

    let scroll = col()
        .key("gp-chat-scroll")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(rune::ScrollSpec { stick_bottom: true })
        .child(scroll_list);

    let mut composer_wrap = col()
        .key("gp-composer-wrap")
        .w_full()
        .shrink()
        .gap(0.0)
        .px(14.0)
        .pt(8.0)
        .pb(10.0)
        .border_sides([0.0, 1.0, 0.0, 0.0], rgba(255.0, 255.0, 255.0, 0.06))
        .border_layout()
        .fill(ink("#0e0e0e"));
    if let Some(error) = &g.send_error {
        composer_wrap = composer_wrap.child(
            text(error.clone())
                .mb(6.0)
                .px(8.0)
                .py(6.0)
                .radius(7.0)
                .border(1.0, rgba(251.0, 113.0, 133.0, 0.18))
                .border_layout()
                .fill(rgba(251.0, 113.0, 133.0, 0.07))
                .font(rune::tok::TextStyle::new(10.0, 400.0))
                .color(ink("#fda4af")),
        );
    }
    if show_menu {
        let mut menu = col()
            .key("gp-target-menu")
            .w(262.0)
            .max_h(270.0)
            .gap(1.0)
            .padding(4.0)
            .mb(6.0)
            .radius(11.0)
            .border(1.0, rgba(255.0, 255.0, 255.0, 0.09))
            .border_layout()
            .fill(ink("#151515"))
            .shadow(
                0.0,
                18.0,
                rune::Color {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.5,
                },
            )
            .transition(crate::motion::menu_enter());
        menu = menu.child(
            row()
                .items_center()
                .gap(5.0)
                .px(7.0)
                .pt(5.0)
                .pb(4.0)
                .child(icon(
                    11.0,
                    if mention.is_some() {
                        crate::icons::at_sign(11.0, ink("#6f727b"))
                    } else {
                        crate::icons::users(11.0, ink("#6f727b"))
                    },
                ))
                .child(
                    text(if mention.is_some() {
                        "Mention an agent".to_string()
                    } else {
                        "Send to".to_string()
                    })
                    .font({
                        let mut style = rune::tok::TextStyle::new(8.0, 700.0);
                        style.line_height = 12.0;
                        style.tracking = 0.8;
                        style
                    })
                    .color(ink("#6f727b")),
                ),
        );
        if target_options.is_empty() {
            menu = menu.child(
                text("No matching agents.".to_string())
                    .padding(8.0)
                    .font(rune::tok::TextStyle::new(10.0, 400.0))
                    .color(ink("#666a74")),
            );
        }
        for option in target_options {
            let value = option.value.clone();
            let active = g.target == option.value;
            menu = menu.child(
                row()
                    .w_full()
                    .items_center()
                    .gap(7.0)
                    .px(7.0)
                    .py(6.0)
                    .radius(7.0)
                    .hover(rgba(255.0, 255.0, 255.0, 0.06))
                    .role("menuitem")
                    .label(format!("Send to {}", option.label))
                    .on_press({
                        let value = value.clone();
                        move |s: &mut S| {
                            let g = &mut s.projects_mut().group;
                            g.target = value.clone();
                            g.menu = Menu::None;
                            if let Some(at) = g.draft.text.rfind('@') {
                                g.draft.text.truncate(at);
                                g.draft.text = g.draft.text.trim_end().to_string();
                            }
                        }
                    })
                    .child(
                        rune::el()
                            .size(20.0, 20.0)
                            .radius(6.0)
                            .border(1.0, rgba(255.0, 255.0, 255.0, 0.08))
                            .border_layout()
                            .fill(rgba(255.0, 255.0, 255.0, 0.05))
                            .center()
                            .child(
                                text(initials(if option.value == "all" {
                                    "Everyone"
                                } else if option.value == "supervisor" {
                                    "supervisor"
                                } else {
                                    &option.value
                                }))
                                .font(rune::tok::TextStyle::new(7.0, 750.0))
                                .color(ink("#d4d4d8")),
                            ),
                    )
                    .child(
                        col()
                            .grow(1.0)
                            .min_w(0.0)
                            .gap(1.0)
                            .child(
                                text(option.label.clone())
                                    .font(rune::tok::TextStyle::new(10.5, 600.0))
                                    .color(ink("#e4e4e8")),
                            )
                            .child(
                                text(option.hint.clone())
                                    .font(rune::tok::TextStyle::new(8.5, 400.0))
                                    .color(ink("#666a74")),
                            ),
                    )
                    .child(if active {
                        icon(12.0, crate::lucide_check(12.0, 2.0, ink("#34d399")))
                    } else {
                        rune::el().size(0.0, 0.0).inert()
                    }),
            );
        }
        composer_wrap = composer_wrap.child(menu);
    }

    let send_enabled = !g.draft.text.trim().is_empty() && !g.sending;
    let target_value = g.target.clone();
    let mut composer = row()
        .key("gp-composer")
        .w_full()
        .items_end()
        .gap(6.0)
        .padding(5.0)
        .radius(12.0)
        .border(1.0, rgba(255.0, 255.0, 255.0, 0.09))
        .border_layout()
        .fill(ink("#141414"))
        .child(
            row()
                .items_center()
                .gap(4.0)
                .h(26.0)
                .px(6.0)
                .radius(7.0)
                .border(1.0, rgba(255.0, 255.0, 255.0, 0.07))
                .border_layout()
                .fill(rgba(255.0, 255.0, 255.0, 0.035))
                .role("button")
                .label("Choose message target")
                .on_press(move |s: &mut S| {
                    let g = &mut s.projects_mut().group;
                    g.menu = if g.menu == Menu::Target {
                        Menu::None
                    } else {
                        Menu::Target
                    };
                })
                .child(
                    text("to".to_string())
                        .font(rune::tok::TextStyle::new(9.5, 400.0))
                        .color(ink("#666a74")),
                )
                .child(
                    text(target_label(&target_value))
                        .font(rune::tok::TextStyle::new(9.5, 400.0))
                        .color(ink("#c6c8cf")),
                )
                .child(icon(11.0, crate::icons::chevron_down(11.0, ink("#666a74")))),
        )
        .child(
            row()
                .grow(1.0)
                .min_w(0.0)
                .items_end()
                .gap(2.0)
                .child(
                    rune::el()
                        .key("gp-composer-at")
                        .size(25.0, 26.0)
                        .radius(6.0)
                        .center()
                        .hover(ink("#ffffff0f"))
                        .role("button")
                        .label("Mention an agent")
                        .on_press(|s: &mut S| {
                            s.projects_mut().group.menu = Menu::Mention;
                        })
                        .child(icon(13.0, crate::icons::at_sign(13.0, ink("#6f727b")))),
                )
                .child(
                    rune::text_input(&g.draft, {
                        let mut style = rune::tok::TextStyle::new(11.5, 400.0);
                        style.line_height = 16.7;
                        style
                    })
                    .key("gp-composer-input")
                    .grow(1.0)
                    .min_w(0.0)
                    .min_h(26.0)
                    .max_h(120.0)
                    .px(2.0)
                    .py(5.0)
                    .placeholder("Message the agents — @ to name one", ink("#5f626b"))
                    .caret(ink("#f2f2f4"), ink("#3b82f6"))
                    .on_edit(move |s: &mut S| &mut s.projects_mut().group.draft)
                    .on_submit(|s: &mut S, _tx: &mut rune::Tx| {
                        s.projects_mut().group.send();
                    }),
                ),
        )
        .child(
            rune::el()
                .key("gp-composer-send")
                .size(26.0, 26.0)
                .radius(7.0)
                .center()
                .fill(if send_enabled {
                    ink("#f1f1f3")
                } else {
                    rgba(255.0, 255.0, 255.0, 0.08)
                })
                .role("button")
                .label("Send message")
                .on_press(move |s: &mut S| {
                    s.projects_mut().group.send();
                })
                .child(icon(
                    14.0,
                    crate::lucide_arrow_up(
                        14.0,
                        2.0,
                        if send_enabled {
                            ink("#0a0a0d")
                        } else {
                            ink("#5f626b")
                        },
                    ),
                )),
        );
    if !send_enabled {
        composer = composer.opacity(1.0);
    }
    composer_wrap = composer_wrap.child(composer).child(
        row()
            .w_full()
            .items_center()
            .gap(5.0)
            .pt(5.0)
            .px(2.0)
            .child(icon(10.0, crate::icons::bot(10.0, ink("#4f525a"))))
            .child(
                text("Messages reach the team on its next turn; replies land here.".to_string())
                    .font(rune::tok::TextStyle::new(9.0, 400.0))
                    .color(ink("#4f525a")),
            ),
    );

    col()
        .key("gp-chat")
        .grow(1.0)
        .min_w(0.0)
        .fill(ink("#0e0e0e"))
        .child(top)
        .child(scroll)
        .child(composer_wrap)
}
