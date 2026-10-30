//! Unified built-in tool inventory: one entry per tool.
//!
//! This is the single source of truth for tool metadata. The contract JSON
//! (`backend-rs/specs/tool-specs.json`) stays frozen as the wire schema, but
//! every Rust-side list derives from here:
//! - `IMPLEMENTED` membership -> [`is_implemented`]
//! - plan-mode write guard -> [`is_plan_write`]
//! - read-only access guard -> [`is_read_blocked`]
//! - frontend verb/summary labels -> `tool-labels.json` (generated, see test)
//!
//! Adding a tool = add one `ToolMeta` entry here, add the contract JSON
//! entry, add the `dispatch` arm, regenerate labels. Tests fail loudly if
//! any of those steps is missed.

pub struct ToolMeta {
    pub name: &'static str,
    /// Blocked at dispatch in plan mode (`PLAN_WRITE_TOOLS` semantics).
    pub plan_write: bool,
    /// Unconditionally blocked for `access == "read"` agents. Conditional
    /// guards (bash/job/browser_act/memory/agent) stay in `lib.rs`.
    pub read_blocked: bool,
    /// Callable from inside a `run_code` program (Codex `allowed_callers`
    /// `programmatic`). Direct-only tools (subagents, user questions, jobs,
    /// schedules, canvas, `run_code` itself) must be called directly.
    pub program: bool,
    /// Short past-tense verb for the activity timeline.
    pub verb: &'static str,
    /// Gerund phrase for grouped summaries ("read files").
    pub summary: &'static str,
}

pub const INVENTORY: &[ToolMeta] = &[
    ToolMeta {
        name: "read_file",
        plan_write: false,
        read_blocked: false,
        program: true,
        verb: "Read",
        summary: "read files",
    },
    ToolMeta {
        name: "edit_file",
        plan_write: true,
        read_blocked: true,
        program: true,
        verb: "Edited",
        summary: "edited files",
    },
    ToolMeta {
        name: "run_command",
        plan_write: false,
        read_blocked: false,
        program: true,
        verb: "Ran command",
        summary: "ran commands",
    },
    ToolMeta {
        name: "search_code",
        plan_write: false,
        read_blocked: false,
        program: true,
        verb: "Searched",
        summary: "searched code",
    },
    ToolMeta {
        name: "ask_user",
        plan_write: false,
        read_blocked: false,
        program: false,
        verb: "Asked",
        summary: "asked a question",
    },
    ToolMeta {
        name: "call_subagent",
        plan_write: true,
        read_blocked: true,
        program: false,
        verb: "Delegated",
        summary: "created an agent",
    },
    ToolMeta {
        name: "update_plan",
        plan_write: false,
        read_blocked: false,
        program: true,
        verb: "Updated plan",
        summary: "updated the plan",
    },
    ToolMeta {
        name: "browser_act",
        plan_write: false,
        read_blocked: false,
        program: true,
        verb: "Used browser",
        summary: "used the browser",
    },
    ToolMeta {
        name: "delegate",
        plan_write: true,
        read_blocked: false,
        program: false,
        verb: "Delegated",
        summary: "created an agent",
    },
    ToolMeta {
        name: "delegation_status",
        plan_write: false,
        read_blocked: false,
        program: true,
        verb: "Checked delegation",
        summary: "checked delegations",
    },
    ToolMeta {
        name: "job_message",
        plan_write: false,
        read_blocked: false,
        program: false,
        verb: "Messaged job",
        summary: "messaged a job",
    },
    ToolMeta {
        name: "job_cancel",
        plan_write: false,
        read_blocked: false,
        program: false,
        verb: "Cancelled job",
        summary: "cancelled a job",
    },
    ToolMeta {
        name: "give_note",
        plan_write: false,
        read_blocked: true,
        program: true,
        verb: "Left note",
        summary: "left a note",
    },
    ToolMeta {
        name: "house_get_state",
        plan_write: false,
        read_blocked: false,
        program: true,
        verb: "Read house state",
        summary: "read house state",
    },
    ToolMeta {
        name: "house_get_device",
        plan_write: false,
        read_blocked: false,
        program: true,
        verb: "Read house device",
        summary: "read a house device",
    },
    ToolMeta {
        name: "house_set_device",
        plan_write: true,
        read_blocked: true,
        program: true,
        verb: "Set house device",
        summary: "set a house device",
    },
    ToolMeta {
        name: "house_run_scene",
        plan_write: true,
        read_blocked: true,
        program: true,
        verb: "Ran house scene",
        summary: "ran a house scene",
    },
    ToolMeta {
        name: "house_get_history",
        plan_write: false,
        read_blocked: false,
        program: true,
        verb: "Read house history",
        summary: "read house history",
    },
    ToolMeta {
        name: "dfd_bridge",
        plan_write: false,
        read_blocked: false,
        program: true,
        verb: "Used DFD bridge",
        summary: "used the DFD bridge",
    },
    ToolMeta {
        name: "create_client",
        plan_write: true,
        read_blocked: true,
        program: true,
        verb: "Created client",
        summary: "created a client",
    },
    ToolMeta {
        name: "files_changed",
        plan_write: false,
        read_blocked: false,
        program: true,
        verb: "Listed changes",
        summary: "listed changed files",
    },
    ToolMeta {
        name: "read_pdf",
        plan_write: false,
        read_blocked: false,
        program: true,
        verb: "Read PDF",
        summary: "read a PDF",
    },
    ToolMeta {
        name: "create_docx",
        plan_write: true,
        read_blocked: true,
        program: true,
        verb: "Created docx",
        summary: "created a docx",
    },
    ToolMeta {
        name: "create_xlsx",
        plan_write: true,
        read_blocked: true,
        program: true,
        verb: "Created xlsx",
        summary: "created an xlsx",
    },
    ToolMeta {
        name: "create_coding_project",
        plan_write: true,
        read_blocked: true,
        program: true,
        verb: "Created project",
        summary: "created a project",
    },
    ToolMeta {
        name: "create_group",
        plan_write: true,
        read_blocked: true,
        program: true,
        verb: "Created group",
        summary: "created a group",
    },
    ToolMeta {
        name: "group_board",
        plan_write: false,
        read_blocked: false,
        program: true,
        verb: "Read group board",
        summary: "read the group board",
    },
    ToolMeta {
        name: "group_item_detail",
        plan_write: false,
        read_blocked: false,
        program: true,
        verb: "Read group item",
        summary: "read a group item",
    },
    ToolMeta {
        name: "group_events",
        plan_write: false,
        read_blocked: false,
        program: true,
        verb: "Read group events",
        summary: "read group events",
    },
    ToolMeta {
        name: "group_update",
        plan_write: true,
        read_blocked: true,
        program: true,
        verb: "Updated group",
        summary: "updated a group",
    },
    ToolMeta {
        name: "schedule",
        plan_write: false,
        read_blocked: true,
        program: false,
        verb: "Scheduled",
        summary: "managed schedules",
    },
    ToolMeta {
        name: "look_at_screen",
        plan_write: true,
        read_blocked: true,
        program: true,
        verb: "Looked at screen",
        summary: "looked at the screen",
    },
    ToolMeta {
        name: "set_screen_share",
        plan_write: true,
        read_blocked: true,
        program: true,
        verb: "Set screen share",
        summary: "set screen sharing",
    },
    ToolMeta {
        name: "canvas_mode",
        plan_write: false,
        read_blocked: true,
        program: false,
        verb: "Used canvas",
        summary: "used the canvas",
    },
    ToolMeta {
        name: "run_code",
        plan_write: true,
        read_blocked: true,
        program: false,
        verb: "Ran code",
        summary: "ran code",
    },
    ToolMeta {
        name: "agent",
        plan_write: true,
        read_blocked: false,
        program: true,
        verb: "Managed agent",
        summary: "managed an agent",
    },
    ToolMeta {
        name: "bash",
        plan_write: false,
        read_blocked: false,
        program: true,
        verb: "Ran command",
        summary: "ran commands",
    },
    ToolMeta {
        name: "job",
        plan_write: false,
        read_blocked: false,
        program: true,
        verb: "Managed job",
        summary: "managed a job",
    },
    ToolMeta {
        name: "read",
        plan_write: false,
        read_blocked: false,
        program: true,
        verb: "Read",
        summary: "read files",
    },
    ToolMeta {
        name: "write",
        plan_write: true,
        read_blocked: true,
        program: true,
        verb: "Wrote",
        summary: "wrote files",
    },
    ToolMeta {
        name: "edit",
        plan_write: true,
        read_blocked: true,
        program: true,
        verb: "Edited",
        summary: "edited files",
    },
    ToolMeta {
        name: "glob",
        plan_write: false,
        read_blocked: false,
        program: true,
        verb: "Listed files",
        summary: "explored files",
    },
    ToolMeta {
        name: "grep",
        plan_write: false,
        read_blocked: false,
        program: true,
        verb: "Searched",
        summary: "searched code",
    },
    ToolMeta {
        name: "todo_write",
        plan_write: true,
        read_blocked: true,
        program: true,
        verb: "Updated todo list",
        summary: "updated the todo list",
    },
    ToolMeta {
        name: "ask_user_question",
        plan_write: false,
        read_blocked: false,
        program: false,
        verb: "Asked",
        summary: "asked a question",
    },
    ToolMeta {
        name: "skill",
        plan_write: false,
        read_blocked: false,
        program: true,
        verb: "Loaded skill",
        summary: "loaded a skill",
    },
    ToolMeta {
        name: "web_search",
        plan_write: false,
        read_blocked: false,
        program: true,
        verb: "Searched web",
        summary: "searched the web",
    },
    ToolMeta {
        name: "web_fetch",
        plan_write: false,
        read_blocked: false,
        program: true,
        verb: "Fetched",
        summary: "fetched pages",
    },
    ToolMeta {
        name: "spawn_agent",
        plan_write: true,
        read_blocked: true,
        program: false,
        verb: "Spawned agent",
        summary: "created an agent",
    },
    ToolMeta {
        name: "message_agent",
        plan_write: true,
        read_blocked: true,
        program: false,
        verb: "Messaged agent",
        summary: "messaged an agent",
    },
    ToolMeta {
        name: "wait_agent",
        plan_write: false,
        read_blocked: false,
        program: false,
        verb: "Waited for agent",
        summary: "waited for an agent",
    },
    ToolMeta {
        name: "list_agents",
        plan_write: false,
        read_blocked: false,
        program: false,
        verb: "Listed agents",
        summary: "listed agents",
    },
    ToolMeta {
        name: "interrupt_agent",
        plan_write: true,
        read_blocked: true,
        program: false,
        verb: "Interrupted agent",
        summary: "interrupted an agent",
    },
    ToolMeta {
        name: "close_agent",
        plan_write: true,
        read_blocked: true,
        program: false,
        verb: "Closed agent",
        summary: "closed an agent",
    },
    ToolMeta {
        name: "memory",
        plan_write: false,
        read_blocked: false,
        program: true,
        verb: "Recalled memory",
        summary: "recalled memory",
    },
];

pub fn find(name: &str) -> Option<&'static ToolMeta> {
    INVENTORY.iter().find(|tool| tool.name == name)
}

pub fn is_implemented(name: &str) -> bool {
    find(name).is_some()
}

/// Names that are plan-blocked but have no implemented handler (defensive:
/// `create_project` predates `create_coding_project`; `github_apply_gate` is
/// intentionally unexposed and returns 404 by design).
const PLAN_WRITE_EXTRA: &[&str] = &["create_project", "github_apply_gate"];

pub fn is_plan_write(name: &str) -> bool {
    find(name).is_some_and(|tool| tool.plan_write) || PLAN_WRITE_EXTRA.contains(&name)
}

pub fn is_read_blocked(name: &str) -> bool {
    find(name).is_some_and(|tool| tool.read_blocked)
}

pub fn is_program(name: &str) -> bool {
    find(name).is_some_and(|tool| tool.program)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn no_duplicate_names() {
        let mut seen = HashSet::new();
        for tool in INVENTORY {
            assert!(
                seen.insert(tool.name),
                "duplicate inventory entry: {}",
                tool.name
            );
        }
    }

    #[test]
    fn matches_exposed_contract() {
        let file = crate::contract();
        let exposed: Vec<&str> = file
            .tools
            .iter()
            .filter(|tool| tool.exposed)
            .map(|tool| tool.name.as_str())
            .collect();
        assert_eq!(
            exposed.len(),
            INVENTORY.len(),
            "inventory drifted from exposed contract tools"
        );
        for tool in INVENTORY {
            assert!(
                exposed.contains(&tool.name),
                "inventory tool missing from contract: {}",
                tool.name
            );
        }
        for name in &exposed {
            assert!(
                find(name).is_some(),
                "exposed contract tool missing from inventory: {name}"
            );
        }
    }

    #[test]
    fn guard_quirks_preserved() {
        // delegate is plan-guarded but callable in read-only mode (core handles it).
        assert!(is_plan_write("delegate") && !is_read_blocked("delegate"));
        // schedule is read-blocked but allowed in plan mode.
        assert!(is_read_blocked("schedule") && !is_plan_write("schedule"));
        // Defensive plan blocks for names with no handler.
        assert!(is_plan_write("create_project") && !is_implemented("create_project"));
        assert!(is_plan_write("github_apply_gate") && !is_implemented("github_apply_gate"));
        // Standard write tool and read tool.
        assert!(is_plan_write("write") && is_read_blocked("write"));
        assert!(!is_plan_write("read") && !is_read_blocked("read"));
    }

    #[test]
    fn program_flags_match_legacy_direct_only_set() {
        // Previously hardcoded DISALLOWED in codemode.rs; now data.
        let direct_only = [
            "run_code",
            "ask_user",
            "ask_user_question",
            "delegate",
            "call_subagent",
            "spawn_agent",
            "message_agent",
            "wait_agent",
            "list_agents",
            "interrupt_agent",
            "close_agent",
            "job_message",
            "job_cancel",
            "schedule",
            "canvas_mode",
        ];
        for tool in INVENTORY {
            assert_eq!(
                is_program(tool.name),
                !direct_only.contains(&tool.name),
                "program flag drift for {}",
                tool.name
            );
        }
    }

    #[test]
    fn generated_frontend_labels_match() {
        let path = format!(
            "{}/../../../frontend/src/utils/toolLabels.generated.ts",
            env!("CARGO_MANIFEST_DIR")
        );
        let text =
            std::fs::read_to_string(&path).expect("run node backend-rs/specs/generate-labels.mjs");
        for tool in INVENTORY {
            assert!(
                text.contains(&format!("{}: '{}'", tool.name, tool.verb)),
                "verb drift for {} (regenerate labels)",
                tool.name
            );
            assert!(
                text.contains(&format!("{}: '{}'", tool.name, tool.summary)),
                "summary drift for {} (regenerate labels)",
                tool.name
            );
        }
    }
}
