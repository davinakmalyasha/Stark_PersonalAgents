//! `GroupProjectModal`: overlay, header, skeleton and the chat/rail split.

use rune::{col, row, text, Easing, El, LoopSpec};

use super::data::{GroupDetail, ProjectInfo};
use super::ui::status_chip;
use super::GroupState;
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

fn icon_box(size: f32, glyph: El) -> El {
    rune::el()
        .size(size, size)
        .radius(10.0)
        .border(1.0, rgba(255.0, 255.0, 255.0, 0.09))
        .border_layout()
        .fill(rgba(255.0, 255.0, 255.0, 0.045))
        .center()
        .inert()
        .child(glyph)
}

fn shimmer(w: f32, h: f32, radius: f32) -> El {
    let w = w.max(8.0);
    rune::el()
        .w(w)
        .h(h)
        .radius(radius)
        .clip_rounded(radius)
        .fill(rgba(255.0, 255.0, 255.0, 0.045))
        .inert()
        .child(
            rune::el()
                .absolute(-(w * 0.8), 0.0)
                .w(w * 0.8)
                .h_full()
                .linear_gradient(
                    0.0,
                    [
                        (0.0, rgba(255.0, 255.0, 255.0, 0.0)),
                        (0.5, rgba(255.0, 255.0, 255.0, 0.085)),
                        (1.0, rgba(255.0, 255.0, 255.0, 0.0)),
                    ],
                )
                .looping(LoopSpec::slide(1.4, [-1.0, 0.0], [2.0, 0.0]).easing(Easing::LINEAR)),
        )
}

fn skeleton(w: f32) -> El {
    col()
        .key("gp-skeleton")
        .w_full()
        .gap(12.0)
        .child(shimmer(w * 2.0 / 3.0, 16.0, 4.0))
        .child(shimmer(w * 0.5, 12.0, 3.0))
        .child(
            col()
                .w_full()
                .gap(8.0)
                .mt(24.0)
                .child(shimmer(w / 3.0, 12.0, 3.0))
                .child(shimmer(w, 36.0, 6.0))
                .child(shimmer(w, 36.0, 6.0))
                .child(shimmer(w * 0.8, 36.0, 6.0)),
        )
}

fn shell_badge() -> El {
    row()
        .key("gp-shell-badge")
        .help("Members can run shell commands")
        .items_center()
        .gap(3.0)
        .px(6.0)
        .py(1.0)
        .radius(5.0)
        .border(1.0, rgba(255.0, 255.0, 255.0, 0.08))
        .border_layout()
        .child(icon(9.0, crate::icons::terminal(9.0, ink("#a1a1aa"))))
        .child(
            text("shell".to_string())
                .font(rune::tok::TextStyle::new(9.0, 500.0))
                .color(ink("#a1a1aa")),
        )
}

fn action_button(label: &str, primary: bool, glyph: Option<rune::Shape>) -> El {
    let mut style = rune::tok::TextStyle::new(11.0, 550.0);
    style.line_height = 16.5;
    let fg = if primary {
        ink("#0a0a0d")
    } else {
        ink("#d4d4d8")
    };
    let mut content = row().items_center().gap(5.0);
    if let Some(shape) = glyph {
        content = content.child(icon(12.0, shape));
    }
    content = content.child(text(label.to_string()).font(style).color(fg));
    let mut el = content
        .h(30.0)
        .px(10.0)
        .radius(8.0)
        .center()
        .role("button")
        .label(label.to_string())
        .focusable(true)
        .focus_ring(2.0, 2.0, rgba(255.0, 255.0, 255.0, 0.55));
    if primary {
        el = el.fill(ink("#ffffff")).color(ink("#0a0a0d"));
    } else {
        el = el
            .fill(rgba(255.0, 255.0, 255.0, 0.035))
            .border(1.0, rgba(255.0, 255.0, 255.0, 0.1))
            .border_layout();
    }
    el
}

fn icon_action(key: &str, label: &str, shape: rune::Shape) -> El {
    rune::el()
        .key(key)
        .size(30.0, 30.0)
        .radius(8.0)
        .center()
        .hover(ink("#ffffff0f"))
        .role("button")
        .label(label)
        .help(label.to_string())
        .focusable(true)
        .focus_ring(2.0, 2.0, rgba(255.0, 255.0, 255.0, 0.55))
        .child(icon(15.0, shape))
}

fn header<S: ProjectsHost + 'static>(
    g: &GroupState,
    project: &ProjectInfo,
    detail: Option<&GroupDetail>,
    compact: bool,
    rail_visible: bool,
) -> El {
    let status = if project.status.is_empty() {
        "unknown".to_string()
    } else {
        project.status.clone()
    };
    let items = detail.map(|d| d.items.len()).unwrap_or(0);
    let milestones = detail.map(|d| d.milestones.len()).unwrap_or(0);
    let items_done = detail.map(GroupDetail::items_done).unwrap_or(0);
    let milestones_done = detail.map(GroupDetail::milestones_done).unwrap_or(0);
    let waiting = detail.map(|d| d.waiting().len()).unwrap_or(0);
    let project_id = g.open_id.clone().unwrap_or_default();
    let title = if project.goal.is_empty() {
        project_id.clone()
    } else {
        project.goal.clone()
    };
    let pause_id = g.open_id.clone().unwrap_or_default();
    let close_id = g.open_id.clone().unwrap_or_default();
    let folder_path = detail.map(|d| d.path.clone()).unwrap_or_default();

    let mut actions = row()
        .key("gp-modal-actions")
        .items_center()
        .justify_end()
        .gap(6.0)
        .flex_wrap();
    if compact {
        // `.gp-rail-toggle` is the mobile-only details switch; desktop keeps
        // the rail mounted and hides the toggle.
        actions = actions
            .child(
                icon_action(
                    "gp-rail-toggle",
                    if rail_visible {
                        "Hide details"
                    } else {
                        "Show details"
                    },
                    if rail_visible {
                        crate::icons::panel_right_close(15.0, ink("#71717a"))
                    } else {
                        crate::icons::panel_right_open(15.0, ink("#71717a"))
                    },
                )
                .on_press({
                    let id = pause_id.clone();
                    move |s: &mut S| {
                        let _ = id;
                        s.projects_mut().group.show_rail = !s.projects_mut().group.show_rail;
                    }
                }),
            )
            .child(rune::spacer());
    }
    actions = actions
        .child(
            icon_action(
                "gp-notes",
                "Open sticky notes",
                crate::icons::sticky_note(14.0, ink("#71717a")),
            )
            .on_press({
                let id = pause_id.clone();
                move |s: &mut S| {
                    let g = &mut s.projects_mut().group;
                    crate::notes::open_notes(&mut g.notes, "group", &id);
                }
            }),
        )
        .child(
            icon_action(
                "gp-folder",
                "Open folder",
                crate::icons::folder_open(15.0, ink("#71717a")),
            )
            .on_press_tx({
                let path = folder_path.clone();
                move |s: &mut S, tx: &mut rune::Tx| {
                    let _ = &tx;
                    let note = if path.is_empty() {
                        "No project folder.".to_string()
                    } else {
                        #[cfg(target_os = "windows")]
                        {
                            let _ = std::process::Command::new("explorer").arg(&path).spawn();
                            "Opened folder.".to_string()
                        }
                        #[cfg(not(target_os = "windows"))]
                        {
                            tx.copy(path.clone());
                            "Path copied.".to_string()
                        }
                    };
                    s.projects_mut().group.set_path_note(note);
                }
            }),
        );
    if status == "running" {
        actions = actions.child(
            action_button(
                "Pause",
                false,
                Some(crate::icons::pause(12.0, ink("#d4d4d8"))),
            )
            .key("gp-pause")
            .on_press(move |s: &mut S| {
                let _ = &pause_id;
                s.projects_mut().group.op("pause");
            }),
        );
    } else if status == "paused" {
        actions = actions.child(
            action_button(
                "Resume",
                true,
                Some(crate::icons::play(12.0, ink("#0a0a0d"))),
            )
            .key("gp-resume")
            .on_press(move |s: &mut S| {
                s.projects_mut().group.op("resume");
            }),
        );
    }
    if status != "done" && status != "closed" {
        actions = actions.child(
            action_button("Close", false, None)
                .key("gp-close-project")
                .on_press(move |s: &mut S| {
                    let _ = &close_id;
                    s.projects_mut().group.op("close");
                }),
        );
    }
    actions = actions.child(
        icon_action(
            "gp-close-x",
            "Close",
            crate::lucide_x(16.0, 2.0, ink("#71717a")),
        )
        .on_press(|s: &mut S| {
            s.projects_mut().group.close();
        }),
    );

    let mut subline = row()
        .key("gp-modal-subline")
        .items_center()
        .flex_wrap()
        .gap(6.0)
        .mt(4.0)
        .child(
            text(project_id.clone())
                .font(rune::tok::TextStyle::new(10.5, 400.0))
                .color(ink("#71717a")),
        )
        .child(text("·".to_string()).color(ink("#3f3f46")))
        .child(
            text(format!("{milestones_done}/{milestones} milestones"))
                .font(rune::tok::TextStyle::new(10.5, 400.0))
                .color(ink("#71717a")),
        )
        .child(text("·".to_string()).color(ink("#3f3f46")))
        .child(
            text(format!("{items_done}/{items} items"))
                .font(rune::tok::TextStyle::new(10.5, 400.0))
                .color(ink("#71717a")),
        );
    if waiting > 0 {
        subline = subline.child(
            text(format!("{waiting} waiting on you"))
                .font(crate::motion::tabular(rune::tok::TextStyle::new(
                    10.5, 650.0,
                )))
                .color(ink("#fbbf24")),
        );
    }

    let identity = row()
        .key("gp-modal-identity")
        .grow(1.0)
        .min_w(0.0)
        .items_start()
        .gap(11.0)
        .child(icon_box(
            36.0,
            icon(18.0, crate::icons::folder_kanban(18.0, ink("#a1a1aa"))),
        ))
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .child(
                    row()
                        .items_center()
                        .flex_wrap()
                        .gap(8.0)
                        .child(
                            text(title.clone())
                                .min_w(0.0)
                                .truncate()
                                .font({
                                    let mut style = rune::tok::TextStyle::new(15.0, 650.0);
                                    style.line_height = 22.5;
                                    style.tracking = -0.15;
                                    style
                                })
                                .color(ink("#f4f4f5")),
                        )
                        .child(status_chip(&status, false))
                        .child(if detail.is_some_and(|d| d.shell_enabled) {
                            shell_badge()
                        } else {
                            rune::el().size(0.0, 0.0).inert()
                        }),
                )
                .child(subline),
        );

    if compact {
        col()
            .key("gp-modal-header")
            .w_full()
            .gap(10.0)
            .px(13.0)
            .pt(12.0)
            .pb(12.0)
            .border_sides([0.0, 0.0, 1.0, 0.0], rgba(255.0, 255.0, 255.0, 0.07))
            .border_layout()
            .child(identity.w_full())
            .child(actions.w_full())
    } else {
        row()
            .key("gp-modal-header")
            .w_full()
            .items_start()
            .gap(18.0)
            .px(18.0)
            .pt(16.0)
            .pb(14.0)
            .border_sides([0.0, 0.0, 1.0, 0.0], rgba(255.0, 255.0, 255.0, 0.07))
            .border_layout()
            .child(identity)
            .child(actions)
    }
}

pub fn group_modal<S: ProjectsHost + 'static>(g: &GroupState, w: f32, h: f32) -> El {
    let compact = w <= 900.0;
    let panel_w = if compact {
        (w - 24.0).max(320.0)
    } else {
        (w - 32.0).clamp(320.0, 1180.0)
    };
    let panel_h = if compact {
        (h - 20.0).max(280.0)
    } else {
        (h - 32.0).clamp(280.0, 860.0)
    };
    let rail_visible = if compact { g.show_rail } else { true };
    let detail = g.detail.as_ref();
    let project = detail.map(|d| d.project.clone()).unwrap_or_default();
    let content_w = (panel_w - 48.0).max(200.0);

    let mut body = col().key("gp-layout").w_full().grow(1.0).min_h(0.0);
    if g.loading && detail.is_none() {
        body = body
            .grow(1.0)
            .min_h(0.0)
            .px(24.0)
            .py(24.0)
            .child(skeleton(content_w));
    } else if let Some(error) = &g.error {
        if detail.is_none() {
            body = body.grow(1.0).min_h(0.0).px(24.0).py(24.0).child(
                text(error.clone())
                    .font(rune::tok::TextStyle::new(13.0, 400.0))
                    .color(ink("#f87171")),
            );
        }
    }
    if let Some(detail) = detail {
        if compact && rail_visible {
            body = body.child(super::rail::rail_panel::<S>(g, detail, panel_w, true));
        } else if compact {
            body = body.child(super::chat::chat_panel::<S>(g, detail, true));
        } else {
            body = body.child(
                row()
                    .key("gp-layout-body")
                    .w_full()
                    .grow(1.0)
                    .min_h(0.0)
                    .child(super::chat::chat_panel::<S>(g, detail, false))
                    .child(super::rail::rail_panel::<S>(g, detail, panel_w, false)),
            );
        }
    }

    let mut notice = row()
        .key("gp-modal-notice")
        .w_full()
        .items_center()
        .flex_wrap()
        .gap(12.0)
        .px(18.0)
        .py(7.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], rgba(255.0, 255.0, 255.0, 0.06))
        .border_layout()
        .fill(rgba(255.0, 255.0, 255.0, 0.018));
    let mut has_notice = false;
    if let Some(error) = &g.action_error {
        has_notice = true;
        notice = notice.child(
            row()
                .items_center()
                .gap(5.0)
                .child(icon(
                    12.0,
                    crate::icons::triangle_alert(12.0, ink("#fda4af")),
                ))
                .child(
                    text(error.clone())
                        .font(rune::tok::TextStyle::new(10.5, 400.0))
                        .color(ink("#fda4af")),
                ),
        );
    }
    if let Some(queued) = &g.queued {
        has_notice = true;
        notice = notice.child(
            row()
                .items_center()
                .gap(5.0)
                .child(icon(12.0, crate::icons::circle_check(12.0, ink("#a1a1aa"))))
                .child(
                    text(queued.clone())
                        .font(rune::tok::TextStyle::new(10.5, 400.0))
                        .color(ink("#a1a1aa")),
                ),
        );
    }
    if let Some(note) = &g.path_note {
        has_notice = true;
        notice = notice.child(
            text(note.clone())
                .font(rune::tok::TextStyle::new(10.5, 400.0))
                .color(ink("#a1a1aa")),
        );
    }
    if let Some(runtime) = detail.and_then(|d| d.runtime_notice.as_ref()) {
        has_notice = true;
        notice = notice.child(
            row()
                .items_center()
                .gap(5.0)
                .child(icon(12.0, crate::icons::info(12.0, ink("#a1a1aa"))))
                .child(
                    text(runtime.clone())
                        .font(rune::tok::TextStyle::new(10.5, 400.0))
                        .color(ink("#a1a1aa")),
                ),
        );
    }

    let mut card = col()
        .key("gp-modal")
        .w(panel_w)
        .h(panel_h)
        .radius(16.0)
        .fill(ink("#111114"))
        .border(1.0, ink("#27272a"))
        .border_layout()
        .clip_rounded(16.0)
        // Tailwind `shadow-2xl`: 0 25px 50px -12px rgb(0 0 0 / .25).
        .box_shadow(rune::Shadow {
            x: 0.0,
            y: 25.0,
            blur: 50.0,
            spread: -12.0,
            color: rgba(0.0, 0.0, 0.0, 0.25),
            inset: false,
        })
        .transition(crate::motion::modal_enter())
        .hover_motion_within(crate::motion::HOVER, crate::motion::HOVER)
        .child(header::<S>(g, &project, detail, compact, rail_visible));
    if has_notice {
        card = card.child(notice);
    }
    card = card.child(body);

    let backdrop = rune::el()
        .key("gp-modal-backdrop")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(rgba(0.0, 0.0, 0.0, 0.65))
        .backdrop_blur(4.0)
        .center()
        .modal(true, |s: &mut S| {
            s.projects_mut().group.close();
        })
        .transition(crate::motion::backdrop_enter())
        .child(card);
    if g.notes.open {
        backdrop.child(super::notes::group_notes_sheet::<S>(
            &g.notes,
            g.open_id.clone().unwrap_or_default(),
        ))
    } else {
        backdrop
    }
}
