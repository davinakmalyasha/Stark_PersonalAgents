# Agent builder API (v2)

The backend owns one runtime agent registry. Built-ins are compiled seeds; a
custom agent is a validated JSON record under
`.deep_agents/config/agents/<key>.json`. No rebuild or restart is required:
the config store watches file stamps and the registry is rebuilt per request.

The frontend contract is additive. Existing `/api/config/agents` fields
(`key`, `display`, `tier`, `configured`, `override`, `effective`) are
unchanged.

## Safety model

- **Closed tool list.** A custom agent gets exactly the tools in `tools`.
  Omitted/unknown tools are rejected on save; an invalid file is quarantined,
  never partially applied. `[]` means no tools.
- **No escalation.** Built-in access/prompt/tool caps cannot be widened.
  Custom access defaults to `read`. A read-only agent cannot receive a
  mutating tool. Depth ≥ 2 is always read-only.
- **Closed MCP/skills.** For custom agents, omitted `skills`/`mcp` means none,
  not all. Workspace skills are trusted instructions: symlinked skill
  directories and `SKILL.md` files are ignored, so a cloned repo cannot point
  a skill at an arbitrary host file. MCP names must exist in the live config when saved. Skills are
  workspace-aware: save/preview with `workspace_path` validates strictly;
  without it unknown skills are warnings because they may be project-local.
  The `skill` tool enforces the same allowlist at dispatch.
- **Custom Code Mode is opt-in hybrid.** A custom agent may set
  `code_mode: "hybrid"` and include `execute` in `tools`. `off` must not
  contain `execute`; `hybrid` requires it. Custom `only`/`full_codemode` are
  rejected; those remain built-in Developer features. Built-in Code Mode
  defaults are unchanged.
- **Capabilities gate orchestration.** `delegate`/`delegation_status`/`job_*`
  require `delegation.can_delegate`; `spawn_agent`/`message_agent`/`wait_agent`/
  `interrupt_agent`/`close_agent`/`agent` require `delegation.can_spawn`.
  Listing a tool without its capability is a validation error.
- **Explicit deletion.** A chat stores its resolved `agentKey`. If that agent
  is deleted, the next run fails with `agent '<key>' no longer exists` instead
  of silently routing to a more privileged built-in.
- **Persona is additive.** `prompt.persona` is appended after harness/safety
  text. It cannot remove code-enforced guards.

## Definition

Schema: [`backend-rs/specs/agent-def.schema.json`](../backend-rs/specs/agent-def.schema.json)

```json
{
  "version": 2,
  "key": "backend",
  "display": "Backend",
  "description": "Rust backend and server work",
  "tier": "workspace",
  "access": "write",
  "prompt": {
    "profile": "coding",
    "persona": "You own backend-rs. Rust only. Read before editing."
  },
  "tools": ["bash", "read", "apply_patch", "edit", "glob", "grep", "update_plan"],
  "skills": ["supabase"],
  "mcp": ["github"],
  "delegation": {
    "delegatable": true,
    "can_delegate": false,
    "can_spawn": false
  },
  "ui": { "visible": true, "order": 25 },
  "routing": {
    "workspace_kinds": ["coding"],
    "paths": ["D:/work/backend/**"]
  },
  "code_mode": "off"
}
```

For base Code Mode:

```json
{
  "code_mode": "hybrid",
  "tools": ["read", "files_changed", "execute"]
}
```

Notes:

- `tools` is required for a **new** custom agent. Use `[]` for a talk-only
  agent.
- For a **built-in** key, `tools` is an optional subset of the compiled scope
  contract. `access` may narrow but not widen the compiled cap. The prompt
  profile is fixed.
- `aliases` are optional for custom agents; they must not shadow any key or
  alias. Aliases are never listed as agents.
- `code_mode` is `off` (default) or `hybrid` for custom agents. `execute` is
  required for hybrid and forbidden for off.
- `routing` is custom-only. It applies only when no explicit agent or chat
  `agentKey` exists and the workspace kind is generic. Both lists non-empty
  means both must match. A unique match wins; multiple matches fall back with
  a warning. Explicit built-in keys and virtual kinds (`general`, `house`,
  `personal`, `design`, …) win over routing.
- Routing paths are absolute globs. `\` is normalized to `/`, trailing
  separators are ignored, and matching is case-insensitive on Windows and
  macOS. Empty or relative globs fail save validation; on load they are
  dropped with a warning instead of quarantining the agent.

## Endpoints

### `GET /api/agent-catalog`

Returns the builder's pick lists and capability flags:

```json
{
  "schema_version": 2,
  "capabilities": {
    "agent_builder": true,
    "custom_agents": true,
    "custom_code_mode": true,
    "custom_routing": true,
    "duplicate": true,
    "preview": true
  },
  "agents": [{ "key": "developer", "display": "Developer", "builtin": true }],
  "tools": [{
    "name": "read",
    "description": "...",
    "parameters": {},
    "program": true,
    "codemode": false,
    "read_blocked": false,
    "plan_write": false
  }],
  "skills": ["supabase"],
  "mcp": [{ "name": "github", "status": "connected" }],
  "mcp_configured": ["github"],
  "providers": [],
  "active_provider": "opencode",
  "issues": []
}
```

Use `tools[].read_blocked` to disable mutating tools while editing a
read-only agent. Use `program`/`codemode` only for display; custom agents
cannot enable Code Mode in v2.

### `GET /api/config/agents`

Lists built-ins and custom agents. Invalid custom files are listed with
`valid: false`, `errors`, and `effective: null` so the builder can repair them.

```json
{
  "agents": [{
    "key": "backend",
    "display": "Backend",
    "description": "Rust backend and server work",
    "tier": "workspace",
    "builtin": false,
    "editable": true,
    "deletable": true,
    "configured": true,
    "valid": true,
    "errors": [],
    "override": { "version": 2, "tools": ["read"] },
    "effective": {
      "access": "read",
      "access_spawned": "read",
      "prompt_profile": "general",
      "tools": ["read"],
      "code_mode": "off"
    }
  }],
  "issues": [],
  "capabilities": { "custom_agents": true, "custom_code_mode": true, "custom_routing": true }
}
```

### `GET /api/config/agents/{key}`

Resolves a key or alias. Built-in aliases (for example `coder`) resolve to
`developer`; aliases are never returned as their own agent.

### `PUT /api/config/agents/{key}`

Creates a custom agent or writes a built-in override. The body is the
definition above. The same path is used for built-in overrides; unknown keys
create a custom agent.

Optional query `?workspace_path=D:/project` validates project-local skills
strictly. Without it, skills not found in the harness catalog are warnings
because they may exist only in a workspace.

Success:

```json
{ "ok": true, "key": "backend", "builtin": false, "warnings": [], "effective": {} }
```

Validation failure (`400`):

```json
{
  "error": "invalid agent definition",
  "errors": ["unknown tool 'not_a_tool'"],
  "warnings": []
}
```

No file is written when `errors` is non-empty.

### `DELETE /api/config/agents/{key}`

- Custom key: deletes the JSON file (`deleted: true`).
- Built-in key: removes the override and restores the compiled seed
  (`reset: true`).

Chats that still reference a deleted custom agent fail explicitly on the next
run.

### `POST /api/config/agents/{key}/preview`

Body (all fields optional):

```json
{
  "draft": { "version": 2, "tools": ["read"], "prompt": { "persona": "..." } },
  "workspace_path": "D:/project",
  "mode": "build"
}
```

`draft` previews unsaved edits. Omit it to preview the saved definition.
The response is the exact runtime view:

```json
{
  "ok": true,
  "key": "backend",
  "builtin": false,
  "effective": {},
  "system_prompt": "You are ...",
  "prompt_tokens": 1234,
  "tools": {
    "direct": ["read", "grep"],
    "program": [],
    "direct_specs": [],
    "program_specs": []
  },
  "warnings": []
}
```

The same resolver composes the prompt and tool partition used by a real run.

### `POST /api/config/agents/{key}/duplicate`

Body:

```json
{ "key": "backend-copy", "display": "Backend Copy" }
```

Copies the effective surface into a safe custom definition:

- `execute`/hybrid Code Mode is preserved when the source offers it.
- Mutating tools are removed when the source is read-only.
- Built-in general-profile instructions are folded into the persona.
- `skills`/`mcp` are materialized from the current catalogs.

### `POST /api/chats/{chat_id}/run`

Existing request plus optional `agent` (alias: `agent_key`):

```json
{
  "prompt": "...",
  "workspace_path": "D:/project",
  "workspace_kind": "coding",
  "agent": "backend"
}
```

If `agent` is omitted, the registry routes by workspace kind/path **unless**
the chat already has an `agentKey`; the persisted agent wins. The resolved key
is stored on the chat and returned as `agentKey` by chat reads. An ambiguous
custom routing match falls back to the built-in heuristic and returns
`route_warning` in the run acceptance response.

## Errors

| Status | Meaning |
|---|---|
| `400` | Invalid definition, unknown model/provider, unknown tool/skill/MCP, unsafe access/tool combination |
| `404` | Unknown agent key |
| `409` | Reserved key/alias collision (reported as validation error today) |

## Compatibility

- v1 agent files are read; every save writes v2.
- `system_prompt` remains a v1 alias for `prompt.persona`. Do not send both.
- Legacy alias files (`coder.json`) are migrated at read time to a canonical
  built-in override (`developer`). They are never listed as separate agents;
  `DELETE /api/config/agents/developer` removes both the canonical and legacy
  alias files.
- Built-in `GET /api/config/agents` fields and the `PUT`/`DELETE` shape are
  unchanged; new fields are additive.
