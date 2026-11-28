# stark-codemode-worker

Bun sidecar that runs Stark Code Mode programs. It hosts OpenCode v2's
`@opencode/codemode` runtime on `psychoscript` and forwards every host
operation to Rust over line-delimited JSON-RPC on stdio. The confined program
cannot reach the filesystem, network, or processes directly; OpenCode's
`fetch`, `path`, and `Bun` globals each cross back to Rust for permission and
plan/read-only checks.

- Runtime: Bun 1.4.4 (pinned, matches OpenCode), `effect` 4.1.0,
  `psychoscript` 0.2.1, `@opencode/codemode` 2.4.1, `minimatch` 10.2.5.
- `src/protocol.ts` — message shapes shared with the Rust client.
- `src/main.ts` — stdio loop, runtime cache, execute / cancel / shutdown,
  `tool_call` and `extension_call` RPC.
- `src/opencode/` — the OpenCode presentation layer and fetch/path/Bun
  extensions.

## Protocol

Rust → worker requests carry `{id, method, params}`; worker → Rust requests
carry `{id, method: "tool_call" | "extension_call", params}` and await
`{id, result}` / `{id, error}` replies. One line per message; values are
JSON-escaped so newlines never break framing.

- `initialize {tools: [{name, description, input_schema, freeform?}]}` →
  `{catalog: [{path, description, signature}]}`
- `execute {code, runId, cwd, full, tools}` → OpenCode `ExecuteOutput`:
  `{ok, text, error, toolCalls, files}`. `tools` is the exact inventory the
  Rust caller prepared; the worker caches runtimes by it, so concurrent
  sessions with different agents cannot see each other's tool set.
- `cancel {runId}` → `{cancelled}` (aborts fetch and interrupts the fiber)
- `shutdown` → `{ok}` then exit

Tool names are dotted program paths (`read`, `github.get_issue`); the Rust
host maps them back to dispatch names (`read`, `mcp__github__get_issue`).
`extension_call` names are `fetch_check`, `file_open`, `file_target`,
`file_read`, `file_exists`, `file_stat`, `file_write`, `file_unlink`, and
`glob_prepare`.

`full: false` selects the always-available `fetch` extension only; `full:
true` adds `path` and `Bun.file`/`Bun.write`/`Bun.Glob`. Like OpenCode, the
application does not persist psychoscript snapshots across a server restart:
running scripts become `Interrupted by server restart.` and the server
delivers that notice once.

## Develop

```sh
bun install
bun run ./src/main.ts        # speaks JSON-RPC on stdio
bun test                     # integration tests (spawns the worker)
bun run typecheck
```

MIT attribution: runtime semantics and the presentation/extensions layer
follow OpenCode (https://github.com/anomalyco/opencode, MIT), `packages/codemode`
2.4.1 at dev commit 9964772. Stark adapts dispatch, permissions, and jobs to
the Rust host.
