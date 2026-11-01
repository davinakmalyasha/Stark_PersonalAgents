# Providers and agents

Provider routing and per-agent configuration are declarative files. Environment
variables hold **API keys only**; they no longer select the provider, base URL,
or model.

```
.deep_agents/
  config/
    providers.json
    agents/
      developer.json
      orchestrator.json
      ...
```

Open **Agents & providers** (click the provider name in the composer) to edit
the same files through the UI. Files are hot-reloaded: the next run uses the
new values, no restart.

## providers.json

```json
{
  "version": 1,
  "active": "opencode",
  "providers": {
    "opencode": {
      "kind": "openai-compatible",
      "label": "OpenCode Go",
      "base_url": "https://opencode.ai/zen/go/v1",
      "key_env": ["OPENCODE_API_KEY", "DEEPSEEK_API_KEY"],
      "default_model": "flash",
      "models": {
        "flash": {
          "id": "deepseek-v4.1-flash",
          "label": "DeepSeek V4.1 Flash",
          "context": 131072,
          "reasoning": true,
          "aliases": ["deepseek/deepseek-v4.1-flash-1017", "deepseek-v4.1-flash"]
        }
      }
    }
  },
  "matrix": {
    "opencode": {
      "orchestrator": { "model": "flash", "thinking": "low" },
      "workspace":    { "model": "flash", "thinking": "high" },
      "specialist":   { "model": "flash", "thinking": "medium" }
    }
  }
}
```

- `key_env` accepts one name or a list. The first non-empty value wins;
  comma-separated lists are accepted.
- `matrix` supplies tier defaults. Any provider can be active.
- Unknown model keys/ids/aliases are rejected with the available list. A model
  from another provider is never sent silently.

## agents/<key>.json

Every field is optional. Missing or `null` means inherit.

```json
{
  "version": 1,
  "key": "developer",
  "provider": null,
  "model": null,
  "thinking": "high",
  "tools": ["read", "write", "edit", "bash", "grep", "glob"],
  "skills": ["agent-browser"],
  "mcp": ["github"],
  "system_prompt": "Optional replacement for the agent preamble."
}
```

- `provider` overrides the active provider for this agent.
- `model` is a key, canonical id, or alias for the selected provider.
- `thinking` is `none`/`minimal`/`low`/`medium`/`high` (or `inherit`).
- `tools` is an allowlist on top of the scope contract. `null` = contract
  default; `[]` or `"none"` = no tools.
- `skills` filters the skill catalog. `null` = all.
- `mcp` filters MCP servers by name (`mcp__<server>__*`). `null` = all
  configured servers.
- `system_prompt` replaces the agent preamble; harness/tool/safety text still
  applies.

## Resolution order

```
provider = agent.provider  ?? providers.active
model    = agent.model     ?? matrix[provider][tier].model ?? provider.default
thinking = agent.thinking  ?? matrix[provider][tier].thinking
tools    = agent.tools     ?? scope contract
```

The resolved values are shown in the modal and returned by:

- `GET /api/config/agents`
- `GET /api/config/agents/<key>`
- `GET /api/provider?agent=<key>`

## Migration

Existing `agents` rows keep their old `model` column for display, but durable
runs ignore it and resolve from config. A Developer created under OpenRouter
therefore runs on the current provider's canonical model after upgrading, with
no database edit.

If a user-edited file is invalid, the agent falls back to the built-in defaults
and the error is listed under `issues` instead of breaking the chat.

## Endpoints

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/provider` | active provider, models, key status, issues |
| GET | `/api/provider?agent=developer` | plus that agent's effective config |
| POST | `/api/provider` | switch active provider (`{"provider":"opencode"}`) |
| GET | `/api/models` | active provider's model catalog |
| GET | `/api/config/agents` | every agent + effective config |
| GET | `/api/config/agents/<key>` | one agent override + effective |
| PUT | `/api/config/agents/<key>` | save an override |
| DELETE | `/api/config/agents/<key>` | reset to built-in |

## Evidence

- `stark-config` — schema, validation, hot reload, resolution precedence,
  alias mapping, key resolution, and tests.
- `stark-server` — provider registry, per-run provider selection, endpoints,
  and migration away from the stale `agents.model`.
- `stark-tools` — tool/MCP allowlists enforced at advertisement and dispatch.
- `stark-core` — per-agent prompt/skills/effort wiring.
