//! Tool labels plus the historical names
//! kept for old transcripts.

/// `TOOL_VERBS` + `LEGACY_VERBS`.
pub fn verb(name: &str) -> &str {
    match name {
        "agent" => "Managed agent",
        "apply_patch" => "Patched",
        "ask_user_question" => "Asked",
        "bash" => "Ran command",
        "browser_act" => "Used browser",
        "canvas_mode" => "Used canvas",
        "close_agent" => "Closed agent",
        "create_client" => "Created client",
        "create_docx" => "Created docx",
        "create_group" => "Created group",
        "create_xlsx" => "Created xlsx",
        "delegate" => "Delegated",
        "delegation_status" => "Checked delegation",
        "dfd_bridge" => "Used DFD bridge",
        "edit" => "Edited",
        "files_changed" => "Listed changes",
        "give_note" => "Left note",
        "glob" => "Listed files",
        "grep" => "Searched",
        "group_board" => "Read group board",
        "group_events" => "Read group events",
        "group_item_detail" => "Read group item",
        "group_update" => "Updated group",
        "house_get_device" => "Read house device",
        "house_get_history" => "Read house history",
        "house_get_state" => "Read house state",
        "house_run_scene" => "Ran house scene",
        "house_set_device" => "Set house device",
        "interrupt_agent" => "Interrupted agent",
        "job" => "Managed job",
        "job_cancel" => "Cancelled job",
        "job_message" => "Messaged job",
        "look_at_screen" => "Looked at screen",
        "memory" => "Recalled memory",
        "message_agent" => "Messaged agent",
        "personal_os" => "Used personal OS",
        "read" => "Read",
        "read_pdf" => "Read PDF",
        "run_code" => "Ran code",
        "schedule" => "Scheduled",
        "set_screen_share" => "Set screen share",
        "skill" => "Loaded skill",
        "spawn_agent" => "Spawned agent",
        "tool_search" => "Searched tools",
        "update_plan" => "Updated plan",
        "wait_agent" => "Waited for agent",
        "web_fetch" => "Fetched",
        "web_search" => "Searched web",
        "workspace" => "Managed workspace",
        // LEGACY_VERBS
        "read_image" => "Read image",
        "list_files" => "Listed files",
        "task" => "Delegated",
        "send_message" => "Messaged agent",
        "followup_task" => "Followed up",
        "job_list" => "Listed jobs",
        "job_output" => "Read job",
        "job_kill" => "Killed job",
        "exit_plan_mode" => "Proposed plan",
        "working_directory" => "Changed directory",
        other => other,
    }
}

/// `TOOL_SUMMARIES` + `LEGACY_SUMMARIES`.
pub fn summary(name: &str) -> &str {
    match name {
        "agent" => "managed an agent",
        "apply_patch" => "patched files",
        "ask_user_question" => "asked a question",
        "bash" => "ran commands",
        "browser_act" => "used the browser",
        "canvas_mode" => "used the canvas",
        "close_agent" => "closed an agent",
        "create_client" => "created a client",
        "create_docx" => "created a docx",
        "create_group" => "created a group",
        "create_xlsx" => "created an xlsx",
        "delegate" => "created an agent",
        "delegation_status" => "checked delegations",
        "dfd_bridge" => "used the DFD bridge",
        "edit" => "edited files",
        "files_changed" => "listed changed files",
        "give_note" => "left a note",
        "glob" => "explored files",
        "grep" => "searched code",
        "group_board" => "read the group board",
        "group_events" => "read group events",
        "group_item_detail" => "read a group item",
        "group_update" => "updated a group",
        "house_get_device" => "read a house device",
        "house_get_history" => "read house history",
        "house_get_state" => "read house state",
        "house_run_scene" => "ran a house scene",
        "house_set_device" => "set a house device",
        "interrupt_agent" => "interrupted an agent",
        "job" => "managed a job",
        "job_cancel" => "cancelled a job",
        "job_message" => "messaged a job",
        "look_at_screen" => "looked at the screen",
        "memory" => "recalled memory",
        "message_agent" => "messaged an agent",
        "personal_os" => "managed personal data",
        "read" => "read files",
        "read_pdf" => "read a PDF",
        "run_code" => "ran code",
        "schedule" => "managed schedules",
        "set_screen_share" => "set screen sharing",
        "skill" => "loaded a skill",
        "spawn_agent" => "created an agent",
        "tool_search" => "searched tools",
        "update_plan" => "updated the plan",
        "wait_agent" => "waited for an agent",
        "web_fetch" => "fetched pages",
        "web_search" => "searched the web",
        "workspace" => "managed workspaces",
        // LEGACY_SUMMARIES
        "read_image" => "read images",
        "list_files" => "explored files",
        "task" => "created an agent",
        "send_message" => "messaged an agent",
        "followup_task" => "sent a follow-up",
        "job_list" => "listed jobs",
        "job_output" => "read a job",
        "job_kill" => "killed a job",
        "exit_plan_mode" => "proposed a plan",
        "working_directory" => "changed directory",
        _ => "used tools",
    }
}

/// `ToolIcon` category.
/// Icon family per tool. Grouped so related tools read alike while still
/// looking like the action (terminal, file, bot, globe, ...).
pub fn icon_kind(name: &str) -> IconKind {
    match name {
        "bash" | "run_command" | "ds_shell" | "job" | "job_cancel" | "job_message" | "run_code" => {
            IconKind::Terminal
        }
        "grep" | "search_code" | "tool_search" | "web_search" => IconKind::Search,
        "glob" | "list_files" => IconKind::Folder,
        "read" | "read_image" | "read_file" | "read_pdf" => IconKind::File,
        "write"
        | "write_file"
        | "edit"
        | "edit_file"
        | "apply_patch"
        | "create_docx"
        | "create_xlsx"
        | "update_feature"
        | "update_plan"
        | "update_project_meta" => IconKind::FileEdit,
        "browser_act" | "look_at_screen" | "set_screen_share" => IconKind::Globe,
        "agent" | "delegate" | "spawn_agent" | "message_agent" | "interrupt_agent"
        | "close_agent" | "wait_agent" | "delegation_status" => IconKind::Bot,
        "memory" | "give_note" => IconKind::Database,
        "schedule" => IconKind::Clock,
        "create_group" | "group_board" | "group_events" | "group_item_detail" | "group_update" => {
            IconKind::Checklist
        }
        "dfd_bridge" | "files_changed" => IconKind::GitBranch,
        "house_get_device" | "house_get_history" | "house_get_state" | "house_run_scene"
        | "house_set_device" => IconKind::House,
        "ask_user_question" => IconKind::Message,
        "skill" | "canvas_mode" | "personal_os" | "workspace" => IconKind::Sparkles,
        _ => IconKind::Wrench,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconKind {
    Terminal,
    Search,
    Folder,
    File,
    FileEdit,
    Globe,
    Bot,
    Database,
    Clock,
    Checklist,
    GitBranch,
    House,
    Message,
    Sparkles,
    Wrench,
}

pub fn is_terminal(name: &str) -> bool {
    matches!(name, "bash" | "run_command" | "ds_shell" | "job")
}

/// `isDelegationToolName`.
pub fn is_delegation(name: &str) -> bool {
    matches!(
        name,
        "task" | "call_subagent" | "delegate" | "spawn_agent" | "message_agent"
    )
}

/// `normalizeAgentName`.
pub fn normalize_agent_name(name: &str) -> String {
    let n = name.trim().to_ascii_lowercase();
    match n.as_str() {
        "developer" | "dev" => "Dev".to_string(),
        "analytics" => "Analytics".to_string(),
        "designer" | "ui/ux designer" => "Designer".to_string(),
        "supervisor" | "orchestrator" | "stark" => "Stark".to_string(),
        _ => name.to_string(),
    }
}

/// `delegationTarget`: `(name, task)` from either delegation args shape.
pub fn delegation_target(args: Option<&str>) -> (String, String) {
    let Some(args) = args else {
        return (String::new(), String::new());
    };
    let Ok(serde_json::Value::Object(parsed)) = serde_json::from_str(args) else {
        return (String::new(), String::new());
    };
    if let Some(task_name) = parsed.get("task_name").and_then(|v| v.as_str()) {
        return (
            normalize_agent_name(task_name),
            parsed
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        );
    }
    if parsed.get("name").is_some() || parsed.get("task").is_some() {
        return (
            normalize_agent_name(
                parsed
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default(),
            ),
            parsed
                .get("task")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        );
    }
    if let Some(first) = parsed
        .get("items")
        .and_then(|v| v.as_array())
        .and_then(|items| items.first())
        .and_then(|v| v.as_object())
    {
        return (
            normalize_agent_name(
                first
                    .get("agent")
                    .or_else(|| first.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default(),
            ),
            first
                .get("task")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        );
    }
    (String::new(), String::new())
}

/// `primaryArg` — the compact tool subject. JSON object fields in the reference UI's
/// exact priority order; parallel fan-out summarises agent names.
pub fn primary_arg(args: Option<&str>) -> String {
    let Some(args) = args else {
        return String::new();
    };
    match serde_json::from_str::<serde_json::Value>(args) {
        Ok(serde_json::Value::Object(map)) => {
            if let Some(items) = map.get("items").and_then(|v| v.as_array()) {
                if !items.is_empty() {
                    let names: Vec<String> = items
                        .iter()
                        .filter_map(|item| {
                            item.get("agent")
                                .or_else(|| item.get("name"))
                                .and_then(|v| v.as_str())
                                .filter(|s| !s.is_empty())
                                .map(ToString::to_string)
                        })
                        .collect();
                    let text = if names.is_empty() {
                        format!("{} agents", items.len())
                    } else {
                        names.join(", ")
                    };
                    return truncate(&text);
                }
            }
            for key in [
                "file_path",
                "path",
                "command",
                "cmd",
                "pattern",
                "name",
                "task",
                "task_name",
                "target",
                "job_id",
                "url",
                "query",
                "queries",
                "prompt",
                "message",
            ] {
                if let Some(value) = map.get(key) {
                    let text = match value {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Null => String::new(),
                        other => other.to_string(),
                    };
                    if !text.is_empty() {
                        return truncate(&text);
                    }
                }
            }
            String::new()
        }
        Ok(_) => String::new(),
        Err(_) => truncate(args.trim()),
    }
}

fn truncate(text: &str) -> String {
    if text.chars().count() > 72 {
        let mut out: String = text.chars().take(72).collect();
        out.push('…');
        out
    } else {
        text.to_string()
    }
}

/// `toolGroupTitle`: unique summary parts, at most two, title-cased.
pub fn group_title(names: &[&str]) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for name in names {
        let summary = summary(name);
        if !parts.contains(&summary) {
            parts.push(summary);
        }
    }
    let text = match parts.len() {
        0 => "Used tools".to_string(),
        1 | 2 => parts.join(", "),
        _ => format!("{}, {}, and more", parts[0], parts[1]),
    };
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => "Used tools".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_arg_priority_and_truncation() {
        assert_eq!(
            primary_arg(Some(r#"{"command":"ls","path":"src/lib.rs"}"#)),
            "src/lib.rs"
        );
        assert_eq!(primary_arg(Some(r#"{"pattern":"needle"}"#)), "needle");
        let long = "x".repeat(80);
        assert_eq!(
            primary_arg(Some(&format!(r#"{{"name":"{long}"}}"#)))
                .chars()
                .count(),
            73
        );
    }

    #[test]
    fn fan_out_uses_agent_names() {
        assert_eq!(
            primary_arg(Some(r#"{"items":[{"agent":"A"},{"agent":"B"}]}"#)),
            "A, B"
        );
    }

    #[test]
    fn group_title_unique_and_capped() {
        assert_eq!(
            group_title(&["grep", "glob", "read"]),
            "Searched code, explored files, and more"
        );
        assert_eq!(group_title(&["read", "read"]), "Read files");
    }
}
