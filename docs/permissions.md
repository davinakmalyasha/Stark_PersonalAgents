# Permissions

Stark's safety layer is an OpenCode-style permission engine. It runs **before**
a tool executes: `deny` blocks the call, `ask` prompts you, `allow` runs it.
No OS sandbox and no admin setup — enforcement is in the tool path.

## Configuration

`.deep_agents/permissions.json` (harness root):

```json
{
  "auto_approve": false,
  "permissions": [
    { "action": "bash", "resource": "*", "effect": "ask" },
    { "action": "bash", "resource": "git *", "effect": "allow" },
    { "action": "bash", "resource": "npm *", "effect": "allow" },
    { "action": "bash", "resource": "rm *", "effect": "deny" },

    { "action": "read", "resource": "*.env", "effect": "ask" },
    { "action": "webfetch", "resource": "*", "effect": "allow" },
    { "action": "websearch", "resource": "*", "effect": "allow" },

    { "action": "external_directory", "resource": "*", "effect": "ask" },
    { "action": "doom_loop", "resource": "*", "effect": "ask" }
  ],
  "agents": {
    "explore": {
      "permissions": [{ "action": "edit", "resource": "*", "effect": "deny" }]
    }
  }
}
```

## Semantics

- **Last matching rule wins.** Catch-all first, specifics after.
- Multiple resources in one operation: **any deny wins**, then any ask.
- Wildcards: `*` matches any run (within one path segment for path actions),
  `**` crosses directories, `?` one character.
- `~` / `$HOME` expand to the home directory.
- Agent rules are appended after global rules, so they win on overlap.

## Actions

| Action | Resource | Wired |
|---|---|---|
| `bash` | command string | ✅ |
| `read` | file path | ✅ |
| `edit` | file path (write/edit) | ✅ |
| `glob` / `grep` | pattern | ✅ |
| `webfetch` | URL | ✅ |
| `websearch` | queries | ✅ |
| `doom_loop` | tool name | ✅ (same tool + args at the repeat limit) |
| `external_directory` | absolute path | ✅ for shell paths outside the workspace |

## Ask outcomes

`Allow once`, `Allow for session` (saved for the run), `Deny`.
Subagents are non-interactive: `ask` becomes a hard block.

## Auto mode

`STARK_AUTO_APPROVE=1` (or `"auto_approve": true`) turns asks into allows.
Explicit `deny` rules always win.

## Defaults without a config file

- Everything **allow**.
- `.env` / `.env.*` reads **ask** (`.env.example` allowed).
- `external_directory` and `doom_loop` **ask**.

## Evidence

- `stark-policy/src/permissions.rs` — evaluator, agent merge, auto-approve,
  tests (last-match, deny-wins, defaults, wildcards, disk merge).
- `stark-tools/src/permissions.rs` — gate (session allow, ask outcomes, auto).
- Canary test `denied_bash_never_spawns`: a denied command with a redirect
  leaves no file, proving the process layer is never reached.
