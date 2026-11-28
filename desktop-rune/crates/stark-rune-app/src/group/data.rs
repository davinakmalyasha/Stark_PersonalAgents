//! Group-project detail models — the `GroupProjectDetail` wire shape from
//! `stark-server`, parsed once into owned Rust values.

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

fn list_of<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroupUsage {
    pub runs: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub failed_runs: u64,
}

impl GroupUsage {
    pub fn from_json(v: &Value) -> Self {
        Self {
            runs: u64_of(v, "runs"),
            input_tokens: u64_of(v, "input_tokens"),
            output_tokens: u64_of(v, "output_tokens"),
            cost_usd: f64_of(v, "cost_usd").unwrap_or(0.0),
            failed_runs: u64_of(v, "failed_runs"),
        }
    }

    pub fn tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroupSchedule {
    pub start_mode: String,
    pub start_at: Option<f64>,
    pub next_run_at: Option<f64>,
    pub recurrence: String,
    pub repeat_active: bool,
}

impl GroupSchedule {
    pub fn from_json(v: &Value) -> Self {
        let n = v.get("schedule").unwrap_or(v);
        Self {
            start_mode: str_of(n, "start_mode"),
            start_at: f64_of(n, "start_at"),
            next_run_at: f64_of(n, "next_run_at"),
            recurrence: str_of(n, "recurrence"),
            repeat_active: bool_of(n, "repeat_active"),
        }
    }

    pub fn repeat_label(&self) -> String {
        if !self.repeat_active {
            return "No repeat".to_string();
        }
        match self.recurrence.as_str() {
            "daily" => "Daily".to_string(),
            "weekly" => "Weekly".to_string(),
            "monthly" => "Monthly".to_string(),
            _ => "No repeat".to_string(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CycleInfo {
    pub cycle: i64,
    pub milestones_total: u64,
    pub milestones_done: u64,
}

impl CycleInfo {
    pub fn from_json(v: &Value) -> Self {
        Self {
            cycle: i64_of(v, "cycle"),
            milestones_total: u64_of(v, "milestones_total"),
            milestones_done: u64_of(v, "milestones_done"),
        }
    }
}

/// `project` envelope in the detail response.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectInfo {
    pub id: String,
    pub title: String,
    pub status: String,
    pub goal: String,
    pub cycle: i64,
    pub workspace_path: String,
    pub origin_workspace_id: String,
    pub plan_status: String,
    pub plan_review: bool,
    pub planner_agent: String,
    pub max_parallel: i64,
    pub spent_usd: Option<f64>,
    pub budget_usd: Option<f64>,
    pub deadline: Option<f64>,
    pub halt_reason: Option<String>,
    pub last_error: Option<String>,
    pub schedule: GroupSchedule,
    pub usage: GroupUsage,
    pub cycles: Vec<CycleInfo>,
}

impl ProjectInfo {
    pub fn from_json(v: &Value) -> Self {
        let n = v.get("project").unwrap_or(v);
        Self {
            id: str_of(n, "id"),
            title: str_of(n, "title"),
            status: str_of(n, "status"),
            goal: str_of(n, "goal"),
            cycle: i64_of(n, "cycle"),
            workspace_path: str_of(n, "workspace_path"),
            origin_workspace_id: str_of(n, "origin_workspace_id"),
            plan_status: str_of(n, "plan_status"),
            plan_review: bool_of(n, "plan_review"),
            planner_agent: str_of(n, "planner_agent"),
            max_parallel: i64_of(n, "max_parallel"),
            spent_usd: f64_of(n, "spent_usd"),
            budget_usd: f64_of(n, "budget_usd"),
            deadline: f64_of(n, "deadline"),
            halt_reason: opt_str(n, "halt_reason"),
            last_error: opt_str(n, "last_error"),
            schedule: GroupSchedule::from_json(n),
            usage: GroupUsage::from_json(n.get("usage").unwrap_or(&Value::Null)),
            cycles: list_of(n, "cycles")
                .iter()
                .map(CycleInfo::from_json)
                .collect(),
        }
    }

    pub fn display_title(&self) -> String {
        if self.title.trim().is_empty() {
            self.goal.clone()
        } else {
            self.title.clone()
        }
    }

    pub fn is_plan_ready(&self) -> bool {
        self.status == "plan_ready"
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroupMember {
    pub id: String,
    pub name: String,
    pub display: String,
    pub agent_key: String,
    pub kind: String,
    pub model: String,
    pub slot_index: i64,
    pub status: String,
    pub current_item: Option<String>,
    pub active_chat: Option<String>,
    pub active_run_id: Option<String>,
    pub assigned: u64,
    pub done: u64,
    pub failed: u64,
    pub running: bool,
    pub tokens: u64,
    pub cost_usd: f64,
    pub last_active: Option<f64>,
}

impl GroupMember {
    pub fn from_json(v: &Value) -> Self {
        let usage = v.get("usage").cloned().unwrap_or_default();
        let active = v.get("active_run").cloned().unwrap_or_default();
        Self {
            id: str_of(v, "id"),
            name: str_of(v, "name"),
            display: str_of(v, "display"),
            agent_key: str_of(v, "agent_key"),
            kind: str_of(v, "kind"),
            model: str_of(v, "model"),
            slot_index: i64_of(v, "slot_index"),
            status: str_of(v, "status"),
            current_item: opt_str(v, "current_item").or_else(|| opt_str(&active, "item_title")),
            active_chat: opt_str(&active, "chat_id"),
            active_run_id: opt_str(&active, "id"),
            assigned: u64_of(v, "assigned"),
            done: u64_of(v, "done"),
            failed: u64_of(v, "failed"),
            running: u64_of(v, "running") > 0,
            tokens: u64_of(&usage, "tokens"),
            cost_usd: f64_of(&usage, "cost_usd").unwrap_or(0.0),
            last_active: f64_of(&usage, "last_active"),
        }
    }

    pub fn label(&self) -> String {
        if self.display.is_empty() {
            self.name.clone()
        } else {
            self.display.clone()
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroupMilestone {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub status: String,
    pub seq_no: i64,
    pub cycle: i64,
    pub depends_on: Vec<String>,
    pub acceptance: Value,
    pub owner_member_id: Option<String>,
    pub due_at: Option<f64>,
    pub updated_at: i64,
}

impl GroupMilestone {
    pub fn from_json(v: &Value) -> Self {
        Self {
            id: str_of(v, "id"),
            title: str_of(v, "title"),
            detail: str_of(v, "detail"),
            status: str_of(v, "status"),
            seq_no: i64_of(v, "seq_no"),
            cycle: i64_of(v, "cycle"),
            depends_on: list_of(v, "depends_on")
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
            acceptance: v.get("acceptance").cloned().unwrap_or(Value::Null),
            owner_member_id: opt_str(v, "owner_member_id"),
            due_at: f64_of(v, "due_at"),
            updated_at: i64_of(v, "updated_at"),
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
    pub cycle: i64,
    pub attempts: u64,
    pub max_attempts: u64,
    pub owner_member: Option<String>,
    pub owner_member_id: Option<String>,
    pub owner_display: Option<String>,
    pub lease_owner: Option<String>,
    pub cost_tokens: f64,
    pub cost_usd: Option<f64>,
    pub priority: i64,
    pub due_at: Option<f64>,
    pub requires_approval: bool,
    pub result: Option<String>,
    pub error: Option<String>,
    pub blocked_reason: Option<String>,
    pub invalidated_reason: Option<String>,
    pub acceptance: Value,
    pub tags: Value,
    pub run_id: Option<String>,
    pub started_at: Option<f64>,
    pub finished_at: Option<f64>,
    pub next_attempt_at: Option<f64>,
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
            cycle: i64_of(v, "cycle"),
            attempts: u64_of(v, "attempts"),
            max_attempts: u64_of(v, "max_attempts"),
            owner_member: opt_str(v, "owner_member"),
            owner_member_id: opt_str(v, "owner_member_id"),
            owner_display: opt_str(v, "owner_display"),
            lease_owner: opt_str(v, "lease_owner"),
            cost_tokens: f64_of(v, "cost_tokens").unwrap_or(0.0),
            cost_usd: f64_of(v, "cost_usd"),
            priority: i64_of(v, "priority"),
            due_at: f64_of(v, "due_at"),
            requires_approval: bool_of(v, "requires_approval"),
            result: opt_str(v, "result"),
            error: opt_str(v, "error"),
            blocked_reason: opt_str(v, "blocked_reason"),
            invalidated_reason: opt_str(v, "invalidated_reason"),
            acceptance: v.get("acceptance").cloned().unwrap_or(Value::Null),
            tags: v.get("tags").cloned().unwrap_or(Value::Null),
            run_id: opt_str(v, "run_id"),
            started_at: f64_of(v, "started_at"),
            finished_at: f64_of(v, "finished_at"),
            next_attempt_at: f64_of(v, "next_attempt_at"),
            updated_at: i64_of(v, "updated_at"),
        }
    }

    pub fn awaiting_approval(&self) -> bool {
        self.status == "awaiting_approval"
            || self.blocked_reason.as_deref() == Some("awaiting_approval")
    }

    pub fn owner_label(&self) -> String {
        self.owner_display
            .clone()
            .or_else(|| self.owner_member.clone())
            .unwrap_or_default()
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
    pub run_id: Option<String>,
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
            run_id: opt_str(v, "run_id"),
            reply_to: v.get("reply_to").and_then(Value::as_u64),
            deliver: bool_of(v, "deliver"),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroupRun {
    pub id: String,
    pub cycle: i64,
    pub item_id: Option<String>,
    pub item_title: Option<String>,
    pub member_id: Option<String>,
    pub member: Option<String>,
    pub agent_key: Option<String>,
    pub chat_id: Option<String>,
    pub run_id: Option<String>,
    pub kind: String,
    pub status: String,
    pub started_at: Option<f64>,
    pub finished_at: Option<f64>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub report: Option<String>,
    pub error: Option<String>,
}

impl GroupRun {
    pub fn from_json(v: &Value) -> Self {
        Self {
            id: str_of(v, "id"),
            cycle: i64_of(v, "cycle"),
            item_id: opt_str(v, "item_id"),
            item_title: opt_str(v, "item_title"),
            member_id: opt_str(v, "member_id"),
            member: opt_str(v, "member"),
            agent_key: opt_str(v, "agent_key"),
            chat_id: opt_str(v, "chat_id"),
            run_id: opt_str(v, "run_id"),
            kind: str_of(v, "kind"),
            status: str_of(v, "status"),
            started_at: f64_of(v, "started_at"),
            finished_at: f64_of(v, "finished_at"),
            input_tokens: u64_of(v, "input_tokens"),
            output_tokens: u64_of(v, "output_tokens"),
            cost_usd: f64_of(v, "cost_usd").unwrap_or(0.0),
            report: opt_str(v, "report"),
            error: opt_str(v, "error"),
        }
    }

    pub fn tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}

/// Full `GET /api/groups/{id}` response.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroupDetail {
    pub id: String,
    pub path: String,
    pub workspace_path: String,
    pub workspace_missing: bool,
    pub runtime_notice: Option<String>,
    pub shell_enabled: bool,
    pub current_cycle: i64,
    pub plan_status: String,
    pub project: ProjectInfo,
    pub members: Vec<GroupMember>,
    pub messages: Vec<GroupMessage>,
    pub milestones: Vec<GroupMilestone>,
    pub items: Vec<GroupItem>,
    pub events: Vec<GroupEvent>,
    pub runs: Vec<GroupRun>,
    pub usage: GroupUsage,
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
            workspace_path: str_of(v, "workspace_path"),
            workspace_missing: bool_of(v, "workspace_missing"),
            runtime_notice: opt_str(v, "runtime_notice"),
            shell_enabled: bool_of(v, "shell_enabled"),
            current_cycle: i64_of(v, "current_cycle"),
            plan_status: str_of(v, "plan_status"),
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
            runs: list("runs").iter().map(GroupRun::from_json).collect(),
            usage: GroupUsage::from_json(v.get("usage").unwrap_or(&Value::Null)),
        }
    }

    pub fn items_done(&self) -> usize {
        self.current_items().filter(|i| i.status == "done").count()
    }

    pub fn milestones_done(&self) -> usize {
        self.current_milestones()
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

    pub fn current_items(&self) -> impl Iterator<Item = &GroupItem> {
        let cycle = if self.current_cycle > 0 {
            self.current_cycle
        } else {
            1
        };
        self.items.iter().filter(move |item| item.cycle == cycle)
    }

    pub fn current_milestones(&self) -> impl Iterator<Item = &GroupMilestone> {
        let cycle = if self.current_cycle > 0 {
            self.current_cycle
        } else {
            1
        };
        self.milestones.iter().filter(move |m| m.cycle == cycle)
    }

    pub fn items_for(&self, milestone_id: &str) -> Vec<&GroupItem> {
        self.current_items()
            .filter(|item| item.milestone_id.as_deref() == Some(milestone_id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_parses_the_live_group_shape() {
        let v: Value = serde_json::from_str(
            r#"{
              "id":"p1","path":"C:/meta","workspace_path":"C:/work","shell_enabled":true,
              "runtime_notice":"tick","current_cycle":2,"plan_status":"ready",
              "project":{"id":"p1","title":"Ship it","status":"running","goal":"Ship it","cycle":2,
                "spent_usd":1.5,"budget_usd":4,"deadline":123,"max_parallel":2,
                "schedule":{"start_mode":"scheduled","recurrence":"daily","repeat_active":true,"next_run_at":999},
                "usage":{"runs":3,"input_tokens":10,"output_tokens":5,"cost_usd":0.2},
                "cycles":[{"cycle":2,"milestones_total":2,"milestones_done":1}]},
              "members":[{"id":"m1","name":"Dev","display":"Developer 1","agent_key":"developer","slot_index":1,"assigned":2,"done":1,"active_run":{"chat_id":"c1","id":"r1","item_title":"Export"}}],
              "messages":[{"seq":4,"ts":99,"from":"owner","to":"all","body":"hi","kind":"note","item_id":"i1","run_id":"r1"}],
              "milestones":[{"id":"m1","title":"One","status":"done","cycle":2,"depends_on":[]}],
              "items":[{"id":"i1","title":"Export","status":"awaiting_approval","cycle":2,"owner_member_id":"m1","acceptance":["a"]},
                       {"id":"i2","title":"Charts","status":"done","cycle":2}],
              "runs":[{"id":"r1","cycle":2,"kind":"item","status":"complete","input_tokens":10,"output_tokens":5,"cost_usd":0.2}],
              "events":[{"seq":1,"ts":98,"kind":"item.done","actor":"member:dev","data":{"result":"ok"}}]
            }"#,
        )
        .expect("json");
        let d = GroupDetail::from_json(&v);
        assert_eq!(d.id, "p1");
        assert_eq!(d.workspace_path, "C:/work");
        assert!(d.shell_enabled);
        assert_eq!(d.project.display_title(), "Ship it");
        assert_eq!(d.project.schedule.repeat_label(), "Daily");
        assert_eq!(d.project.budget_usd, Some(4.0));
        assert_eq!(d.project.max_parallel, 2);
        assert_eq!(d.members[0].display, "Developer 1");
        assert_eq!(d.members[0].current_item.as_deref(), Some("Export"));
        assert_eq!(d.items.len(), 2);
        assert_eq!(d.milestones_done(), 1);
        assert_eq!(d.items_done(), 1);
        assert_eq!(d.waiting().len(), 1);
        assert_eq!(d.item("i1").unwrap().acceptance[0], "a");
        assert_eq!(d.events[0].seq, 1);
        assert_eq!(d.runs[0].tokens(), 15);
        assert_eq!(d.project.cycles[0].cycle, 2);
    }

    #[test]
    fn current_cycle_filters_history() {
        let v = serde_json::json!({
            "id":"p","current_cycle":2,
            "project":{"cycle":2},
            "milestones":[
                {"id":"m1","title":"old","cycle":1,"status":"done"},
                {"id":"m2","title":"new","cycle":2,"status":"todo"}
            ],
            "items":[
                {"id":"i1","milestone_id":"m1","cycle":1,"status":"done"},
                {"id":"i2","milestone_id":"m2","cycle":2,"status":"todo"}
            ]
        });
        let d = GroupDetail::from_json(&v);
        assert_eq!(d.current_milestones().count(), 1);
        assert_eq!(d.current_items().count(), 1);
        assert_eq!(d.items_done(), 0);
    }
}
