# 13 — Canvas mode and MCP (Rust)

## Canvas mode

`canvas_mode` is an orchestrator-scoped tool that watches agent chats and group
projects and wakes the orchestrator when watched work finishes or fails.

Persistence uses the existing `.deep_agents/chats.db` and the Python-compatible
tables:

- `pending_wakes`
- `canvas_watches`
- `canvas_events`

Actions:

- `available` — list agent chats and group projects
- `enter` — mode `single|multiple|all`, targets as chat ids or `group:<id>`,
  `wake_on`, `max_actions`, optional `until`
- `status` / `read` — watch snapshot plus recent events
- `message` — queue a follow-up for a watched chat (groups use `group_update`)
- `exit` — close the watch

The server's canvas loop runs every second. A due wake is delivered to an
active run via the follow-up queue; otherwise the target chat is started with
the appropriate agent scope. Normal, follow-up and group runs notify matching
watches and enqueue `[CANVAS EVENT]` wakes. The `/api/canvas/{chat_id}/status`
endpoint now reports the active watch.

## MCP

MCP uses the existing `.deep_agents/mcp_servers.json` format. Servers are
started in the background at API startup; tool schemas are cached and exposed
dynamically as `mcp__<server>__<tool>` in the owning scope.

Supported config fields include:

```json
{
  "servers": {
    "local": {
      "command": "npx",
      "args": ["-y", "some-mcp-server"],
      "env_vars": ["SOME_TOKEN"],
      "scopes": ["developer", "orchestrator"],
      "approval": "auto",
      "tool_timeout_sec": 60
    },
    "remote": {
      "url": "https://example.invalid/mcp",
      "bearer_token_env_var": "REMOTE_TOKEN",
      "http_headers": {"X-MCP-Readonly": "true"},
      "scopes": ["general"]
    }
  }
}
```

Transports: line-delimited JSON-RPC over stdio and Streamable HTTP. Calls are
serialized per server, bounded to the configured timeout, and filtered by
`enabled_tools` / `disabled_tools`. `approval: "prompt"` asks through
`ask_user` (and therefore works in voice mode); the default is `auto`.

Routes:

- `GET /api/mcp/servers`
- `POST /api/mcp/servers/{name}`
- `DELETE /api/mcp/servers/{name}`
- `POST /api/mcp/servers/{name}/restart`

There is no MCP settings UI yet; configure the JSON file or use these routes.

## Known limits

- MCP HTTP is the minimal Streamable-HTTP flow: initialize, list, call. OAuth
  discovery and long-lived sessions are not implemented.
- MCP child processes are trusted configuration; there is no OS sandbox.
- Canvas `/api/canvas/{chat_id}/status` exposes the watch but the full canvas
  snapshot is returned by the `canvas_mode read` tool, not the HTTP route.
- Canvas wake dispatch is in-process and relies on the server running; pending
  wakes survive restarts, active child processes do not.
