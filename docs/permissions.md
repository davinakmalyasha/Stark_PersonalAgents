# Permissions

Stark's safety layer is an OpenCode-style permission engine. It runs **before**
a tool executes: `deny` blocks the call, `ask` prompts you, `allow` runs it.

No OS sandbox, no admin setup. Enforcement is in the tool path, not the prompt.

## Configuration

`.deep_agents/permissions.json` (harness root):

```json
{
  "permissions": [
    { "action": "bash", "resource": "*", "effect": "ask" },
    { "action": "bash", "resource": "git *", "effect": "allow" },
    { "action": "bash", "resource": "npm *", "effect": "allow" },
    { "action": "bash", "resource": "rm *", "effect": "deny" },

    { "action": "edit", "resource": "*", "effect": "allow" },
    { "action": "edit", "resource": "secrets/*", "effect": "deny" },

    { "action": "read", "resource": "*.env", "effect": "ask" },
    { "action": "glob", "resource": "*", "effect": "allow" },
    { "action": "grep", "resource": "*", "effect": "allow" },

    { "action": "doom_loop", "resource": "*", "effect": "ask" }
  ]
}
```

## Semantics (copied from OpenCode)

- **Last matching rule wins.** Put the catch-all first, specifics after.
- An operation that touches several resources: **any deny wins**, otherwise
  **any ask**, otherwise allow.
- Wildcards: `*` matches any run (within one path segment for path actions),
  `**` crosses directories, `?` matches one character.
- `~` / `$HOME` expand to your home directory.

## Actions

| Action | Resource | Wired |
|---|---|---|
| `bash` | the command string | ✅ `bash` tool |
| `read` | file path (relative to workspace) | ✅ `read` |
| `edit` | file path (write/edit) | ✅ `write`, `edit` |
| `glob` | glob pattern | ✅ |
| `grep` | search pattern | ✅ |
| `doom_loop` | tool name | ⚠️ repeat guard reports it, not yet routed through rules |
| `external_directory` | absolute path | ⚠️ file tools already refuse paths outside the workspace |
| `webfetch` / `websearch` | URL / query | ⚠️ not gated yet |

## Defaults (no config file)

- Everything is **allow**.
- `external_directory` and `doom_loop` are **ask**.
- `.env` and `.env.*` reads are **ask** (`.env.example` is allowed).

## Subagents

Subagents are non-interactive: an `ask` result is a hard block for them. They
never silently inherit an approval.

## Evidence

- `stark-policy/src/permissions.rs` — evaluator + tests (last-match, deny-wins,
  defaults, wildcards).
- `stark-tools/src/permissions.rs` — gate wired into `bash`, `read`, `write`,
  `edit`, `glob`, `grep`.
- A denied call never spawns: the gate returns before the process layer.
