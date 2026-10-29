use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentSpec {
    pub key: &'static str,
    pub display: &'static str,
    pub plan_scope: &'static str,
    pub tier: &'static str,
    pub instructions: &'static str,
}

pub const DEVELOPER: AgentSpec = AgentSpec {
    key: "developer",
    display: "Developer",
    plan_scope: "developer",
    tier: "workspace",
    instructions: "You are the Developer agent. Implement, test, and verify code. Read files before editing. Keep the plan current for multi-step work.",
};

pub const GENERAL: AgentSpec = AgentSpec {
    key: "general",
    display: "General",
    plan_scope: "general",
    tier: "workspace",
    instructions: "You are the General agent. Handle research, documents, planning, and everyday work. Coding requests belong to Developer.",
};

pub const HOUSE: AgentSpec = AgentSpec {
    key: "house",
    display: "House",
    plan_scope: "house",
    tier: "workspace",
    instructions: "You are the House agent. Read and control the smart home through the house tools. You do not write code.",
};

pub const DFD: AgentSpec = AgentSpec {
    key: "dfd",
    display: "DFD",
    plan_scope: "dfd",
    tier: "workspace",
    instructions: "You are the DFD agent. Handle agency clients, projects, and scaffolding. You do not move money.",
};

pub const UI: AgentSpec = AgentSpec {
    key: "ui",
    display: "UI",
    plan_scope: "ui",
    tier: "workspace",
    instructions: "You are the UI agent. Design and verify frontends with the browser and editing tools.",
};

pub const DEVOPS: AgentSpec = AgentSpec {
    key: "devops",
    display: "DevOps",
    plan_scope: "devops",
    tier: "specialist",
    instructions: "You are the DevOps agent. Handle deployment, CI, infrastructure, and operational verification. Prefer reproducible commands and explicit rollback paths.",
};

pub const EXPLORE: AgentSpec = AgentSpec {
    key: "explore",
    display: "Explore",
    plan_scope: "explore",
    tier: "specialist",
    instructions: "You are the Explore agent. Read broadly, map unfamiliar code, and report concise findings with exact paths. Do not edit files unless explicitly asked.",
};

pub const ORCHESTRATOR: AgentSpec = AgentSpec {
    key: "orchestrator",
    display: "Orchestrator",
    plan_scope: "orchestrator",
    tier: "orchestrator",
    instructions: "You are the Orchestrator. Route work to the right agent and coordinate jobs.",
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
    let scope = scope.trim().to_ascii_lowercase();
    if scope == "orchestrator" || scope == "house" || scope == "explore" {
        return FileAccess::Read;
    }
    if depth >= 2 {
        return FileAccess::Read;
    }
    if !spawned {
        return match scope.as_str() {
            "developer" | "ui" | "devops" => FileAccess::Write,
            "general" | "dfd" => FileAccess::Write,
            _ => FileAccess::Read,
        };
    }
    match scope.as_str() {
        "developer" | "ui" | "devops" => FileAccess::Worktree,
        "general" | "dfd" => FileAccess::Write,
        _ => FileAccess::Read,
    }
}

pub fn worktree_mode() -> String {
    std::env::var("STARK_WORKTREES")
        .unwrap_or_else(|_| "auto".to_string())
        .trim()
        .to_ascii_lowercase()
}

const WORKSPACE_AGENTS: &[AgentSpec] = &[
    DEVELOPER,
    GENERAL,
    HOUSE,
    DFD,
    UI,
    DEVOPS,
    EXPLORE,
    ORCHESTRATOR,
];

pub fn for_scope(scope: &str) -> Option<AgentSpec> {
    let scope = scope.trim().to_lowercase();
    WORKSPACE_AGENTS
        .iter()
        .copied()
        .find(|agent| agent.key == scope)
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
    fn scopes_resolve_case_insensitively() {
        assert_eq!(for_scope("Developer"), Some(DEVELOPER));
        assert_eq!(for_scope("UI"), Some(UI));
        assert_eq!(for_scope("missing"), None);
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
    }
}
