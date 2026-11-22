//! Group-project labels, chips, avatars, clocks, and the derived chat feed.

use rune::{text, El};

use super::data::{GroupDetail, GroupEvent, GroupItem, GroupMember};

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

pub const OWNER_NAMES: [&str; 4] = ["owner", "human", "you", "user"];
pub const SUPERVISOR_NAMES: [&str; 3] = ["supervisor", "stark", "orchestrator"];
pub const NON_PARTICIPANTS: [&str; 9] = [
    "all",
    "everyone",
    "group",
    "owner",
    "human",
    "you",
    "user",
    "supervisor",
    "stark",
];

pub fn raw_name(value: &str) -> String {
    let lower = value.trim();
    let without = lower
        .strip_prefix("member:")
        .or_else(|| lower.strip_prefix("MEMBER:"))
        .unwrap_or(lower);
    without.trim().to_string()
}

pub fn is_owner(value: &str) -> bool {
    let name = raw_name(value).to_lowercase();
    OWNER_NAMES.contains(&name.as_str())
}

pub fn is_supervisor(value: &str) -> bool {
    let name = raw_name(value).to_lowercase();
    SUPERVISOR_NAMES.contains(&name.as_str())
}

pub fn display_actor(value: &str) -> String {
    let raw = value.trim();
    if raw.is_empty() {
        return "Stark".to_string();
    }
    let lower = raw.to_lowercase();
    if lower.starts_with("member:") {
        let member = raw[7..].trim();
        if member.is_empty() {
            "Member".to_string()
        } else {
            capitalize(member)
        }
    } else if lower == "supervisor" {
        "Stark".to_string()
    } else if lower == "owner" || lower == "human" || lower == "you" {
        "You".to_string()
    } else if lower == "orchestrator" {
        "Orchestrator".to_string()
    } else {
        raw.to_string()
    }
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

pub fn actor_label(value: &str) -> String {
    if is_owner(value) {
        return "You".to_string();
    }
    if is_supervisor(value) {
        return "Orchestrator".to_string();
    }
    let label = display_actor(value);
    if label.is_empty() {
        "Member".to_string()
    } else {
        capitalize(&label)
    }
}

pub fn target_label(value: &str) -> String {
    match value {
        "all" => "Everyone".to_string(),
        "supervisor" => "Orchestrator".to_string(),
        other => actor_label(other),
    }
}

pub fn same_actor(left: &str, right: &str) -> bool {
    raw_name(left).to_lowercase() == raw_name(right).to_lowercase()
}

pub fn initials(value: &str) -> String {
    let name = display_actor(value);
    let parts: Vec<&str> = name
        .split(|c: char| c.is_whitespace() || c == '_' || c == '-')
        .filter(|p| !p.is_empty())
        .collect();
    let first = parts.first().and_then(|p| p.chars().next()).unwrap_or('S');
    let second = parts.get(1).and_then(|p| p.chars().next()).unwrap_or(' ');
    format!(
        "{}{}",
        first.to_uppercase(),
        second.to_uppercase().to_string().trim()
    )
}

pub fn avatar(name: &str, small: bool, owner: bool) -> El {
    let (size, radius, font) = if small {
        (22.0, 7.0, 8.0)
    } else {
        (28.0, 9.0, 9.0)
    };
    let mut el = rune::el().size(size, size).radius(radius).center();
    if owner {
        el = el.fill(ink("#ededf0"));
    } else {
        el = el
            .fill(rgba(255.0, 255.0, 255.0, 0.055))
            .border(1.0, rgba(255.0, 255.0, 255.0, 0.09))
            .border_layout();
    }
    el.child(
        text(initials(name))
            .font({
                let mut style = rune::tok::TextStyle::new(font, 750.0);
                style.line_height = font * 1.5;
                style
            })
            .color(if owner {
                ink("#0a0a0d")
            } else {
                ink("#c4c6cd")
            }),
    )
}

pub fn status_chip(status: &str, awaiting: bool) -> El {
    let (bg, fg) = if awaiting {
        (rgba(251.0, 191.0, 36.0, 0.14), ink("#fcd34d"))
    } else {
        match status {
            "done" => (rgba(126.0, 224.0, 168.0, 0.13), ink("#7ee0a8")),
            "doing" => (rgba(125.0, 211.0, 252.0, 0.13), ink("#7dd3fc")),
            "blocked" | "failed" => (rgba(251.0, 113.0, 133.0, 0.13), ink("#fb7185")),
            "invalidated" => (rgba(179.0, 160.0, 255.0, 0.13), ink("#b3a0ff")),
            _ => (rgba(255.0, 255.0, 255.0, 0.07), ink("#a1a1aa")),
        }
    };
    text(if awaiting {
        "NEEDS YOU".to_string()
    } else if status.is_empty() {
        "unknown".to_string()
    } else {
        status.to_uppercase()
    })
    .font({
        let mut style = rune::tok::TextStyle::new(9.0, 600.0);
        style.line_height = 13.5;
        style.tracking = 0.18;
        style
    })
    .px(6.0)
    .py(1.0)
    .radius(5.0)
    .fill(bg)
    .color(fg)
}

const EVENT_LABELS: &[(&str, &str)] = &[
    ("project.created", "Project created"),
    ("project.paused", "Paused"),
    ("project.resumed", "Resumed"),
    ("project.closed", "Closed"),
    ("project.stalled", "Stalled — no possible progress"),
    ("milestone.created", "Milestone added"),
    ("milestone.ready", "Milestone started"),
    ("milestone.done", "Milestone done"),
    ("milestone.failed", "Milestone failed"),
    ("item.created", "Item added"),
    ("item.ready", "Item ready"),
    ("item.claimed", "Item claimed"),
    ("item.released", "Item released"),
    ("item.progress", "Progress"),
    ("item.done", "Item done"),
    ("item.failed", "Item failed"),
    ("item.blocked", "Item blocked"),
    ("item.invalidated", "Item invalidated — plan was wrong"),
    ("item.cancelled", "Item cancelled"),
    ("plan.revised", "Plan revised"),
    ("idea.proposed", "Idea proposed"),
    ("op.submitted", "Change queued"),
    ("op.applied", "Change applied"),
    ("op.rejected", "Change rejected"),
    ("lease.expired", "Worker stopped responding"),
    ("budget.warn", "Budget warning"),
    ("budget.stop", "Budget ceiling reached"),
    ("deadline.reached", "Deadline reached"),
    ("approval.requested", "Approval requested"),
    ("approval.granted", "Approved"),
    ("approval.rejected", "Rejected"),
    ("member.added", "Member added"),
    ("member.removed", "Member removed"),
    ("member.halted", "Member halted"),
    ("capability.changed", "Capability changed"),
    ("message.sent", "Message"),
    ("message.read", "Read"),
];

pub fn event_label(kind: &str) -> String {
    EVENT_LABELS
        .iter()
        .find(|(key, _)| *key == kind)
        .map(|(_, label)| (*label).to_string())
        .unwrap_or_else(|| kind.to_string())
}

/// `Money` in `groupProjectUi.ts`: two decimals from $1, four below it.
pub fn money(value: f64) -> String {
    if value >= 1.0 {
        format!("${value:.2}")
    } else {
        format!("${value:.4}")
    }
}

pub fn time_ago(seconds: i64) -> String {
    if seconds <= 0 {
        return String::new();
    }
    let now = (crate::clock::now_millis() / 1000) as i64;
    let mins = (now - seconds).max(0) / 60;
    if mins < 1 {
        "just now".to_string()
    } else if mins < 60 {
        format!("{mins}m ago")
    } else if mins < 24 * 60 {
        format!("{}h ago", mins / 60)
    } else {
        format!("{}d ago", mins / (24 * 60))
    }
}

/// `YYYY-MM-DDTHH:MM:SSZ` from Unix seconds (UTC civil-date conversion).
pub fn iso_from_secs(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        if m <= 2 { y + 1 } else { y },
        m,
        d,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// `shortTime`: the browser's local 12-hour clock (`02:44 PM`).
pub fn short_time(seconds: i64) -> String {
    if seconds <= 0 {
        return String::new();
    }
    crate::rune_transcript::clock(&iso_from_secs(seconds))
}

/// Project deadline (`new Date(deadline * 1000).toLocaleString()`), rendered
/// as the ISO day plus the same 12-hour clock.
pub fn deadline_label(seconds: f64) -> String {
    if seconds <= 0.0 {
        return String::new();
    }
    let iso = iso_from_secs(seconds as i64);
    format!("{} {}", &iso[..10], crate::rune_transcript::clock(&iso))
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChatEntry {
    pub id: String,
    pub kind: String,
    pub ts: i64,
    pub from: String,
    pub to: String,
    pub body: String,
    pub message_kind: String,
    pub item_id: Option<String>,
    pub error: Option<String>,
    pub derived: bool,
}

impl ChatEntry {
    pub fn is_message(&self) -> bool {
        self.kind == "message"
    }
}

fn event_result(event: &GroupEvent) -> String {
    let data = &event.data;
    for key in ["result", "reason", "detail"] {
        if let Some(value) = data.get(key).and_then(|v| v.as_str()) {
            return value.to_string();
        }
    }
    String::new()
}

fn event_error(event: &GroupEvent) -> String {
    event
        .data
        .get("error")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// `deriveChatEntries`: a readable transcript for projects whose messages
/// predate the persisted message feed.
pub fn derive_chat_entries(events: &[GroupEvent], items: &[GroupItem]) -> Vec<ChatEntry> {
    let mut out = Vec::new();
    for event in events {
        let item = event
            .item_id
            .as_deref()
            .and_then(|id| items.iter().find(|i| i.id == id));
        match event.kind.as_str() {
            "item.claimed" => {
                let owner = event
                    .data
                    .get("lease_owner")
                    .and_then(|v| v.as_str())
                    .map(display_actor)
                    .or_else(|| {
                        item.and_then(|i| i.owner_member.clone())
                            .map(|m| display_actor(&m))
                    })
                    .unwrap_or_else(|| "Member".to_string());
                let body = format!(
                    "Assigned {}{}",
                    event.item_id.as_deref().unwrap_or("work item"),
                    item.map(|i| format!(": {}", i.title)).unwrap_or_default()
                );
                out.push(ChatEntry {
                    id: format!("derived-{}", event.seq),
                    kind: "message".to_string(),
                    ts: event.ts,
                    from: "supervisor".to_string(),
                    to: owner,
                    body,
                    message_kind: "assignment".to_string(),
                    item_id: event.item_id.clone(),
                    error: None,
                    derived: true,
                });
            }
            "item.progress" => {
                let body = event_result(event);
                if body.is_empty() {
                    continue;
                }
                out.push(ChatEntry {
                    id: format!("derived-{}", event.seq),
                    kind: "message".to_string(),
                    ts: event.ts,
                    from: event.actor.clone(),
                    to: "supervisor".to_string(),
                    body,
                    message_kind: "report".to_string(),
                    item_id: event.item_id.clone(),
                    error: Some(event_error(event)).filter(|e| !e.is_empty()),
                    derived: true,
                });
            }
            "approval.requested" => {
                let body = event
                    .data
                    .get("detail")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .or_else(|| {
                        event
                            .data
                            .get("pending")
                            .and_then(|v| v.as_array())
                            .map(|a| {
                                a.iter()
                                    .filter_map(|v| v.as_str())
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            })
                    })
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "Waiting for your approval.".to_string());
                out.push(ChatEntry {
                    id: format!("derived-{}", event.seq),
                    kind: "approval".to_string(),
                    ts: event.ts,
                    from: event.actor.clone(),
                    to: "owner".to_string(),
                    body,
                    message_kind: "approval".to_string(),
                    item_id: event.item_id.clone(),
                    error: None,
                    derived: false,
                });
            }
            "item.done" | "item.failed" | "milestone.done" => {
                let body = match event.kind.as_str() {
                    "item.done" => format!(
                        "{} passed acceptance.",
                        event.item_id.as_deref().unwrap_or("item")
                    ),
                    "item.failed" => {
                        format!("{} failed.", event.item_id.as_deref().unwrap_or("item"))
                    }
                    _ => format!(
                        "{} complete.",
                        event.milestone_id.as_deref().unwrap_or("milestone")
                    ),
                };
                out.push(ChatEntry {
                    id: format!("derived-{}", event.seq),
                    kind: "activity".to_string(),
                    ts: event.ts,
                    from: "supervisor".to_string(),
                    to: "all".to_string(),
                    body,
                    message_kind: "verdict".to_string(),
                    item_id: event.item_id.clone(),
                    error: None,
                    derived: true,
                });
            }
            _ => {}
        }
    }
    out.sort_by_key(|e| e.ts);
    out
}

pub fn chat_entries(detail: Option<&GroupDetail>) -> Vec<ChatEntry> {
    let Some(detail) = detail else {
        return Vec::new();
    };
    if !detail.messages.is_empty() {
        detail
            .messages
            .iter()
            .map(|m| ChatEntry {
                id: format!("message-{}", m.seq),
                kind: if m.kind == "approval" {
                    "approval".to_string()
                } else {
                    "message".to_string()
                },
                ts: m.ts,
                from: m.from.clone(),
                to: m.to.clone(),
                body: m.body.clone(),
                message_kind: m.kind.clone(),
                item_id: m.item_id.clone(),
                error: None,
                derived: false,
            })
            .collect()
    } else {
        derive_chat_entries(&detail.events, &detail.items)
    }
}

pub fn participant_names(entries: &[ChatEntry], members: &[GroupMember]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |raw: &str| {
        let name = raw_name(raw);
        if name.is_empty() {
            return;
        }
        let lower = name.to_lowercase();
        if NON_PARTICIPANTS.contains(&lower.as_str()) {
            return;
        }
        if !out.iter().any(|existing| existing.to_lowercase() == lower) {
            out.push(name);
        }
    };
    for entry in entries {
        add(&entry.from);
        add(&entry.to);
    }
    for member in members {
        add(&member.name);
        add(&member.display);
    }
    out
}

pub fn active_item_for<'a>(items: &'a [GroupItem], member: &str) -> Option<&'a GroupItem> {
    let name = display_actor(member).to_lowercase();
    items.iter().find(|item| {
        (item.status == "doing" || item.status == "ready")
            && display_actor(
                item.lease_owner
                    .as_deref()
                    .or(item.owner_member.as_deref())
                    .unwrap_or(""),
            )
            .to_lowercase()
                == name
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::group::data::{GroupEvent, GroupItem};

    #[test]
    fn actor_labels_and_clocks_match_the_reference_helpers() {
        assert_eq!(actor_label("member:dev"), "Dev");
        assert_eq!(actor_label("supervisor"), "Orchestrator");
        assert_eq!(actor_label("owner"), "You");
        assert!(is_owner("human"));
        assert!(is_supervisor("stark"));
        assert_eq!(initials("member:dev"), "D");
        assert_eq!(initials("group general"), "GG");
        assert_eq!(money(0.5), "$0.5000");
        assert_eq!(money(2.0), "$2.00");
        assert_eq!(iso_from_secs(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn derived_entries_cover_claims_progress_and_approvals() {
        let events = vec![
            GroupEvent {
                seq: 1,
                ts: 10,
                kind: "item.claimed".to_string(),
                actor: "supervisor".to_string(),
                item_id: Some("i1".to_string()),
                data: serde_json::json!({"lease_owner":"member:dev"}),
                ..GroupEvent::default()
            },
            GroupEvent {
                seq: 2,
                ts: 20,
                kind: "item.progress".to_string(),
                actor: "member:dev".to_string(),
                item_id: Some("i1".to_string()),
                data: serde_json::json!({"result":"charts ready"}),
                ..GroupEvent::default()
            },
            GroupEvent {
                seq: 3,
                ts: 30,
                kind: "approval.requested".to_string(),
                actor: "member:dev".to_string(),
                item_id: Some("i2".to_string()),
                data: serde_json::json!({"detail":"sign the export"}),
                ..GroupEvent::default()
            },
            GroupEvent {
                seq: 4,
                ts: 40,
                kind: "item.done".to_string(),
                actor: "supervisor".to_string(),
                item_id: Some("i1".to_string()),
                data: serde_json::json!({}),
                ..GroupEvent::default()
            },
        ];
        let items = vec![GroupItem {
            id: "i1".to_string(),
            title: "Charts".to_string(),
            owner_member: Some("member:dev".to_string()),
            ..GroupItem::default()
        }];
        let out = derive_chat_entries(&events, &items);
        assert_eq!(out.len(), 4);
        assert!(out[0].body.contains("Charts"));
        assert_eq!(out[1].body, "charts ready");
        assert_eq!(out[2].kind, "approval");
        assert_eq!(out[3].kind, "activity");
    }
}
