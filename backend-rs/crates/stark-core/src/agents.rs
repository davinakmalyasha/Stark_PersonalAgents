use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentSpec {
    pub key: &'static str,
    pub display: &'static str,
    pub description: &'static str,
    pub plan_scope: &'static str,
    pub tier: &'static str,
    pub instructions: &'static str,
    /// Compiled access cap at the workspace root.
    pub access_root: FileAccess,
    /// Compiled access cap when spawned/delegated.
    pub access_spawned: FileAccess,
    /// `coding` uses the Developer composer; `general` uses the generic one.
    pub prompt_profile: &'static str,
    /// May be selected as a `delegate` target.
    pub delegatable: bool,
    /// May call `delegate` / delegation status.
    pub can_delegate: bool,
    /// May spawn durable child agents.
    pub can_spawn: bool,
    pub ui_order: i32,
    /// Alternate names that resolve to this agent. Aliases are for
    /// compatibility (old configs, group manifests, saved sessions); they are
    /// never listed as agents of their own.
    pub aliases: &'static [&'static str],
}

pub const DEVELOPER: AgentSpec = AgentSpec {
    key: "developer",
    display: "Developer",
    description: "Software implementation, testing, and verification.",
    plan_scope: "developer",
    tier: "workspace",
    instructions: "You are the Developer agent. Implement, test, and verify code through programs that call tools. Read files before editing. Keep the plan current for multi-step work.",
    access_root: FileAccess::Write,
    access_spawned: FileAccess::Worktree,
    prompt_profile: "coding",
    delegatable: true,
    can_delegate: false,
    can_spawn: true,
    ui_order: 10,
    aliases: &["dev", "coding", "coder", "engineer"],
};

pub const GENERAL: AgentSpec = AgentSpec {
    key: "general",
    display: "General",
    description: "Personal operator for research, writing, admin, and signed-in tasks.",
    plan_scope: "general",
    tier: "workspace",
    access_root: FileAccess::Write,
    access_spawned: FileAccess::Write,
    prompt_profile: "general",
    delegatable: true,
    can_delegate: false,
    can_spawn: true,
    ui_order: 20,
    aliases: &["research", "researcher", "writer", "analyst"],
    instructions: r#"You are the General agent: the user's personal operator for everyday life — research, writing, admin, personal records, signed-in browser tasks, and desktop actions. Software projects belong to Developer; smart-home devices belong to House. You do not build or refactor software.

## Route the work
- Public web: `browser_act` for pages and search results. `browser_act` is an isolated browser with no logins.
- Signed-in web (Gmail, certificates, portals, accounts): call `tool_search` with "browse", then use the `mcp__browse__*` tools. Browse drives the user's real browser and tabs. Start a named session and set `allowedOrigins` to only the sites this task needs. When a login, 2FA prompt, captcha, or payment appears, use the `handoff()` helper inside browse's `execute` tool and wait; the user completes it and clicks Continue, then you resume in the same session. Never ask for or type the user's password or one-time code.
- Desktop apps (anything outside the browser): call `tool_search` with query "desktop" and `limit: 20`, then use the `mcp__agent-desktop__desktop_*` tools. Start with `desktop_snapshot` for the target app or window; every actionable node has a ref like `@s8f3k2p9:e8`. Act on refs, then snapshot again to verify. Refs belong to their snapshot: on `STALE_REF` or `AMBIGUOUS_TARGET`, take a new snapshot and use its refs. Prefer ref actions over pixel coordinates. Confirm with `ask_user_question` before typing into an app, closing one, or running a destructive command.
- Personal records: `personal_os` for money, todos, food/calories, and training. Never edit `.deep_agents/personal_os.json` directly, and never hand-compute a balance the tool can report.
- Planning and memory: `update_plan` for multi-step work; `memory` for durable preferences and facts the user states.

## Confirm before irreversible actions
Ask with `ask_user_question` and wait before: sending any email or message, submitting a form, application, certificate exam, or payment, making a purchase, deleting files or records, or starting a desktop task that types into an app. Show exactly what will be sent or submitted (recipient, subject, body, or final amount and payee) and get an explicit yes. One confirmation covers one action, not a batch of unrelated actions.

## Money
`personal_os` is bookkeeping only. Record and summarize what the user tells you; never initiate transfers, never enter card or bank credentials, and never claim a payment happened. If asked to move money, say you only keep records and hand that step back to the user.

## Privacy
Never write passwords, API keys, tokens, full card numbers, or one-time codes to memory, files, plans, notes, or any tool argument. 2FA and logins are always the user's job. Do not dump the whole personal store into chat; summarize and keep only what the task needs.

## Work style
Inspect before answering; never invent tool results or page content. If a site or app blocks you, report the exact error and the state you reached, then propose the next step. Keep replies short, concrete, and in the user's terms."#,
};

pub const HOUSE: AgentSpec = AgentSpec {
    key: "house",
    display: "House",
    description: "Smart-home control and household context.",
    plan_scope: "house",
    tier: "workspace",
    instructions: "You are the House agent. Read and control the smart home through the house tools. You do not write code. You may read household context from personal_os (overview, todo_list, food_summary, training_summary); money and all personal_os writes are blocked for you, so never attempt them.",
    access_root: FileAccess::Read,
    access_spawned: FileAccess::Read,
    prompt_profile: "general",
    delegatable: true,
    can_delegate: false,
    can_spawn: true,
    ui_order: 30,
    aliases: &[],
};

pub const DFD: AgentSpec = AgentSpec {
    key: "dfd",
    display: "DFD",
    description: "Agency clients, projects, and scaffolding.",
    plan_scope: "dfd",
    tier: "workspace",
    instructions: "You are the DFD agent. Handle agency clients, projects, and scaffolding. You do not move money.",
    access_root: FileAccess::Write,
    access_spawned: FileAccess::Write,
    prompt_profile: "general",
    delegatable: true,
    can_delegate: false,
    can_spawn: true,
    ui_order: 40,
    aliases: &[],
};

pub const UI: AgentSpec = AgentSpec {
    key: "ui",
    display: "UI",
    description: "Frontend design, implementation, and browser verification.",
    plan_scope: "ui",
    tier: "workspace",
    instructions: "You are the UI agent. Design and verify frontends with the browser and editing tools.",
    access_root: FileAccess::Write,
    access_spawned: FileAccess::Worktree,
    prompt_profile: "general",
    delegatable: true,
    can_delegate: false,
    can_spawn: true,
    ui_order: 50,
    aliases: &["design", "designer", "frontend", "front-end"],
};

pub const DEVOPS: AgentSpec = AgentSpec {
    key: "devops",
    display: "DevOps",
    description: "Deployment, CI, infrastructure, and operational verification.",
    plan_scope: "devops",
    tier: "specialist",
    instructions: "You are the DevOps agent. Handle deployment, CI, infrastructure, and operational verification. Prefer reproducible commands and explicit rollback paths.",
    access_root: FileAccess::Write,
    access_spawned: FileAccess::Worktree,
    prompt_profile: "general",
    delegatable: false,
    can_delegate: false,
    can_spawn: false,
    ui_order: 60,
    aliases: &["ops", "operations", "sre", "infra"],
};

pub const EXPLORE: AgentSpec = AgentSpec {
    key: "explore",
    display: "Explore",
    description: "Read-only codebase mapping and findings.",
    plan_scope: "explore",
    tier: "specialist",
    instructions: "You are the Explore agent. Read broadly, map unfamiliar code, and report concise findings with exact paths. Do not edit files unless explicitly asked.",
    access_root: FileAccess::Read,
    access_spawned: FileAccess::Read,
    prompt_profile: "general",
    delegatable: false,
    can_delegate: false,
    can_spawn: false,
    ui_order: 70,
    aliases: &["scout", "explorer"],
};

pub const ORCHESTRATOR: AgentSpec = AgentSpec {
    key: "orchestrator",
    display: "Orchestrator",
    description: "Manager and router for workspaces, groups, and schedules.",
    plan_scope: "orchestrator",
    tier: "orchestrator",
    access_root: FileAccess::Read,
    access_spawned: FileAccess::Read,
    prompt_profile: "general",
    delegatable: false,
    can_delegate: true,
    can_spawn: false,
    ui_order: 80,
    instructions: r#"You are the Orchestrator: the user's manager and router. Your first job is to decide whose job a request is, not to do the work yourself.

Answer directly when the request can be answered read-only. You have read-only bash (for example `ls`, `cat`, `rg`, `git status`) and read-only memory for exactly this. Inspect the actual files before answering; do not answer from memory or guesswork. Do not browse, search the web, or run mutating commands yourself; delegate those.

When the request needs edits, builds, tests, deployments, file writes, browser mutations, or any long-running work, delegate. Use delegate with the workspace id or exact name (for example "4Ceria"); Delegation results arrive as [DELEGATION RESULT] events; do not poll. Call delegation_status only once when the user asks for status or after a timeout. Delegated workspace agents run in the target workspace and are NOT bound by your read-only sandbox. Never conclude that a path is blocked before you have tried workspace action="info"/"list" and a read-only inspection.

Use workspace to list or resolve registered workspaces. If a path is outside every registered workspace, say so and ask the user whether to register it; do not silently give up. Use create_group, group_update, schedule, give_note, and canvas_mode to coordinate work. Group progress is event-driven; call group_board once only when the user asks for status. You do not maintain a plan or todo list; coordination state belongs to groups and schedules.

Memory is historical context, not current policy. The runtime context in this prompt (workspace, sandbox, approvals) always wins over memory entries. If memory contradicts what you can verify now, trust the verification and say what you found. Never invent tool results. If a tool fails, report the exact error, fix the arguments or try the next read-only alternative, and only then report a blocker."#,
    aliases: &[],
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileAccess {
    Read,
    Write,
    Worktree,
}

impl FileAccess {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Worktree => "worktree",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "read" | "readonly" | "read-only" => Self::Read,
            "worktree" | "isolated" => Self::Worktree,
            _ => Self::Write,
        }
    }

    pub fn is_read_only(self) -> bool {
        matches!(self, Self::Read)
    }
}

pub fn default_access(scope: &str, depth: u32, spawned: bool) -> FileAccess {
    // Resolve aliases first so compatibility names keep their agent's access.
    if let Some(agent) = for_scope(scope) {
        if depth >= 2 {
            return FileAccess::Read;
        }
        return if spawned {
            agent.access_spawned
        } else {
            agent.access_root
        };
    }
    // Unknown/custom scopes fail closed. A custom runtime passes its resolved
    // access explicitly; this fallback only protects callers that lost it.
    FileAccess::Read
}

/// Parse the persisted access string. Unknown values fail closed.
pub fn access_from_str(value: &str) -> FileAccess {
    FileAccess::from_str(value)
}

pub fn worktree_mode() -> String {
    std::env::var("STARK_WORKTREES")
        .unwrap_or_else(|_| "auto".to_string())
        .trim()
        .to_ascii_lowercase()
}

pub const WORKSPACE_AGENTS: &[AgentSpec] = &[
    DEVELOPER,
    GENERAL,
    HOUSE,
    DFD,
    UI,
    DEVOPS,
    EXPLORE,
    ORCHESTRATOR,
];

/// Every first-class agent, in registry order. Consumers (the config API, the
/// picker, docs) derive their lists from here instead of duplicating keys.
pub fn workspace_agents() -> &'static [AgentSpec] {
    WORKSPACE_AGENTS
}

pub fn for_scope(scope: &str) -> Option<AgentSpec> {
    let scope = scope.trim().to_lowercase();
    WORKSPACE_AGENTS
        .iter()
        .copied()
        .find(|agent| agent.key == scope.as_str() || agent.aliases.contains(&scope.as_str()))
}

pub fn resolve_agent(
    workspace_kind: &str,
    workspace_path: &Path,
    harness_root: &Path,
) -> AgentSpec {
    let kind = workspace_kind.trim().to_lowercase();
    match kind.as_str() {
        "house" | "house_workspace" => HOUSE,
        "personal" | "personal_workspace" | "general_workspace" => GENERAL,
        "dfd" | "dfd_workspace" => DFD,
        "design" | "design_workspace" => UI,
        _ if workspace_path != harness_root => DEVELOPER,
        _ => ORCHESTRATOR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_kind_hints_win() {
        let root = Path::new("root");
        assert_eq!(resolve_agent("house", root, root), HOUSE);
        assert_eq!(resolve_agent("personal", root, root), GENERAL);
        assert_eq!(resolve_agent("dfd_workspace", root, root), DFD);
        assert_eq!(resolve_agent("design", root, root), UI);
    }

    #[test]
    fn project_paths_route_to_developer() {
        let root = Path::new("root");
        let project = Path::new("project");
        assert_eq!(resolve_agent("", project, root), DEVELOPER);
        assert_eq!(resolve_agent("coding", project, root), DEVELOPER);
    }

    #[test]
    fn empty_workspace_uses_orchestrator() {
        let root = Path::new("root");
        assert_eq!(resolve_agent("", root, root), ORCHESTRATOR);
        assert_eq!(resolve_agent("unknown", root, root), ORCHESTRATOR);
    }

    #[test]
    fn orchestrator_is_a_read_only_manager() {
        assert_eq!(default_access("orchestrator", 0, false), FileAccess::Read);
        let prompt = ORCHESTRATOR.instructions;
        assert!(prompt.contains("whose job"));
        assert!(prompt.contains("read-only"));
        assert!(prompt.contains("NOT bound by your read-only sandbox"));
        assert!(prompt.contains("read-only bash"));
        assert!(!prompt.contains("browser_act"));
        assert!(prompt.contains("delegate"));
    }

    #[test]
    fn scopes_resolve_case_insensitively() {
        assert_eq!(for_scope("Developer"), Some(DEVELOPER));
        assert_eq!(for_scope("UI"), Some(UI));
        assert_eq!(for_scope("missing"), None);
    }

    #[test]
    fn aliases_resolve_to_their_canonical_agent() {
        assert_eq!(for_scope("coder"), Some(DEVELOPER));
        assert_eq!(for_scope("Coder"), Some(DEVELOPER));
        assert_eq!(for_scope("frontend"), Some(UI));
        assert_eq!(for_scope("ops"), Some(DEVOPS));
    }

    #[test]
    fn coder_is_not_a_first_class_agent() {
        assert_eq!(workspace_agents().len(), 8);
        assert!(workspace_agents().iter().all(|agent| agent.key != "coder"));
        assert_eq!(for_scope("coder").map(|agent| agent.key), Some("developer"));
    }

    #[test]
    fn aliases_are_unique_and_do_not_shadow_agent_keys() {
        let mut seen: Vec<&str> = Vec::new();
        for agent in WORKSPACE_AGENTS {
            for alias in agent.aliases {
                assert!(
                    !WORKSPACE_AGENTS.iter().any(|other| other.key == *alias),
                    "alias {alias} shadows an agent key"
                );
                assert!(!seen.contains(alias), "duplicate alias {alias}");
                seen.push(alias);
            }
        }
    }

    #[test]
    fn general_routes_public_private_and_desktop_work() {
        let prompt = GENERAL.instructions;
        assert!(prompt.contains("personal operator"));
        assert!(prompt.contains("browser_act"));
        assert!(prompt.contains("mcp__browse__"));
        assert!(prompt.contains("mcp__agent-desktop__desktop_"));
        assert!(prompt.contains("personal_os"));
        assert!(prompt.contains("ask_user_question"));
        assert!(prompt.contains("bookkeeping only"));
        assert!(prompt.contains("Never write passwords"));
        assert!(prompt.contains("Developer"));
        assert!(prompt.contains("House"));
        assert!(HOUSE.instructions.contains("personal_os"));
        assert!(HOUSE.instructions.contains("money"));
    }

    #[test]
    fn coder_alias_matches_developer_access() {
        assert_eq!(default_access("coder", 0, false), FileAccess::Write);
        assert_eq!(default_access("coder", 1, true), FileAccess::Worktree);
        assert_eq!(default_access("coder", 2, false), FileAccess::Read);
    }

    #[test]
    fn every_registered_scope_exists_in_the_tool_contract() {
        let contract = stark_tools::contract();
        for agent in WORKSPACE_AGENTS {
            assert!(
                contract.scope_map.contains_key(agent.key),
                "missing scope {}",
                agent.key
            );
        }
        for scope in contract.scope_map.keys() {
            assert!(
                WORKSPACE_AGENTS.iter().any(|agent| agent.key == scope),
                "contract scope {scope} has no registered agent"
            );
        }
    }
}
