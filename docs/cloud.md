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
| `/var/lib/stark-data` | harness root: `chats.db`, `sessions/`, `config/`, `.env`, exports |
| `/srv/stark-projects` | project folders (`STARK_PROJECTS_ROOT`) |

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

## Portable data (exports)

`Settings → Backup & move` (or `GET /api/data/export`) produces one zip:

- `manifest.json` — format, app, contract version, export time, projects root
- `.deep_agents/chats.db` — `VACUUM INTO` snapshot (never a live file or WAL)
- `.deep_agents/workspaces.json` — portable records (`%PROJECTS_ROOT%` + rel)
- `config/`, `sessions/`, `memory/`, `uploads/`, `ui_state.json`

Excluded: `.env`, voice models, logs, `*.db-wal`, `*.db-shm`, old backups.

`workspaces.json` stores `{ "root": "%PROJECTS_ROOT%", "rel": "coffee-shop" }`
next to the absolute `path`. On read, `path` is recomputed from the machine's
`STARK_PROJECTS_ROOT` (Windows default `D:\MyProject`, Linux
`/srv/stark-projects`). A folder that is not under the projects root keeps its
absolute path and is reported as machine-specific.

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

## Limits and guardrails

- `STARK_MAX_CONCURRENT_RUNS` (default 2) — overflow gets HTTP 429, never OOM.
- `STARK_RUN_IDLE_TIMEOUT_SEC` (default 900) — a provider that stops emitting
  fails the run instead of hanging forever.
- Drain marker refuses new runs during deploys.
- 2GB swap on the 20GB disk; `vm.swappiness=10`.
