"""agents.group.materialize — write the capability set into a project (Phase 2).

``docs/group-projects/01-concept-model.md`` section 2 and
``05-integration-map.md`` section 9.2.

A group project folder is **self-contained**: the agents, the skills, the tool
bindings and the MCP servers it uses live inside it, chosen by the
orchestrator at creation. This module writes them.

Two contracts are enforced here rather than merely documented:

- **DM-09 / provenance** — every materialised agent records ``derived_from``
  and ``derived_at_version``. Without them, a snapshot is archaeology: you
  cannot tell what it came from or regenerate it.
- **DM-10 / credentials by name** — ``mcp/servers.json`` may reference a
  secret only through an environment-variable *name*. A literal value is
  rejected, not sanitised. Copying a folder must never copy a secret.

The registry stays the template (``workspace_registry.py``); the folder owns
its copy from creation onward. Editing one does not affect the other.
"""
from __future__ import annotations

import json
import os
import re
from typing import Any

from .manifest import dumps

#: Version stamped into every materialised agent. Should track the harness
#: release; kept here so there is one place to change when it moves.
HARNESS_VERSION = "2.0.0"

SCHEMA = 1

#: MCP keys a project file may carry. Anything else is refused rather than
#: passed through — an unknown key is how a credential value would slip in.
_MCP_ALLOWED_KEYS = frozenset({
    "url", "command", "args", "env_vars", "bearer_token_env_var",
    "http_headers", "scopes", "enabled_tools", "disabled_tools",
    "approval", "startup_timeout_sec", "tool_timeout_sec", "instructions",
})

#: Header names that carry credentials; always refused as literals.
_CREDENTIAL_HEADERS = frozenset({
    "authorization", "x-api-key", "api-key", "x-auth-token", "cookie",
    "proxy-authorization",
})

_ENV_NAME = re.compile(r"^[A-Z][A-Z0-9_]*$")

#: A value that looks like a token rather than a name.
_SECRET_LOOKING = re.compile(
    r"(sk-[A-Za-z0-9_\-]{16,}|ghp_[A-Za-z0-9]{16,}|github_pat_|"
    r"xox[baprs]-|AIza[0-9A-Za-z_\-]{20,}|hf_[A-Za-z0-9]{16,}|"
    r"\d{8,10}:[A-Za-z0-9_\-]{30,}|BEGIN [A-Z ]*PRIVATE KEY)"
)


class MaterializationError(Exception):
    """A capability set could not be safely written."""


# ── agents ──────────────────────────────────────────────────────────────────


def agent_from_spec(spec: Any, name: str, *,
                    model: str = "", thinking: str = "",
                    tools: list[str] | None = None,
                    skills: list[str] | None = None,
                    mcp: list[str] | None = None,
                    tool_caps: dict | None = None,
                    version: str = HARNESS_VERSION) -> dict:
    """Resolve a registry ``WorkspaceSpec`` into a project-owned agent file.

    ``tools`` narrows the spec's allowlist — it can never widen it. That is the
    same intersection rule the runtime enforces (``04`` section 5), applied at
    write time so the file is honest about what the member may do.
    """
    base_tools = list(getattr(spec, "tools", ()) or ())
    if tools is None:
        resolved_tools = base_tools
    else:
        resolved_tools = [t for t in tools if t in base_tools]
        dropped = sorted(set(tools) - set(resolved_tools))
        if dropped:
            raise MaterializationError(
                f"tools {dropped} are not in {getattr(spec, 'key', '?')!r}'s "
                f"allowlist; a project may narrow capabilities, never widen them"
            )
    return {
        "schema": SCHEMA,
        "name": name,
        "derived_from": str(getattr(spec, "key", "") or ""),
        "derived_at_version": version,
        "display": str(getattr(spec, "display", "") or name.title()),
        "kind": str(getattr(spec, "kind", "workspace") or "workspace"),
        "runtime": str(getattr(spec, "runtime", "") or ""),
        "runtime_name": str(getattr(spec, "runtime_name", "") or ""),
        "state_builder": str(getattr(spec, "state_builder", "plain") or "plain"),
        "prompt_module": str(getattr(spec, "prompt_module", "") or ""),
        "tier": str(getattr(spec, "provider_tier", "") or getattr(spec, "tier", "") or ""),
        "model": model or str(getattr(spec, "model", "") or ""),
        "thinking": thinking or str(getattr(spec, "thinking", "inherit") or "inherit"),
        "tools": resolved_tools,
        "tool_caps": dict(tool_caps or getattr(spec, "tool_caps", {}) or {}),
        "skills": list(skills or []),
        "mcp": list(mcp or []),
        "compaction": dict(getattr(spec, "compaction", {}) or {}),
        "permissions": [dict(p) for p in (getattr(spec, "permissions", ()) or ())],
        "system_prompt": str(getattr(spec, "system_prompt", "") or ""),
    }


def agent_from_profile(profile: dict, name: str, *,
                       version: str = HARNESS_VERSION) -> dict:
    """Resolve a specialist JSON profile (``agents/specialists/*.json``)."""
    return {
        "schema": SCHEMA,
        "name": name,
        "derived_from": str(profile.get("name") or "").strip().lower(),
        "derived_at_version": version,
        "display": str(profile.get("name") or name.title()),
        "kind": str(profile.get("kind") or "subagent"),
        "runtime": "",
        "runtime_name": "",
        "state_builder": "plain",
        "prompt_module": "",
        "tier": str(profile.get("backend") or ""),
        "model": str(profile.get("model") or ""),
        "thinking": str(profile.get("thinking") or "inherit"),
        "tools": list(profile.get("tools") or []),
        "tool_caps": dict(profile.get("tool_caps") or {}),
        "skills": list(profile.get("skills") or []),
        "mcp": list(profile.get("mcp") or []),
        "compaction": dict(profile.get("compaction") or {}),
        "permissions": [dict(p) for p in (profile.get("permissions") or [])],
        "system_prompt": _join_prompt(profile.get("system_prompt")),
    }


def _join_prompt(value: Any) -> str:
    if isinstance(value, str):
        return value
    if isinstance(value, list):
        return "\n".join(str(line) for line in value)
    return ""


def write_agent(agent: dict, agents_dir: str) -> str:
    """Write ``agents/<name>.json``. Refuses an agent with no provenance."""
    if not agent.get("derived_from"):
        raise MaterializationError(
            f"agent {agent.get('name')!r} has no derived_from (DM-09)"
        )
    if not agent.get("derived_at_version"):
        raise MaterializationError(
            f"agent {agent.get('name')!r} has no derived_at_version (DM-09)"
        )
    os.makedirs(agents_dir, exist_ok=True)
    path = os.path.join(agents_dir, f"{agent['name']}.json")
    with open(path, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(dumps(agent))
    return path


# ── skills ──────────────────────────────────────────────────────────────────


def skill_markdown(skill: dict) -> str:
    """Re-render a discovered skill as a standalone ``SKILL.md``.

    The copied file is the project's own from then on: editing it does not
    touch the global catalogue, and no other project sees the change.
    """
    frontmatter = dict(skill.get("frontmatter") or {})
    frontmatter.setdefault("name", skill.get("name") or "")
    if skill.get("description"):
        frontmatter.setdefault("description", skill["description"])
    allowed = skill.get("allowed_tools")
    if allowed:
        frontmatter.setdefault("allowed-tools", ", ".join(allowed))
    lines = ["---"]
    for key, value in frontmatter.items():
        lines.append(f"{key}: {value}")
    lines.append("---")
    lines.append("")
    lines.append(str(skill.get("body") or "").rstrip())
    lines.append("")
    return "\n".join(lines)


def copy_skill(skill: dict, skills_dir: str) -> str:
    name = str(skill.get("name") or "").strip()
    if not name:
        raise MaterializationError("skill has no name")
    target_dir = os.path.join(skills_dir, name)
    os.makedirs(target_dir, exist_ok=True)
    path = os.path.join(target_dir, "SKILL.md")
    with open(path, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(skill_markdown(skill))
    return path


def write_skills_index(skills: list[dict], dest_dir: str) -> str:
    """``skills/index.json`` — what is here, so the folder explains itself."""
    os.makedirs(dest_dir, exist_ok=True)
    path = os.path.join(dest_dir, "index.json")
    payload = {
        "schema": SCHEMA,
        "skills": [
            {
                "name": s.get("name"),
                "description": s.get("description") or "",
                "allowed_tools": list(s.get("allowed_tools") or []),
            }
            for s in skills
        ],
    }
    with open(path, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(dumps(payload))
    return path


# ── tool bindings ───────────────────────────────────────────────────────────


def write_bindings(agents: list[dict], dest_dir: str) -> str:
    """``tools/bindings.json`` — the members' allowlists and caps.

    The ceiling is the registry scope map; this file only ever narrows. The
    runtime intersects the two (``04`` section 5), so a hand-edited binding
    cannot grant a tool the agent never had.
    """
    os.makedirs(dest_dir, exist_ok=True)
    path = os.path.join(dest_dir, "bindings.json")
    payload = {
        "schema": SCHEMA,
        "agents": {
            a["name"]: {
                "allow": list(a.get("tools") or []),
                "caps": dict(a.get("tool_caps") or {}),
                "derived_from": a.get("derived_from") or "",
            }
            for a in agents
        },
    }
    with open(path, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(dumps(payload))
    return path


# ── MCP ─────────────────────────────────────────────────────────────────────


def validate_mcp_servers(servers: dict) -> None:
    """Raise if any server config could carry a literal credential (DM-10)."""
    problems: list[str] = []
    for name, config in (servers or {}).items():
        if not isinstance(config, dict):
            problems.append(f"{name}: config is not an object")
            continue
        for key, value in config.items():
            if key not in _MCP_ALLOWED_KEYS:
                problems.append(f"{name}: key {key!r} is not allowed")
                continue
            if key == "bearer_token_env_var":
                if value and not _ENV_NAME.match(str(value)):
                    problems.append(
                        f"{name}: bearer_token_env_var must be a NAME, got a value"
                    )
            elif key == "env_vars":
                for entry in value or []:
                    if not _ENV_NAME.match(str(entry)):
                        problems.append(
                            f"{name}: env_vars entry {entry!r} must be a NAME"
                        )
            elif key == "http_headers":
                for header, header_value in (value or {}).items():
                    if str(header).lower() in _CREDENTIAL_HEADERS:
                        problems.append(
                            f"{name}: header {header!r} must not carry a literal "
                            f"credential"
                        )
                    if _SECRET_LOOKING.search(str(header_value)):
                        problems.append(f"{name}: header {header!r} looks like a secret")
            elif isinstance(value, str) and _SECRET_LOOKING.search(value):
                problems.append(f"{name}: {key!r} looks like a secret value")
    if problems:
        raise MaterializationError(
            "MCP config must reference credentials by environment-variable name "
            "only (DM-10):\n  - " + "\n  - ".join(problems)
        )


def write_mcp_servers(servers: dict, dest_dir: str) -> str:
    """``mcp/servers.json`` — the project's own copy, credentials by name."""
    validate_mcp_servers(servers)
    os.makedirs(dest_dir, exist_ok=True)
    path = os.path.join(dest_dir, "servers.json")
    with open(path, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(dumps({"schema": SCHEMA, "servers": servers or {}}))
    return path


def select_mcp_servers(catalogue: dict, wanted: list[str],
                       tool_filter: dict | None = None) -> dict:
    """Pick servers out of the machine-level catalogue by name.

    ``.deep_agents/mcp_servers.json`` stays the record of what is *installed*;
    this produces the project's subset. The catalogue's transport config is
    copied; credentials are only ever referenced by name, so nothing secret
    travels.
    """
    available = (catalogue or {}).get("servers") or catalogue or {}
    selected: dict = {}
    for name in wanted or []:
        config = available.get(name)
        if config is None:
            raise MaterializationError(
                f"MCP server {name!r} is not installed on this machine; a "
                f"project may only attach what the catalogue has"
            )
        entry = {k: v for k, v in config.items() if k in _MCP_ALLOWED_KEYS}
        overrides = (tool_filter or {}).get(name) or {}
        entry.update({k: v for k, v in overrides.items() if k in _MCP_ALLOWED_KEYS})
        selected[name] = entry
    return selected


# ── deps ────────────────────────────────────────────────────────────────────


def write_deps(deps: dict, dest_dir: str) -> list[str]:
    """``deps/`` — this project's own dependencies, not the harness's.

    Installed into the project (local or container alike), so one project can
    never mutate another's environment, and the folder is reproducible.
    """
    os.makedirs(dest_dir, exist_ok=True)
    written: list[str] = []
    python_deps = deps.get("python") or []
    if python_deps:
        path = os.path.join(dest_dir, "requirements.txt")
        with open(path, "w", encoding="utf-8", newline="\n") as handle:
            handle.write("\n".join(str(d) for d in python_deps) + "\n")
        written.append(path)
    node_deps = deps.get("node") or []
    if node_deps:
        path = os.path.join(dest_dir, "package.json")
        payload = {
            "name": f"gp-{deps.get('project_id') or 'project'}".lower(),
            "private": True,
            "dependencies": {str(k): str(v) for k, v in dict(node_deps).items()}
            if isinstance(node_deps, dict) else
            {str(name): "latest" for name in node_deps},
        }
        with open(path, "w", encoding="utf-8", newline="\n") as handle:
            handle.write(json.dumps(payload, indent=2) + "\n")
        written.append(path)
    return written


# ════════════════════════════════════════════════════════════════════════════
#  Live capability changes (Phase 3)
# ════════════════════════════════════════════════════════════════════════════
#
# The orchestrator can change a RUNNING project's capability set: add an MCP
# server, copy a skill in, widen (never widen past the ceiling) a member's
# tools. These helpers edit the folder; the caller turns each into an op so
# there is still exactly one writer and one event per change.


def read_json(path: str, default: Any = None) -> Any:
    if not os.path.isfile(path):
        return default
    try:
        with open(path, encoding="utf-8") as handle:
            return json.load(handle)
    except (OSError, ValueError):
        return default


def read_agents(agents_dir: str) -> dict[str, dict]:
    out: dict[str, dict] = {}
    if not os.path.isdir(agents_dir):
        return out
    for filename in sorted(os.listdir(agents_dir)):
        if not filename.endswith(".json"):
            continue
        data = read_json(os.path.join(agents_dir, filename))
        if isinstance(data, dict) and data.get("name"):
            out[data["name"]] = data
    return out


def add_mcp_server(paths: Any, catalogue: dict, server: str,
                   overrides: dict | None = None) -> dict:
    """Attach one server to the project. Credentials stay names (DM-10)."""
    current = read_json(os.path.join(paths.mcp_dir, "servers.json"), {}) or {}
    servers = dict(current.get("servers") or {})
    if server in servers:
        return servers[server]
    if not servers:
        os.makedirs(paths.mcp_dir, exist_ok=True)
    selected = select_mcp_servers(catalogue, [server], {server: overrides or {}})
    servers[server] = selected[server]
    write_mcp_servers(servers, paths.mcp_dir)
    return servers[server]


def remove_mcp_server(paths: Any, server: str) -> bool:
    current = read_json(os.path.join(paths.mcp_dir, "servers.json"), {}) or {}
    servers = dict(current.get("servers") or {})
    if server not in servers:
        return False
    servers.pop(server)
    write_mcp_servers(servers, paths.mcp_dir)
    return True


def add_skill(paths: Any, skill: dict) -> str:
    path = copy_skill(skill, paths.skills_dir)
    index = read_json(os.path.join(paths.skills_dir, "index.json"), {}) or {}
    entries = [e for e in (index.get("skills") or [])
               if e.get("name") != skill.get("name")]
    entries.append({"name": skill.get("name"),
                    "description": skill.get("description") or "",
                    "allowed_tools": list(skill.get("allowed_tools") or [])})
    os.makedirs(paths.skills_dir, exist_ok=True)
    with open(os.path.join(paths.skills_dir, "index.json"), "w",
              encoding="utf-8", newline="\n") as handle:
        handle.write(dumps({"schema": SCHEMA, "skills": entries}))
    return path


def remove_skill(paths: Any, name: str) -> bool:
    import shutil

    target = os.path.join(paths.skills_dir, name)
    if not os.path.isdir(target):
        return False
    shutil.rmtree(target, ignore_errors=True)
    index_path = os.path.join(paths.skills_dir, "index.json")
    index = read_json(index_path, {}) or {}
    entries = [e for e in (index.get("skills") or []) if e.get("name") != name]
    with open(index_path, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(dumps({"schema": SCHEMA, "skills": entries}))
    return True


def set_member_tools(paths: Any, member: str, tools: list[str],
                     *, ceiling: list[str] | None = None) -> dict:
    """Narrow or extend a member's tools — but never past the ceiling.

    ``ceiling`` is the scope map's allowlist for the member's ``derived_from``.
    A project may only ever intersect it (``04`` section 5), so a hand-edited
    binding cannot grant a tool the agent never had.
    """
    agent_path = os.path.join(paths.agents_dir, f"{member}.json")
    agent = read_json(agent_path)
    if not isinstance(agent, dict):
        raise MaterializationError(f"member {member!r} has no materialised agent")
    wanted = list(tools)
    if ceiling is not None:
        allowed = [t for t in wanted if t in ceiling]
        dropped = sorted(set(wanted) - set(allowed))
        if dropped:
            raise MaterializationError(
                f"tools {dropped} are outside {member!r}'s scope ceiling; a "
                f"project may narrow capabilities, never widen them"
            )
        wanted = allowed
    agent["tools"] = wanted
    write_agent(agent, paths.agents_dir)

    bindings_path = os.path.join(paths.tools_dir, "bindings.json")
    bindings = read_json(bindings_path, {}) or {}
    agents = dict(bindings.get("agents") or {})
    entry = dict(agents.get(member) or {})
    entry["allow"] = wanted
    entry.setdefault("caps", agent.get("tool_caps") or {})
    entry["derived_from"] = agent.get("derived_from") or ""
    agents[member] = entry
    os.makedirs(paths.tools_dir, exist_ok=True)
    with open(bindings_path, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(dumps({"schema": SCHEMA, "agents": agents}))
    return agent


def remove_agent(paths: Any, member: str) -> bool:
    agent_path = os.path.join(paths.agents_dir, f"{member}.json")
    if not os.path.isfile(agent_path):
        return False
    os.remove(agent_path)
    bindings_path = os.path.join(paths.tools_dir, "bindings.json")
    bindings = read_json(bindings_path, {}) or {}
    agents = dict(bindings.get("agents") or {})
    if agents.pop(member, None) is not None:
        with open(bindings_path, "w", encoding="utf-8", newline="\n") as handle:
            handle.write(dumps({"schema": SCHEMA, "agents": agents}))
    return True
