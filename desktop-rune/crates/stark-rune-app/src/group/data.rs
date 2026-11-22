//! Group-project detail models — the `GroupProjectDetail` wire shape from
//! the original wire types, parsed once into owned Rust values.

use serde_json::Value;

fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn opt_str(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

fn u64_of(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn i64_of(v: &Value, key: &str) -> i64 {
    v.get(key)
        .and_then(Value::as_i64)
        .or_else(|| v.get(key).and_then(Value::as_u64).map(|n| n as i64))
        .unwrap_or(0)
}

fn f64_of(v: &Value, key: &str) -> Option<f64> {
    v.get(key)
        .and_then(Value::as_f64)
        .or_else(|| v.get(key).and_then(Value::as_i64).map(|n| n as f64))
}

fn bool_of(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// `project` envelope in the detail response.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectInfo {
    pub status: String,
    pub goal: String,
    pub spent_usd: Option<f64>,
    pub budget_usd: Option<f64>,
    pub deadline: Option<f64>,
    pub halt_reason: Option<String>,
}

impl ProjectInfo {
    pub fn from_json(v: &Value) -> Self {
        let n = v.get("project").unwrap_or(v);
        Self {
            status: str_of(n, "status"),
            goal: str_of(n, "goal"),
            spent_usd: f64_of(n, "spent_usd"),
            budget_usd: f64_of(n, "budget_usd"),
            deadline: f64_of(n, "deadline"),
            halt_reason: opt_str(n, "halt_reason"),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroupMember {
    pub name: String,
    pub display: String,
    pub kind: String,
    pub model: String,
}

impl GroupMember {
    pub fn from_json(v: &Value) -> Self {
        Self {
            name: str_of(v, "name"),
            display: str_of(v, "display"),
            kind: str_of(v, "kind"),
            model: str_of(v, "model"),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroupMilestone {
    pub id: String,
    pub title: String,
    pub status: String,
    pub depends_on: Vec<String>,
}

impl GroupMilestone {
    pub fn from_json(v: &Value) -> Self {
        Self {
            id: str_of(v, "id"),
            title: str_of(v, "title"),
            status: str_of(v, "status"),
            depends_on: v
                .get("depends_on")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroupItem {
    pub id: String,
    pub project_id: String,
    pub milestone_id: Option<String>,
    pub title: String,
    pub detail: String,
    pub status: String,
    pub attempts: u64,
    pub max_attempts: u64,
    pub owner_member: Option<String>,
    pub lease_owner: Option<String>,
    pub cost_tokens: f64,
    pub cost_usd: Option<f64>,
    pub result: Option<String>,
    pub error: Option<String>,
    pub blocked_reason: Option<String>,
    pub invalidated_reason: Option<String>,
    pub acceptance: Value,
    pub updated_at: i64,
}

impl GroupItem {
    pub fn from_json(v: &Value) -> Self {
        Self {
            id: str_of(v, "id"),
            project_id: str_of(v, "project_id"),
            milestone_id: opt_str(v, "milestone_id"),
            title: str_of(v, "title"),
            detail: str_of(v, "detail"),
            status: str_of(v, "status"),
            attempts: u64_of(v, "attempts"),
            max_attempts: u64_of(v, "max_attempts"),
            owner_member: opt_str(v, "owner_member"),
            lease_owner: opt_str(v, "lease_owner"),
            cost_tokens: f64_of(v, "cost_tokens").unwrap_or(0.0),
            cost_usd: f64_of(v, "cost_usd"),
            result: opt_str(v, "result"),
            error: opt_str(v, "error"),
            blocked_reason: opt_str(v, "blocked_reason"),
            invalidated_reason: opt_str(v, "invalidated_reason"),
            acceptance: v.get("acceptance").cloned().unwrap_or(Value::Null),
            updated_at: i64_of(v, "updated_at"),
        }
    }

    pub fn awaiting_approval(&self) -> bool {
        self.blocked_reason.as_deref() == Some("awaiting_approval")
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroupEvent {
    pub seq: u64,
    pub ts: i64,
    pub kind: String,
    pub actor: String,
    pub milestone_id: Option<String>,
    pub item_id: Option<String>,
    pub data: Value,
}

impl GroupEvent {
    pub fn from_json(v: &Value) -> Self {
        Self {
            seq: u64_of(v, "seq"),
            ts: i64_of(v, "ts"),
            kind: str_of(v, "kind"),
            actor: str_of(v, "actor"),
            milestone_id: opt_str(v, "milestone_id"),
            item_id: opt_str(v, "item_id"),
            data: v.get("data").cloned().unwrap_or(Value::Null),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroupMessage {
    pub seq: u64,
    pub ts: i64,
    pub from: String,
    pub to: String,
    pub body: String,
    pub kind: String,
    pub item_id: Option<String>,
    pub reply_to: Option<u64>,
    pub deliver: bool,
}

impl GroupMessage {
    pub fn from_json(v: &Value) -> Self {
        Self {
            seq: u64_of(v, "seq"),
            ts: i64_of(v, "ts"),
            from: str_of(v, "from"),
            to: str_of(v, "to"),
            body: str_of(v, "body"),
            kind: str_of(v, "kind"),
            item_id: opt_str(v, "item_id"),
            reply_to: v.get("reply_to").and_then(Value::as_u64),
            deliver: bool_of(v, "deliver"),
        }
    }
}

/// Full `GET /api/groups/{id}` response.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroupDetail {
    pub id: String,
    pub path: String,
    pub runtime_notice: Option<String>,
    pub shell_enabled: bool,
    pub project: ProjectInfo,
    pub members: Vec<GroupMember>,
    pub messages: Vec<GroupMessage>,
    pub milestones: Vec<GroupMilestone>,
    pub items: Vec<GroupItem>,
    pub events: Vec<GroupEvent>,
}

impl GroupDetail {
    pub fn from_json(v: &Value) -> Self {
        let list = |key: &str| {
            v.get(key)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        };
        Self {
            id: str_of(v, "id"),
            path: str_of(v, "path"),
            runtime_notice: opt_str(v, "runtime_notice"),
            shell_enabled: bool_of(v, "shell_enabled"),
            project: ProjectInfo::from_json(v),
            members: list("members").iter().map(GroupMember::from_json).collect(),
            messages: list("messages")
                .iter()
                .map(GroupMessage::from_json)
                .collect(),
            milestones: list("milestones")
                .iter()
                .map(GroupMilestone::from_json)
                .collect(),
            items: list("items").iter().map(GroupItem::from_json).collect(),
            events: list("events").iter().map(GroupEvent::from_json).collect(),
        }
    }

    pub fn items_done(&self) -> usize {
        self.items.iter().filter(|i| i.status == "done").count()
    }

    pub fn milestones_done(&self) -> usize {
        self.milestones
            .iter()
            .filter(|m| m.status == "done")
            .count()
    }

    pub fn waiting(&self) -> Vec<&GroupItem> {
        self.items
            .iter()
            .filter(|i| i.awaiting_approval())
            .collect()
    }

    pub fn item(&self, id: &str) -> Option<&GroupItem> {
        self.items.iter().find(|i| i.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_parses_the_live_group_shape() {
        let v: Value = serde_json::from_str(
            r#"{
              "id":"p1","path":"C:/work","shell_enabled":true,
              "runtime_notice":"tick",
              "project":{"status":"running","goal":"Ship it","spent_usd":1.5,"budget_usd":4,"deadline":123},
              "members":[{"name":"member:dev","display":"Dev","kind":"member","model":"m"}],
              "messages":[{"seq":4,"ts":99,"from":"owner","to":"all","body":"hi","kind":"note","item_id":"i1"}],
              "milestones":[{"id":"m1","title":"One","status":"done","depends_on":[]}],
              "items":[{"id":"i1","title":"Export","status":"blocked","blocked_reason":"awaiting_approval","acceptance":["a"]},
                       {"id":"i2","title":"Charts","status":"done"}],
              "events":[{"seq":1,"ts":98,"kind":"item.done","actor":"member:dev","data":{"result":"ok"}}]
            }"#,
        )
        .expect("json");
        let d = GroupDetail::from_json(&v);
        assert_eq!(d.id, "p1");
        assert!(d.shell_enabled);
        assert_eq!(d.project.goal, "Ship it");
        assert_eq!(d.project.budget_usd, Some(4.0));
        assert_eq!(d.members[0].display, "Dev");
        assert_eq!(d.items.len(), 2);
        assert_eq!(d.milestones_done(), 1);
        assert_eq!(d.items_done(), 1);
        assert_eq!(d.waiting().len(), 1);
        assert_eq!(d.item("i1").unwrap().acceptance[0], "a");
        assert_eq!(d.events[0].seq, 1);
    }
}
