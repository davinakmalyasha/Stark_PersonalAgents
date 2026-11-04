# Stark Cloud (VPS runner)

One always-on machine hosts the harness. The desktop app and the phone are
remote controls onto the same server; they never hold live state. That single
rule removes sync/conflict handling entirely: SQLite has exactly one writer.

```
phone ──┐
        ├─(Tailscale + pairing token)─► VPS: stark-server (24/7)
laptop ─┘                                   │
                                            ├─ agents (orchestrator + Developer/General/House)
                                            ├─ /srv/stark-projects (code, git)
                                            └─ /var/lib/stark-data (chats, sessions, config)
```

## Layout

| Path | Contents |
|---|---|
| `/opt/stark/releases/<stamp>/stark-server` | immutable release binaries |
| `/opt/stark/current` | symlink to the active release (systemd ExecStart) |
| `/opt/stark/current/frontend` | frontend dist served by the server |
| `/var/lib/stark-data` | harness root: `.deep_agents/` (chats, registry, memory, config, tasks, exports), `.env` |
| `/srv/stark-projects` | project folders (`STARK_PROJECTS_ROOT`) |

## Workspace model

A workspace is an identity, not a path. The registry at
`<harness>/.deep_agents/workspaces.json` stores one record per project:

```json
{
  "id": "ws-a1b2c3d4",
  "name": "coffee-shop",
  "root": "%PROJECTS_ROOT%",
  "rel": "coffee-shop",
  "repo": "https://github.com/owner/coffee-shop",
  "branch": "main"
}
```

- `id` is stable for the lifetime of the project. Chats, delegated agents,
  canvas watches, and memory are keyed by it, so re-cloning or moving never
  forks a project into a duplicate.
- `name` is display metadata. Delegation resolves an exact id first, then an
  exact unique name; an ambiguous name returns the candidate ids instead of
  guessing.
- `rel` is the folder relative to the machine's `STARK_PROJECTS_ROOT`. The
  absolute `path` is derived at runtime and is never sent to the model.
- `repo` is optional. When present, a missing folder is recoverable with
  `workspace(action="clone", workspace_id="ws-…")` or the Settings **Clone**
  button. The id does not change.

The system prompt describes the run with its identity and relative folder:

```xml
<workspaces>
  <workspace id="ws-a1b2c3d4" name="coffee-shop" primary="true" status="ready">
    <folder>coffee-shop</folder>
    <repo>owner/coffee-shop</repo>
  </workspace>
</workspaces>
```

`status` is `ready`, `missing`, `worktree`, or `virtual`
(General/House/Personal/DFD). Isolated subagents run in a worktree and report
`worktree` with `<folder>.</folder>`, because their directory is not the
registered project folder.

Tool results use the same relative folder. Absolute paths stay in logs and the
UI, so transcripts and exports stay portable without a rewrite pass.

### Instructions and memory

- `<projects root>/<rel>/AGENTS.md` (or `AGENTS.override.md`) — user-owned
  project rules. Loaded from the project root down to the workspace, one file
  per directory, 32 KiB cap. Files closer to the workspace override earlier
  guidance, and direct system/developer/user instructions take precedence.
  The agent reads these; learned notes live in the separate memory store.
- `<harness>/.deep_agents/AGENTS.md` — global preferences.
- `<harness>/.deep_agents/memory/<workspace_id>/` — agent-owned notes and a
  compact `MEMORY.md` index. Keyed by workspace id, so it survives re-cloning.
  Every agent scope receives the compact index; details stay behind the
  `memory` tool.

### Run checkpoints

Each run starts with a baseline snapshot of the workspace (excluding `.git`,
`node_modules`, `target`, `dist`, and similar heavy folders) and records files
before the agent edits them. Restore is available per run, per user message, or
per file from the server API. Checkpoints are local safety data: they live under
the workspace's `.deep_agents/snapshots/` and are not part of the portable
export.

### Rollout log (canonical trace)

Every chat has one append-only JSONL rollout at
`<harness>/.deep_agents/rollouts/YYYY/MM/DD/rollout-<timestamp>-<chat_id>.jsonl`.
Each line is `{"timestamp","ordinal","type","payload"}` with Codex-compatible
wire names: `session_meta`, `turn_context`, `response_item` (`message`,
`reasoning`, `function_call`, `function_call_output`), `event_msg` (`error`,
`warning`, `turn_started`, `turn_complete`, `turn_aborted`), `compacted`, and
`token_usage_record`. `turn_complete` carries an optional error and the last
message; `turn_aborted` carries the turn id (when known), reason
(`cancelled`), and optional error.

The rollout is written as work happens: user turns, reasoning bursts,
assistant text, tool calls/results, usage, compaction, and errors/warnings.
A failure records the partial text and reasoning first, then an `error` event
plus a `turn_complete` carrying the error; a user stop records a
`turn_aborted`. The `chats.db` projection keeps the same order with an
`error` role message (the UI renders it inline, never replacing history).
Reasoning is durable, not SSE-only, and `metadata_json` is persisted so
reloads show thinking and the timeline.

Resume never mutates the file. An unfinished `turn_started` with no matching
`turn_complete`/`turn_aborted` is the interruption signal, derived on read;
new chats have no turns and running chats are skipped, so neither shows a
false warning. A truncated final line is ignored and unknown item types are
skipped. Dangling tool calls are dropped when rebuilding history, so the next
turn re-issues the work instead of sending an unanswered call. A brand-new
rollout with legacy history copies the old user/assistant messages once, so
existing chats keep their context. Request retries (default 4) and stream
retries (default 5) use `Retry-After` when present, otherwise 200ms exponential
backoff with jitter; stream idle defaults to 300s (`STARK_STREAM_IDLE_TIMEOUT_SEC`).
`GET /api/stream/{chat_id}` accepts `Last-Event-ID` (a rollout ordinal) and
replays later items before attaching live. Rollouts are part of the portable
export.

## Deploy (drain-safe, auto-rollback)

```powershell
pwsh scripts/deploy-stark.ps1 -VpsHost <ip> -Port <ssh-port>
```

1. Downloads the newest `build-linux` artifact from Actions.
2. Uploads it, runs `scripts/deploy-vps.sh` on the server.
3. The script writes `<harness>/.drain` (new runs get 503), waits up to 15 min
   for active runs to settle, swaps `current`, restarts, health-checks, and
   rolls back automatically when `/api/health` fails.

The GitHub workflow builds on `ubuntu-24.04` (older glibc than the 26.04 VPS,
which is the compatible direction). The VPS never compiles.

## Auth

- Non-loopback `/api/*` requires `X-API-Token`.
- `GET /api/pairing` (loopback only) returns the legacy pairing token used by
  the desktop QR dialog.
- Per-device tokens: `POST /api/pairing/devices {"name":"phone"}` returns a
  token once; `GET /api/pairing/devices` lists devices with `last_seen`;
  `DELETE /api/pairing/devices/{id}` revokes one device without touching the
  others. Revoked tokens are rejected immediately.
- Tailscale is the network boundary: port 8123 is allowed only on
  `tailscale0`, and UFW still denies it on the public interface.

## Projects from repositories

`workspace(action="create", name="coffee-shop", repo="owner/coffee-shop")`
clones the repository into `<projects root>/coffee-shop`, registers it, and
stores the URL plus optional branch on the workspace record. Accepted forms:
`owner/repo`, `github.com/owner/repo`, HTTPS, `git@…`, `file://…`. Private
repositories need a deploy key on the server (`~stark/.ssh` + GitHub deploy
key); the tool never receives credentials.

A workspace whose folder is missing on this machine (after an import, a disk
loss, or a new box) can be restored with:

- `workspace(action="clone", workspace_id="ws-…")` from the agent, or
- the **Clone** button next to the folder in `Settings → Workspaces`.

Re-cloning keeps the existing workspace id, so chats and sessions reconnect to
the same project instead of creating a duplicate. The tool reports the id and
the folder relative to the projects root, never the absolute path.

## Portable data (exports)

`Settings → Backup & move` (or `GET /api/data/export`) produces one zip:
- `manifest.json` — format, app, contract version, export time, projects root,
  and the exact `included` / `excluded` lists
- `.deep_agents/chats.db` — `VACUUM INTO` snapshot (never a live file or WAL)
- `.deep_agents/workspaces.json` — portable records (`%PROJECTS_ROOT%` + rel)
- `.deep_agents/config/`, `sessions/`, `memory/`, `uploads/`, `skills/`,
  `tasks/` (plans), `groups/`, `AGENTS.md`, `AGENTS.override.md`,
  `schedules.db`, `dfd_clients.json`, `house_state.json`, MCP servers, notes,
  UI state

Excluded and listed in the manifest: `.env`, `jobs.db` (transient queue),
workspace checkpoints, voice models, logs, old exports/imports/backups,
`*.db-wal`, `*.db-shm`.

`workspaces.json` stores `{ "root": "%PROJECTS_ROOT%", "rel": "coffee-shop" }`
next to the absolute `path`. On read, `path` is recomputed from the machine's
`STARK_PROJECTS_ROOT` (Windows default `D:\MyProject`, Linux
`/srv/stark-projects`). A folder that is not under the projects root keeps its
absolute path and is reported as machine-specific. Only the registry and the UI
see `path`; the model sees `rel`.

## Import

`POST /api/data/import` (multipart `file`, optional `projects_root`) extracts to
`.deep_agents/imports/<id>/`, validates `manifest.json` + `chats.db`, and writes
`pending_import.json`. Nothing is replaced while the server is running. On the
next start, before the store opens:

1. Current `chats.db`/`workspaces.json`/dirs are copied to
   `.deep_agents/backups/pre-import-<stamp>/`.
2. Staged files replace the harness copies.
3. `workspaces.json` records under the old root are rebased onto the new root.
4. After the store opens, `chats.workspace_path` (plus canvas watches/wakes)
   are rebased with the same prefix mapping.

## Snapshots

Every `STARK_SNAPSHOT_HOURS` (default 24) a portable export is written under
`.deep_agents/exports/`, newest three kept. If `STARK_SNAPSHOT_PASSPHRASE` is
set, the zip is encrypted with AES-256-CBC + PBKDF2 via `openssl enc` (secret
passed through the environment, never argv) and the plaintext is removed. Set
`STARK_SNAPSHOT_DIR` to also copy the encrypted file to a repo/mount.

Restore drill (safe: extracts to a temp dir, never touches live data):

```bash
STARK_SNAPSHOT_PASSPHRASE=... bash scripts/restore-drill.sh <backup.zip[.enc]>
```

It verifies the manifest, extracts `chats.db`, and runs `PRAGMA integrity_check`
plus a chat count. Run it after the first snapshot and after any restore.

## Schedules

The scheduler queries `next_run_at <= now`, so an overdue wake fires once on
the first tick after a restart/deploy instead of being skipped. A wake that was
due several times during downtime collapses into one run (no burst replay).

## Limits and guardrails

- `STARK_MAX_CONCURRENT_RUNS` (default 2) — overflow gets HTTP 429, never OOM.
- `STARK_RUN_IDLE_TIMEOUT_SEC` (default 900) — a provider that stops emitting
  fails the run instead of hanging forever.
- Drain marker refuses new runs during deploys.
- 2GB swap on the 20GB disk; `vm.swappiness=10`.
- `STARK_LANDLOCK=1` (Linux) confines spawned children: reads/exec allowed
  everywhere; writes only inside the workspace, `/tmp`, `~/.cargo`,
  `~/.rustup`, `~/.npm`, `~/.cache`, `~/.config`, `~/.local`, and `/dev/null`.
  Read-only mode allows no writes; `danger-full-access` skips Landlock.
  Failure to apply fails the spawn (no silent fallback). PTY sessions are not
  covered (portable-pty has no `pre_exec` hook). The system prompt and startup
  log report `landlock (opt-in)` only when it is actually enabled.

## Phone behaviour

- PWA manifest + icons → installable; the service worker keeps the shell
  offline and never caches `/api`.
- Offline outbox: text prompts are queued in `localStorage` and sent on
  reconnect (and every 30s); attachments require connectivity and are never
  queued.
- Notifications: one completion notification while the page is hidden. Closed
  tabs need server push (not wired yet).
