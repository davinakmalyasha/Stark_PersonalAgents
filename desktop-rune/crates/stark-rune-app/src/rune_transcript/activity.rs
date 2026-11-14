//! `ActivityTimeline.tsx` + `TraceAccordion.tsx` for the home transcript.
//!
//! One visual language for reasoning, tool calls and intermediate updates.
//! The DOM/CSS values are ported literally from `index.css`; geometry follows
//! the same flex/gap/padding rules as the React components.

use rune::{col, row, text, El, TextStyle};

use super::{home_icon_copy, home_ink, home_rgba, md_render, RowData, Thread, TranscriptHost};
use crate::stream_accum::{
    LiveToolEvent, RetryNotice, RunStatusNotice, StreamSegment, ToolStatus, TraceStep,
};
use crate::tool_labels;

fn style(size: f32, weight: f32) -> TextStyle {
    let mut s = TextStyle::new(size, weight);
    s.line_height = size * 1.45;
    s
}

fn mono(size: f32, weight: f32) -> TextStyle {
    let mut s = TextStyle::new(size, weight).mono();
    s.line_height = size * 1.5;
    s
}

fn icon_box(size: f32, shape: rune::Shape) -> El {
    rune::el().size(size, size).inert().shape(shape)
}

/// Lucide `Loader2` as a three-quarter arc.
fn loader(size: f32, color: rune::Color) -> rune::Shape {
    const K: f32 = 4.971;
    let p = rune::Path::new()
        .move_to([12.0, 3.0])
        .cubic_to([12.0 + K, 3.0], [21.0, 12.0 - K], [21.0, 12.0])
        .cubic_to([21.0, 12.0 + K], [12.0 + K, 21.0], [12.0, 21.0])
        .cubic_to([12.0 - K, 21.0], [3.0, 12.0 + K], [3.0, 12.0]);
    rune::Shape::new()
        .stroke_path(&p, crate::lucide_stroke(size), rune::LineCap::Round, color)
        .view_box(24.0, 24.0)
}

fn spinner(size: f32, color: rune::Color) -> El {
    icon_box(size, loader(size, color)).looping(rune::LoopSpec::rotation(0.9))
}

fn check_circle(size: f32, color: rune::Color) -> rune::Shape {
    const K: f32 = 5.0;
    let circle = rune::Path::new()
        .move_to([12.0, 2.0])
        .cubic_to([12.0 + K, 2.0], [22.0, 12.0 - K], [22.0, 12.0])
        .cubic_to([22.0, 12.0 + K], [12.0 + K, 22.0], [12.0, 22.0])
        .cubic_to([12.0 - K, 22.0], [2.0, 12.0 + K], [2.0, 12.0])
        .cubic_to([2.0, 12.0 - K], [12.0 - K, 2.0], [12.0, 2.0])
        .close()
        .move_to([8.0, 12.0])
        .line_to([11.0, 15.0])
        .line_to([16.0, 9.0]);
    rune::Shape::new()
        .stroke_path(
            &circle,
            crate::lucide_stroke(size),
            rune::LineCap::Round,
            color,
        )
        .view_box(24.0, 24.0)
}

fn chevron(size: f32, down: bool, color: rune::Color) -> rune::Shape {
    let p = if down {
        rune::Path::new()
            .move_to([6.0, 9.0])
            .line_to([12.0, 15.0])
            .line_to([18.0, 9.0])
    } else {
        rune::Path::new()
            .move_to([9.0, 18.0])
            .line_to([15.0, 12.0])
            .line_to([9.0, 6.0])
    };
    rune::Shape::new()
        .stroke_path(&p, crate::lucide_stroke(size), rune::LineCap::Round, color)
        .view_box(24.0, 24.0)
}

fn tool_icon(kind: tool_labels::IconKind, size: f32, color: rune::Color) -> rune::Shape {
    match kind {
        tool_labels::IconKind::Terminal => {
            let p = rune::Path::new()
                .move_to([4.0, 17.0])
                .line_to([10.0, 11.0])
                .line_to([4.0, 5.0])
                .move_to([12.0, 19.0])
                .line_to([20.0, 19.0]);
            rune::Shape::new()
                .stroke_path(&p, crate::lucide_stroke(size), rune::LineCap::Round, color)
                .view_box(24.0, 24.0)
        }
        tool_labels::IconKind::Search => {
            const K: f32 = 4.14;
            let p = rune::Path::new()
                .move_to([11.0, 3.0])
                .cubic_to([11.0 + K, 3.0], [19.0, 11.0 - K], [19.0, 11.0])
                .cubic_to([19.0, 11.0 + K], [11.0 + K, 19.0], [11.0, 19.0])
                .cubic_to([11.0 - K, 19.0], [3.0, 11.0 + K], [3.0, 11.0])
                .cubic_to([3.0, 11.0 - K], [11.0 - K, 3.0], [11.0, 3.0])
                .close()
                .move_to([21.0, 21.0])
                .line_to([16.65, 16.65]);
            rune::Shape::new()
                .stroke_path(&p, crate::lucide_stroke(size), rune::LineCap::Round, color)
                .view_box(24.0, 24.0)
        }
        tool_labels::IconKind::File => {
            let p = rune::Path::new()
                .move_to([14.0, 2.0])
                .line_to([6.0, 2.0])
                .cubic_to([4.9, 2.0], [4.0, 2.9], [4.0, 4.0])
                .line_to([4.0, 20.0])
                .cubic_to([4.0, 21.1], [4.9, 22.0], [6.0, 22.0])
                .line_to([18.0, 22.0])
                .cubic_to([19.1, 22.0], [20.0, 21.1], [20.0, 20.0])
                .line_to([20.0, 8.0])
                .close()
                .move_to([14.0, 2.0])
                .line_to([14.0, 8.0])
                .line_to([20.0, 8.0])
                .move_to([16.0, 13.0])
                .line_to([8.0, 13.0])
                .move_to([16.0, 17.0])
                .line_to([8.0, 17.0]);
            rune::Shape::new()
                .stroke_path(&p, crate::lucide_stroke(size), rune::LineCap::Round, color)
                .view_box(24.0, 24.0)
        }
        tool_labels::IconKind::Wrench => {
            let p = rune::Path::new()
                .move_to([14.7, 6.3])
                .cubic_to([14.0, 5.6], [14.0, 4.4], [14.7, 3.7])
                .line_to([17.0, 1.4])
                .cubic_to([18.9, 2.3], [20.2, 4.0], [20.8, 6.0])
                .cubic_to([21.5, 8.0], [21.1, 10.0], [19.9, 11.6])
                .line_to([13.0, 18.5])
                .cubic_to([11.6, 19.9], [9.4, 19.9], [8.0, 18.5])
                .cubic_to([6.6, 17.1], [6.6, 14.9], [8.0, 13.5])
                .line_to([14.9, 6.6]);
            rune::Shape::new()
                .stroke_path(&p, crate::lucide_stroke(size), rune::LineCap::Round, color)
                .view_box(24.0, 24.0)
        }
    }
}

fn bot(size: f32, color: rune::Color) -> rune::Shape {
    let p = rune::Path::new()
        .move_to([12.0, 8.0])
        .line_to([12.0, 4.0])
        .move_to([8.0, 8.0])
        .line_to([16.0, 8.0])
        .cubic_to([17.1, 8.0], [18.0, 8.9], [18.0, 10.0])
        .line_to([18.0, 18.0])
        .cubic_to([18.0, 19.1], [17.1, 20.0], [16.0, 20.0])
        .line_to([8.0, 20.0])
        .cubic_to([6.9, 20.0], [6.0, 19.1], [6.0, 18.0])
        .line_to([6.0, 10.0])
        .cubic_to([6.0, 8.9], [6.9, 8.0], [8.0, 8.0])
        .move_to([9.0, 13.0])
        .line_to([9.01, 13.0])
        .move_to([15.0, 13.0])
        .line_to([15.01, 13.0]);
    rune::Shape::new()
        .stroke_path(&p, crate::lucide_stroke(size), rune::LineCap::Round, color)
        .view_box(24.0, 24.0)
}

fn toggle_tool<S: TranscriptHost + 'static>(
    s: &mut S,
    which: Thread,
    row_key: &str,
    item_key: &str,
    default_open: bool,
) {
    let thread = s.thread_mut(which);
    if let Some(row) = thread.rows.iter_mut().find(|row| row.key == row_key) {
        let open = row.tool_open.get(item_key).copied().unwrap_or(default_open);
        row.tool_open.insert(item_key.to_string(), !open);
    }
}

/// `ActivityTimeline`.
pub(super) fn activity_timeline<S: TranscriptHost + 'static>(
    s: &S,
    which: Thread,
    row_key: &str,
    segments: &[StreamSegment],
    is_active: bool,
    data: &RowData,
) -> El {
    let segments = if is_active && segments.len() > 18 {
        &segments[segments.len() - 18..]
    } else {
        segments
    };
    if segments.is_empty() {
        return col().key(format!("{row_key}-activity-empty"));
    }

    enum Entry<'a> {
        Tools(Vec<&'a LiveToolEvent>),
        Reasoning(&'a str),
        Text(&'a str),
    }
    let mut entries: Vec<Entry<'_>> = Vec::new();
    for segment in segments {
        match segment {
            StreamSegment::Tool { ev } => {
                if let Some(Entry::Tools(events)) = entries.last_mut() {
                    events.push(ev);
                } else {
                    entries.push(Entry::Tools(vec![ev]));
                }
            }
            StreamSegment::Reasoning { text, .. } => entries.push(Entry::Reasoning(text)),
            StreamSegment::Text { text, .. } => entries.push(Entry::Text(text)),
        }
    }
    let mut list = col()
        .key(format!("{row_key}-activity"))
        .w_full()
        .max_w(820.0)
        .gap(5.0)
        .mb(11.0);

    for (index, entry) in entries.iter().enumerate() {
        match entry {
            Entry::Tools(events) => {
                let group_key = format!(
                    "tools-{}",
                    events.first().map(|event| event.id.as_str()).unwrap_or("0")
                );
                let default_open = is_active;
                let open = data
                    .tool_open
                    .get(&group_key)
                    .copied()
                    .unwrap_or(default_open);
                let names: Vec<&str> = events.iter().map(|event| event.name.as_str()).collect();
                let title = tool_labels::group_title(&names);
                let header = row()
                    .key(format!("{row_key}-{group_key}-head"))
                    .role("button")
                    .label(format!("{title} details"))
                    .w_full()
                    .items_center()
                    .gap(7.0)
                    .px(7.0)
                    .py(7.0)
                    .radius(7.0)
                    .hover(home_rgba(255.0, 255.0, 255.0, 0.045))
                    .on_press({
                        let row_key = row_key.to_string();
                        let item_key = group_key.clone();
                        move |s: &mut S| toggle_tool(s, which, &row_key, &item_key, default_open)
                    })
                    .child(icon_box(
                        13.0,
                        tool_icon(tool_labels::IconKind::Wrench, 13.0, home_ink("#aeb1b9")),
                    ))
                    .child(
                        text(title)
                            .key(format!("{row_key}-{group_key}-title"))
                            .font(style(15.0, 600.0))
                            .color(home_ink("#d3d4d9"))
                            .truncate()
                            .grow(1.0)
                            .min_w(0.0),
                    );
                let header = if events.len() > 1 {
                    header.child(
                        text(format!("{} actions", events.len()))
                            .font(style(11.0, 400.0))
                            .color(home_ink("#777b85")),
                    )
                } else {
                    header
                };
                let header = header.child(icon_box(14.0, chevron(14.0, open, home_ink("#747781"))));
                let mut group = col()
                    .key(format!("{row_key}-{group_key}"))
                    .min_w(0.0)
                    .child(header);
                if open {
                    let mut body = col()
                        .key(format!("{row_key}-{group_key}-body"))
                        .gap(2.0)
                        .mt(1.0)
                        .mb(5.0)
                        .ml(13.0)
                        .pl(14.0)
                        .py(2.0)
                        .border_sides([1.0, 0.0, 0.0, 0.0], home_rgba(255.0, 255.0, 255.0, 0.11))
                        .border_layout();
                    for event in events {
                        body = body.child(tool_event_detail::<S>(
                            s,
                            which,
                            row_key,
                            event,
                            &data.tool_open,
                            is_active,
                        ));
                    }
                    group = group.child(body);
                }
                list = list.child(group);
            }
            Entry::Reasoning(note) => {
                list = list.child(
                    col()
                        .key(format!("{row_key}-reasoning-{index}"))
                        .w_full()
                        .max_w(820.0)
                        .py(5.0)
                        .pl(13.0)
                        .border_sides([1.0, 0.0, 0.0, 0.0], home_rgba(171.0, 159.0, 206.0, 0.5))
                        .border_layout()
                        .child(md_render::render_body_compact::<S>(
                            s,
                            which,
                            &format!("{row_key}-reasoning-{index}"),
                            note,
                            "#a1a1aa",
                            12.5,
                        )),
                );
            }
            Entry::Text(note) => {
                list = list.child(
                    col()
                        .key(format!("{row_key}-status-{index}"))
                        .w_full()
                        .max_w(820.0)
                        .my(5.0)
                        .child(md_render::render_body_compact::<S>(
                            s,
                            which,
                            &format!("{row_key}-status-{index}"),
                            note,
                            "#a1a1aa",
                            12.5,
                        )),
                );
            }
        }
    }
    list
}

fn tool_event_detail<S: TranscriptHost + 'static>(
    s: &S,
    which: Thread,
    row_key: &str,
    event: &LiveToolEvent,
    open_map: &std::collections::HashMap<String, bool>,
    is_active: bool,
) -> El {
    if tool_labels::is_delegation(&event.name) {
        if event.children.is_empty() {
            return delegation_card(event, None, is_active, 0);
        }
        let mut cards = col()
            .key(format!("{row_key}-{}", event.id))
            .gap(6.0)
            .w_full();
        for (index, child) in event.children.iter().enumerate() {
            cards = cards.child(delegation_card(event, Some(child), is_active, index));
        }
        return cards;
    }
    let subject = tool_labels::primary_arg(event.args.as_deref());
    let verb = tool_labels::verb(&event.name);
    let status_label = if event.status.is_running() {
        "Running"
    } else if event.ok == Some(false) {
        "Failed"
    } else {
        "Completed"
    };
    let item_key = format!("call-{}", event.id);
    let open = open_map.get(&item_key).copied().unwrap_or(false);
    let has_details = event.args.is_some() || event.preview.is_some() || event.status.is_running();
    let mut title = row()
        .key(format!("{row_key}-{item_key}-row"))
        .role("button")
        .label(format!(
            "{verb}{}{}",
            if subject.is_empty() { "" } else { " " },
            subject
        ))
        .w_full()
        .items_center()
        .gap(7.0)
        .px(5.0)
        .py(4.0)
        .min_h(31.0)
        .radius(7.0)
        .hover(home_rgba(255.0, 255.0, 255.0, 0.045))
        .on_press({
            let row_key = row_key.to_string();
            let item_key = item_key.clone();
            move |s: &mut S| toggle_tool(s, which, &row_key, &item_key, false)
        })
        .child(icon_box(
            13.0,
            tool_icon(
                tool_labels::icon_kind(&event.name),
                13.0,
                home_ink("#8f949e"),
            ),
        ))
        .child(
            text(verb.to_string())
                .font(style(15.0, 500.0))
                .color(home_ink("#bfc1c8")),
        );
    if !subject.is_empty() {
        title = title.child(
            text(subject)
                .font(mono(13.0, 500.0))
                .color(home_ink("#969aa4"))
                .truncate()
                .grow(1.0)
                .min_w(0.0),
        );
    } else {
        title = title.child(rune::spacer());
    }
    let status_icon = if event.status.is_running() {
        icon_box(13.0, loader(13.0, home_ink("#b8bdc4"))).looping(rune::LoopSpec::rotation(0.9))
    } else if event.ok == Some(false) {
        icon_box(13.0, crate::lucide_x(13.0, 2.0, home_ink("#d39ba4")))
    } else {
        icon_box(13.0, check_circle(13.0, home_ink("#b8bdc4")))
    };
    title = title.child(status_icon);
    if !event.status.is_running() && event.ms.is_some_and(|ms| ms > 0) {
        let ms = event.ms.unwrap_or(0);
        title = title.child(
            text(if ms >= 1000 {
                format!("{:.1}s", ms as f64 / 1000.0)
            } else {
                format!("{ms}ms")
            })
            .font(mono(11.0, 400.0))
            .color(home_ink("#747781")),
        );
    }
    if has_details {
        title = title.child(icon_box(13.0, chevron(13.0, open, home_ink("#6f727b"))));
    }
    let mut call = col()
        .key(format!("{row_key}-{item_key}"))
        .min_w(0.0)
        .radius(8.0);
    if open {
        call = call.fill(home_rgba(255.0, 255.0, 255.0, 0.017));
    }
    call = call.child(title);
    if open {
        call = call.child(tool_panel::<S>(
            s,
            which,
            &format!("{row_key}-{item_key}-panel"),
            event,
            status_label,
        ));
    }
    call
}

fn text_field(args: Option<&serde_json::Value>, keys: &[&str]) -> String {
    let Some(serde_json::Value::Object(map)) = args else {
        return String::new();
    };
    for key in keys {
        if let Some(value) = map.get(*key).and_then(|v| v.as_str()) {
            if !value.trim().is_empty() {
                return value.to_string();
            }
        }
    }
    String::new()
}

fn pretty_input(event: &LiveToolEvent) -> String {
    let Some(args) = event.args.as_deref() else {
        return String::new();
    };
    match serde_json::from_str::<serde_json::Value>(args) {
        Ok(value @ serde_json::Value::Object(_)) => {
            serde_json::to_string_pretty(&value).unwrap_or_else(|_| args.to_string())
        }
        _ => args.trim().to_string(),
    }
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    // CSI: consume until a final byte in @..~
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    // OSC: consume until BEL or ESC '\'.
                    let mut prev_esc = false;
                    for c in chars.by_ref() {
                        if c == '\u{7}' || (prev_esc && c == '\\') {
                            break;
                        }
                        prev_esc = c == '\u{1b}';
                    }
                }
                _ => {}
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn edit_diff(args: Option<&serde_json::Value>, tool: &str) -> Option<(String, String)> {
    let path = text_field(args, &["file_path", "path", "file"]);
    let filename = if path.is_empty() {
        "file".to_string()
    } else {
        path
    };
    if tool == "edit" || tool == "edit_file" {
        let old = text_field(args, &["old_string", "oldText", "old"]);
        let new = text_field(args, &["new_string", "newText", "new"]);
        if old.is_empty() && new.is_empty() {
            return None;
        }
        let old_lines: Vec<&str> = old.split('\n').collect();
        let new_lines: Vec<&str> = new.split('\n').collect();
        let mut lines = vec![
            format!("--- a/{filename}"),
            format!("+++ b/{filename}"),
            format!(
                "@@ -1,{} +1,{} @@",
                old_lines.len().max(1),
                new_lines.len().max(1)
            ),
        ];
        lines.extend(old_lines.iter().map(|line| format!("-{line}")));
        lines.extend(new_lines.iter().map(|line| format!("+{line}")));
        return Some((filename, lines.join("\n")));
    }
    if tool == "write" || tool == "write_file" {
        let content = text_field(args, &["content", "text", "body"]);
        if content.is_empty() {
            return None;
        }
        let lines: Vec<&str> = content.split('\n').collect();
        let mut out = vec![
            "--- /dev/null".to_string(),
            format!("+++ b/{filename}"),
            format!("@@ -0,0 +1,{} @@", lines.len()),
        ];
        out.extend(lines.iter().map(|line| format!("+{line}")));
        return Some((filename, out.join("\n")));
    }
    if tool == "apply_patch" || tool == "patch" {
        let patch = text_field(args, &["patch", "input", "diff"]);
        if patch.is_empty() || (!patch.contains("@@") && !patch.contains("---")) {
            return None;
        }
        return Some((filename, patch));
    }
    None
}

fn tool_panel<S: TranscriptHost + 'static>(
    s: &S,
    which: Thread,
    key: &str,
    event: &LiveToolEvent,
    status: &str,
) -> El {
    let args_value = event
        .args
        .as_deref()
        .and_then(|args| serde_json::from_str::<serde_json::Value>(args).ok());
    let args = args_value.as_ref();
    let output = strip_ansi(event.preview.as_deref().unwrap_or(""));
    let running = event.status.is_running();
    let diff = edit_diff(args, &event.name);
    let command = text_field(args, &["command", "cmd", "script"]);
    let path = text_field(args, &["file_path", "path", "file"]);

    let mut head = row()
        .key(format!("{key}-head"))
        .items_center()
        .gap(7.0)
        .min_h(29.0)
        .px(9.0)
        .py(5.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], home_rgba(255.0, 255.0, 255.0, 0.07))
        .border_layout()
        .fill(home_rgba(255.0, 255.0, 255.0, 0.025))
        .child(
            text(event.name.clone())
                .font(mono(13.0, 500.0))
                .color(home_ink("#c9cbd2"))
                .truncate()
                .grow(1.0)
                .min_w(0.0),
        )
        .child(
            text(status.to_uppercase())
                .font({
                    let mut st = TextStyle::new(10.5, 650.0);
                    st.line_height = 15.0;
                    st.tracking = 0.4;
                    st
                })
                .color(if event.ok == Some(false) {
                    home_ink("#d39ba4")
                } else {
                    home_ink("#898d96")
                }),
        );
    let copy_text = if let Some((filename, diff)) = &diff {
        format!("{filename}\n\n{diff}")
    } else {
        [command.clone(), output.clone()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    let copy_text = if copy_text.is_empty() {
        pretty_input(event)
    } else {
        copy_text
    };
    head = head.child(
        rune::el()
            .key(format!("{key}-copy"))
            .role("button")
            .label("Copy tool details")
            .size(22.0, 22.0)
            .radius(5.0)
            .center()
            .hover(home_rgba(255.0, 255.0, 255.0, 0.07))
            .on_press_tx(move |_: &mut S, tx: &mut rune::Tx| {
                tx.copy(copy_text.clone());
            })
            .child(icon_box(11.0, home_icon_copy(11.0, home_ink("#777b85")))),
    );
    // The copy control is keyboard/pointer inert in the headless shell only
    // when the app cannot reach a clipboard; in the real app the runtime
    // handles it through `on_press_tx`. Keep the visual shape identical.
    let mut panel = col()
        .key(key)
        .ml(19.0)
        .mt(2.0)
        .mb(7.0)
        .radius(9.0)
        .border(1.0, home_rgba(255.0, 255.0, 255.0, 0.09))
        .border_layout()
        .fill(home_ink("#0b0b0e"))
        .child(head);
    if let Some((filename, diff)) = diff {
        let mut body = col()
            .key(format!("{key}-diff"))
            .px(8.0)
            .py(8.0)
            .child(md_render::render_diff::<S>(s, which, key, &diff));
        if !output.trim().is_empty() {
            body = body.child(
                text(output)
                    .font(mono(13.0, 400.0))
                    .color(home_ink("#9b9ea7"))
                    .mt(6.0)
                    .px(8.0)
                    .py(7.0)
                    .radius(7.0)
                    .border(1.0, home_rgba(255.0, 255.0, 255.0, 0.06))
                    .border_layout()
                    .fill(home_rgba(255.0, 255.0, 255.0, 0.018)),
            );
        }
        panel = panel.child(body);
        let _ = filename;
    } else if tool_labels::is_terminal(&event.name) {
        let mut body = col().key(format!("{key}-command-wrap")).gap(0.0).child(
            row()
                .items_start()
                .gap(7.0)
                .px(10.0)
                .py(8.0)
                .border_sides([0.0, 0.0, 1.0, 0.0], home_rgba(255.0, 255.0, 255.0, 0.06))
                .border_layout()
                .fill(home_rgba(255.0, 255.0, 255.0, 0.018))
                .child(
                    text("$".to_string())
                        .font(mono(11.0, 500.0))
                        .color(home_ink("#6f727b")),
                )
                .child(
                    text(if command.is_empty() {
                        let desc = text_field(args, &["description"]);
                        if desc.is_empty() {
                            event.name.clone()
                        } else {
                            desc
                        }
                    } else {
                        command.clone()
                    })
                    .font(mono(13.0, 500.0))
                    .color(home_ink("#dfe0e4"))
                    .grow(1.0)
                    .min_w(0.0),
                ),
        );
        if event.args.is_some() && command.is_empty() {
            body = body.child(
                text(pretty_input(event))
                    .font(mono(13.0, 400.0))
                    .color(home_ink("#aeb1b8"))
                    .px(10.0)
                    .pt(7.0)
                    .pb(9.0),
            );
        }
        body = body.child(tool_output(&format!("{key}-output"), &output, running));
        panel = panel.child(body);
    } else {
        let mut body = col().key(format!("{key}-sections"));
        body = body.child(
            col()
                .child(
                    text("Input".to_string())
                        .font({
                            let mut st = TextStyle::new(9.0, 700.0);
                            st.line_height = 13.0;
                            st.tracking = 0.72;
                            st
                        })
                        .color(home_ink("#777b85"))
                        .px(10.0)
                        .pt(7.0),
                )
                .child(
                    text({
                        let pretty = pretty_input(event);
                        if pretty.is_empty() {
                            "—".to_string()
                        } else {
                            pretty
                        }
                    })
                    .font(mono(13.0, 400.0))
                    .color(home_ink("#aeb1b8"))
                    .px(10.0)
                    .pt(7.0)
                    .pb(9.0),
                ),
        );
        body = body.child(
            col()
                .border_sides([1.0, 0.0, 0.0, 0.0], home_rgba(255.0, 255.0, 255.0, 0.055))
                .border_layout()
                .child(
                    text(if path.is_empty() {
                        "Output".to_string()
                    } else {
                        format!("Output · {path}")
                    })
                    .font({
                        let mut st = TextStyle::new(9.0, 700.0);
                        st.line_height = 13.0;
                        st.tracking = 0.72;
                        st
                    })
                    .color(home_ink("#777b85"))
                    .px(10.0)
                    .pt(7.0),
                )
                .child(tool_output(&format!("{key}-output"), &output, running)),
        );
        panel = panel.child(body);
    }
    panel
}

fn tool_output(key: &str, output: &str, running: bool) -> El {
    if output.trim().is_empty() {
        return row()
            .items_center()
            .gap(6.0)
            .px(10.0)
            .py(9.0)
            .child(if running {
                spinner(12.0, home_ink("#747781"))
            } else {
                rune::el()
            })
            .child(
                text(if running {
                    "Waiting for output…"
                } else {
                    "No output captured."
                })
                .font(style(11.5, 400.0))
                .color(home_ink("#747781")),
            );
    }
    col()
        .key(key)
        .max_h(300.0)
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        })
        .child(
            text(output.to_string())
                .font(mono(13.0, 400.0))
                .color(home_ink("#b9bcc4"))
                .px(10.0)
                .py(9.0),
        )
}

fn delegation_card(
    event: &LiveToolEvent,
    step: Option<&crate::stream_accum::ChildRef>,
    is_active: bool,
    index: usize,
) -> El {
    let (name, task) = if let Some(step) = step {
        (
            step.agent_name
                .clone()
                .unwrap_or_else(|| "Subagent".to_string()),
            step.label
                .clone()
                .unwrap_or_else(|| "Delegated task".to_string()),
        )
    } else {
        let (name, task) = tool_labels::delegation_target(event.args.as_deref());
        let name = if name.is_empty() {
            "Subagent".to_string()
        } else {
            name
        };
        let task = if task.is_empty() {
            let arg = tool_labels::primary_arg(event.args.as_deref());
            if arg.is_empty() {
                "Delegated task".to_string()
            } else {
                arg
            }
        } else {
            task
        };
        (name, task)
    };
    let running = is_active || event.status.is_running();
    let failed = event.ok == Some(false);
    let status = if running {
        "Working"
    } else if failed {
        "Failed"
    } else {
        "Completed"
    };
    let mut status_row = row().items_center().gap(4.0);
    status_row = status_row.child(if running {
        spinner(12.0, home_ink("#989da5"))
    } else if failed {
        icon_box(12.0, crate::lucide_x(12.0, 2.0, home_ink("#d39ba4")))
    } else {
        icon_box(12.0, check_circle(12.0, home_ink("#989da5")))
    });
    status_row = status_row.child(text(status.to_string()).font(style(10.0, 400.0)).color(
        if failed {
            home_ink("#d39ba4")
        } else {
            home_ink("#989da5")
        },
    ));
    row()
        .key(format!("{}-delegation-{index}", event.id))
        .w_full()
        .max_w(620.0)
        .my(4.0)
        .child(
            row()
                .w_full()
                .min_h(54.0)
                .items_center()
                .gap(11.0)
                .px(13.0)
                .py(11.0)
                .radius(11.0)
                .border(1.0, home_rgba(255.0, 255.0, 255.0, 0.09))
                .border_layout()
                .fill(home_rgba(255.0, 255.0, 255.0, 0.028))
                .child(
                    rune::el()
                        .size(29.0, 29.0)
                        .radius(9.0)
                        .fill(home_rgba(255.0, 255.0, 255.0, 0.07))
                        .center()
                        .child(icon_box(15.0, bot(15.0, home_ink("#c9cbd2")))),
                )
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(2.0)
                        .child(
                            row()
                                .items_center()
                                .gap(4.0)
                                .child(
                                    text(name)
                                        .font(style(12.5, 650.0))
                                        .color(home_ink("#e4e5e8")),
                                )
                                .child(
                                    text("subagent".to_string())
                                        .font(style(10.0, 500.0))
                                        .color(home_ink("#888b95")),
                                ),
                        )
                        .child(
                            text(task)
                                .font(style(11.5, 400.0))
                                .color(home_ink("#a7a9b0"))
                                .truncate(),
                        ),
                )
                .child(status_row),
        )
}

/// Completed trace accordion. Live rows render no completed shell.
pub(super) fn trace_accordion<S: TranscriptHost + 'static>(
    s: &S,
    which: Thread,
    r: &RowData,
) -> El {
    let duration = if r.duration.is_empty() || r.duration == "active" {
        "Worked for a few seconds".to_string()
    } else if let Ok(seconds) = r.duration.trim_end_matches('s').parse::<u64>() {
        format!("Worked for {seconds}s")
    } else {
        format!("Worked for {}", r.duration)
    };
    let open = r.trace_open;
    let header = row()
        .key(format!("{}-trace-head", r.key))
        .role("button")
        .label(format!("{duration} activity details"))
        .w_full()
        .h(42.0)
        .items_center()
        .gap(7.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], home_ink("#ffffff16"))
        .border_layout()
        .on_press({
            let key = r.key.clone();
            move |s: &mut S| {
                let thread = s.thread_mut(which);
                if let Some(row) = thread.rows.iter_mut().find(|row| row.key == key) {
                    row.trace_open = !row.trace_open;
                }
            }
        })
        .child(
            text(duration)
                .font({
                    let mut style = TextStyle::new(13.0, 500.0);
                    style.line_height = 19.5;
                    style.tracking = 0.065;
                    style
                })
                .color(home_ink("#a4a4ad")),
        )
        .child(icon_box(14.0, chevron(14.0, open, home_ink("#71717a"))));
    let mut shell = col()
        .key(format!("{}-trace", r.key))
        .w_full()
        .mt(8.0)
        .mb(22.0)
        .child(header)
        // Header inline margin (14 open / 0 closed) + collapsible top 9.
        .child(rune::el().h(if open { 23.0 } else { 9.0 }));
    if open {
        let timeline: Vec<StreamSegment> = if !r.activity.is_empty() {
            r.activity.clone()
        } else {
            project_steps(&r.steps)
        };
        if !timeline.is_empty() {
            shell = shell.child(activity_timeline::<S>(
                s, which, &r.key, &timeline, false, r,
            ));
        }
        let cards = delegation_fallback(&r.steps, &timeline);
        if !cards.is_empty() {
            let mut list = col()
                .key(format!("{}-delegations", r.key))
                .gap(6.0)
                .max_w(620.0)
                .my(4.0);
            for step in cards {
                list = list.child(delegation_step_card(step));
            }
            shell = shell.child(list);
        }
    }
    shell
}

fn project_steps(steps: &[TraceStep]) -> Vec<StreamSegment> {
    let mut out = Vec::new();
    for (index, step) in steps.iter().enumerate() {
        if step.child_id.is_some() {
            continue;
        }
        if matches!(step.kind.as_str(), "tool" | "command") {
            out.push(StreamSegment::Tool {
                ev: LiveToolEvent {
                    id: if step.id.is_empty() {
                        format!("step-{index}")
                    } else {
                        step.id.clone()
                    },
                    name: if step.tool_name.is_empty() {
                        step.title.clone()
                    } else {
                        step.tool_name.clone()
                    },
                    args: None,
                    status: ToolStatus::Done,
                    ok: Some(!step.is_error),
                    ms: None,
                    preview: Some(step.content.clone()).filter(|s| !s.trim().is_empty()),
                    agent: Some(step.agent.clone()).filter(|s| !s.is_empty()),
                    children: Vec::new(),
                },
            });
        } else if !step.content.trim().is_empty() {
            out.push(StreamSegment::Text {
                text: step.content.clone(),
                agent: None,
            });
        }
    }
    out
}

fn delegation_fallback<'a>(
    steps: &'a [TraceStep],
    timeline: &[StreamSegment],
) -> Vec<&'a TraceStep> {
    let mut claimed = std::collections::HashSet::new();
    for segment in timeline {
        if let StreamSegment::Tool { ev } = segment {
            for child in &ev.children {
                claimed.insert(child.chat_id.clone());
            }
        }
    }
    steps
        .iter()
        .filter(|step| {
            step.child_id
                .as_ref()
                .is_some_and(|child| !claimed.contains(child))
        })
        .collect()
}

fn delegation_step_card(step: &TraceStep) -> El {
    let name = step
        .child_name
        .clone()
        .unwrap_or_else(|| "Subagent".to_string());
    let task = step
        .label
        .clone()
        .or_else(|| Some(step.title.clone()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Delegated task".to_string());
    row()
        .key(format!("{}-delegation-step", step.id))
        .w_full()
        .max_w(620.0)
        .child(
            row()
                .w_full()
                .min_h(54.0)
                .items_center()
                .gap(11.0)
                .px(13.0)
                .py(11.0)
                .radius(11.0)
                .border(1.0, home_rgba(255.0, 255.0, 255.0, 0.09))
                .border_layout()
                .fill(home_rgba(255.0, 255.0, 255.0, 0.028))
                .child(
                    rune::el()
                        .size(29.0, 29.0)
                        .radius(9.0)
                        .fill(home_rgba(255.0, 255.0, 255.0, 0.07))
                        .center()
                        .child(icon_box(15.0, bot(15.0, home_ink("#c9cbd2")))),
                )
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(2.0)
                        .child(
                            text(name)
                                .font(style(12.5, 650.0))
                                .color(home_ink("#e4e5e8")),
                        )
                        .child(
                            text(task)
                                .font(style(11.5, 400.0))
                                .color(home_ink("#a7a9b0"))
                                .truncate(),
                        ),
                )
                .child(
                    text("Completed".to_string())
                        .font(style(10.0, 400.0))
                        .color(home_ink("#989da5")),
                ),
        )
}

/// Live `TraceAccordion` fallback: only unclaimed child sessions, rendered
/// after the chronological activity feed. Returns `None` when the stream
/// already has an inline delegation tool event.
pub(super) fn live_delegation_cards(r: &RowData, timeline: &[StreamSegment]) -> Option<El> {
    if timeline.iter().any(|segment| {
        matches!(segment, StreamSegment::Tool { ev } if tool_labels::is_delegation(&ev.name))
    }) {
        return None;
    }
    let cards = delegation_fallback(&r.steps, timeline);
    if cards.is_empty() {
        return None;
    }
    let mut list = col()
        .key(format!("{}-live-delegations", r.key))
        .gap(6.0)
        .max_w(620.0)
        .my(4.0);
    for step in cards {
        list = list.child(delegation_step_card(step));
    }
    Some(list)
}

/// React `ThinkingPlaceholder`.
pub(super) fn thinking_placeholder(key: &str) -> El {
    let mut dots = row().items_center().gap(2.0).ml(2.0);
    for (n, delay) in [0.0, 0.16, 0.32].into_iter().enumerate() {
        dots = dots.child(
            rune::el()
                .key(format!("{key}-think-dot-{n}"))
                .size(4.0, 4.0)
                .radius(2.0)
                .fill(home_ink("#71717a"))
                .looping(rune::LoopSpec::opacity(1.1, delay, 0.28, 1.0)),
        );
    }
    row()
        .key(format!("{key}-thinking"))
        .items_center()
        .gap(8.0)
        .mb(8.0)
        .child(
            rune::el()
                .size(6.0, 6.0)
                .radius(3.0)
                .fill(home_ink("#a1a1aa"))
                .looping(rune::LoopSpec::opacity(1.3, 0.0, 0.25, 1.0)),
        )
        .child(
            text("Thinking".to_string())
                .font(style(13.0, 400.0))
                .color(home_ink("#71717a")),
        )
        .child(dots)
}

/// `RetryStatus`.
pub(super) fn retry_status(key: &str, retry: &RetryNotice) -> El {
    let total = retry.max.max(1);
    let attempt = retry.attempt.clamp(1, total);
    let mut track = row().gap(3.0).w_full().max_w(150.0);
    for index in 0..total {
        let filled = index < attempt;
        let mut segment = rune::el()
            .key(format!("{key}-retry-seg-{index}"))
            .grow(1.0)
            .h(2.0)
            .radius(1.0);
        if filled {
            segment = segment
                .fill(home_rgba(255.0, 255.0, 255.0, 0.72))
                .looping(rune::LoopSpec::opacity(1.4, 0.0, 0.35, 1.0));
        } else {
            segment = segment.fill(home_rgba(255.0, 255.0, 255.0, 0.10));
        }
        track = track.child(segment);
    }
    row()
        .key(format!("{key}-retry"))
        .w_full()
        .max_w(520.0)
        .mb(10.0)
        .items_center()
        .gap(10.0)
        .px(11.0)
        .py(8.0)
        .radius(10.0)
        .border(1.0, home_rgba(255.0, 255.0, 255.0, 0.085))
        .border_layout()
        .linear_gradient(
            0.0,
            [
                (0.0, home_rgba(255.0, 255.0, 255.0, 0.05)),
                (1.0, home_rgba(255.0, 255.0, 255.0, 0.018)),
            ],
        )
        .child(
            rune::el()
                .size(22.0, 22.0)
                .radius(7.0)
                .border(1.0, home_rgba(255.0, 255.0, 255.0, 0.1))
                .border_layout()
                .fill(home_rgba(255.0, 255.0, 255.0, 0.045))
                .center()
                .child(spinner(12.0, home_ink("#c7c9d0"))),
        )
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(4.0)
                .child(
                    row()
                        .items_center()
                        .gap(7.0)
                        .child(
                            text("Reconnecting".to_string())
                                .font(style(12.0, 600.0))
                                .color(home_ink("#e7e7eb")),
                        )
                        .child(
                            text(format!("{attempt}/{total}"))
                                .font(mono(10.5, 600.0))
                                .color(home_ink("#ffffff")),
                        )
                        .child(
                            text("provider hiccup — retrying".to_string())
                                .font(style(10.0, 400.0))
                                .color(home_ink("#7f828b"))
                                .truncate()
                                .grow(1.0)
                                .min_w(0.0),
                        ),
                )
                .child(track),
        )
}

/// `LiveCompactionStatus`.
pub(super) fn live_compaction_status(key: &str, status: &RunStatusNotice) -> El {
    let failed = status.kind == "compaction_failed";
    let completed = status.kind == "compaction_completed";
    let label = if failed {
        "Session compaction failed"
    } else if completed {
        "Session compacted"
    } else {
        "Session compaction started"
    };
    let line = || {
        rune::el().grow(1.0).h(1.0).linear_gradient(
            std::f32::consts::FRAC_PI_2,
            [
                (0.0, home_rgba(255.0, 255.0, 255.0, 0.0)),
                (0.5, home_rgba(255.0, 255.0, 255.0, 0.09)),
                (1.0, home_rgba(255.0, 255.0, 255.0, 0.0)),
            ],
        )
    };
    let mut main = row().items_center().gap(6.0).child(
        rune::el()
            .size(18.0, 18.0)
            .radius(6.0)
            .border(
                1.0,
                home_rgba(255.0, 255.0, 255.0, if failed { 0.2 } else { 0.08 }),
            )
            .border_layout()
            .fill(if failed {
                home_rgba(251.0, 113.0, 133.0, 0.06)
            } else {
                home_rgba(255.0, 255.0, 255.0, 0.035)
            })
            .center()
            .child(if completed {
                icon_box(11.0, crate::lucide_check(11.0, 2.0, home_ink("#9b9ba5")))
            } else if failed {
                icon_box(11.0, home_ink_alert(11.0))
            } else {
                spinner(11.0, home_ink("#d6d6dc"))
            }),
    );
    main = main.child(
        text(label.to_string())
            .font({
                let mut st = TextStyle::new(10.5, 600.0);
                st.line_height = 15.0;
                st.tracking = 0.1;
                st
            })
            .color(if failed {
                home_ink("#f0a6b1")
            } else {
                home_ink("#a1a1a9")
            }),
    );
    if completed && (status.input_tokens.unwrap_or(0) > 0 || status.output_tokens.unwrap_or(0) > 0)
    {
        main = main.child(
            text(format!(
                "{} in · {} out",
                compact_tokens(status.input_tokens.unwrap_or(0)),
                compact_tokens(status.output_tokens.unwrap_or(0))
            ))
            .font(mono(9.5, 400.0))
            .color(home_ink("#5f5f68")),
        );
    } else if !completed && !failed {
        main = main.child(
            text("summarizing older context…".to_string())
                .font(mono(9.5, 400.0))
                .color(home_ink("#5f5f68")),
        );
    } else if failed {
        main = main.child(
            text(if status.reason.as_deref() == Some("no-reduction") {
                "context unchanged"
            } else {
                "continuing with current context"
            })
            .font(mono(9.5, 400.0))
            .color(home_ink("#5f5f68")),
        );
    }
    row()
        .key(format!("{key}-compaction"))
        .w_full()
        .items_center()
        .gap(10.0)
        .my(10.0)
        .mb(16.0)
        .child(line())
        .child(main)
        .child(line())
}

fn home_ink_alert(size: f32) -> rune::Shape {
    const K: f32 = 5.0;
    let circle = rune::Path::new()
        .move_to([12.0, 2.0])
        .cubic_to([12.0 + K, 2.0], [22.0, 12.0 - K], [22.0, 12.0])
        .cubic_to([22.0, 12.0 + K], [12.0 + K, 22.0], [12.0, 22.0])
        .cubic_to([12.0 - K, 22.0], [2.0, 12.0 + K], [2.0, 12.0])
        .cubic_to([2.0, 12.0 - K], [12.0 - K, 2.0], [12.0, 2.0])
        .close()
        .move_to([12.0, 8.0])
        .line_to([12.0, 12.0])
        .move_to([12.0, 16.0])
        .line_to([12.01, 16.0]);
    rune::Shape::new()
        .stroke_path(
            &circle,
            crate::lucide_stroke(size),
            rune::LineCap::Round,
            home_ink("#fda4af"),
        )
        .view_box(24.0, 24.0)
}

fn compact_tokens(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 10_000 {
        format!("{:.0}k", value as f64 / 1_000.0)
    } else if value >= 1_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

/// `Stopped — partial response kept`.
pub(super) fn stopped_badge(key: &str) -> El {
    text("Stopped — partial response kept".to_string())
        .key(format!("{key}-stopped"))
        .font({
            let mut st = TextStyle::new(11.0, 600.0);
            st.line_height = 16.5;
            st
        })
        .color(home_ink("#a1a1aa"))
        .mt(6.0)
        .px(8.0)
        .py(2.0)
        .radius(6.0)
        .border(1.0, home_rgba(161.0, 161.0, 170.0, 0.2))
        .border_layout()
        .fill(home_rgba(161.0, 161.0, 170.0, 0.1))
}
