# Harness root and workspaces

The harness root owns app data. Workspaces own project files the agent edits.
They are separate by design.

```
D:\Stark\              app binaries (desktop + sidecar)
D:\StarkData\           harness root
  .env                  provider keys ONLY (never a workspace .env)
  .deep_agents\
    chats.db
    sessions\
    config\
      providers.json
      agents\
    backups\            chats-YYYYMMDD.db (newest 7)
    workspaces.json
D:\MyProject\...        workspaces (your existing projects, untouched)
```

## Rules

- The packaged app uses a **persisted** harness root (`D:\StarkData` by default).
  The launch folder is never consulted.
- `STARK_HARNESS_ROOT` overrides the persisted root (dev/advanced). A missing
  or unwritable override fails closed with a dialog — never a silent fallback.
- Dev builds use the persisted root too, unless `STARK_DEV_REPO_ROOT=1`.
- Keys come only from `<harness_root>\.env` (or Credential Manager key refs).
  A workspace `.env` is never read for provider keys.
- Child processes (bash, PTY, MCP servers, browser, git) inherit a **scrubbed**
  environment: `*_API_KEY`, `*_TOKEN`, `*_SECRET`, `*_PASSWORD`, and friends are
  removed. MCP servers only receive secrets explicitly listed in their config.
- `env`, `printenv`, bare `set`, `export -p`, and any shell read of `.env`
  (except `.env.example`) are denied by the shell policy.

## Settings

Title bar gear → Settings shows the install path, active harness root, and
workspaces, with a folder picker to move the harness root (restart to apply).

## One-time migration

Settings → Migration lists old roots (AppData, repo) holding chats/sessions/
config. Import copies them into the active root after backing the active files
up to `.bak`. Nothing merges automatically. A folder picker also allows
importing from any explicit path containing `.deep_agents` (e.g. a repo that
currently holds the live harness data).

## Endpoints

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/harness` | active harness root, install path, workspaces |
| GET | `/api/migration/status` | candidate old roots |
| POST | `/api/migration/import` | import `{source}` after `.bak` backup |

## Evidence

- `frontend/src-tauri/src/backend.rs` — persisted root, fail-closed, dialogs.
- `stark-server/src/main.rs` — requires `STARK_HARNESS_ROOT`, loads only the
  harness `.env`, daily `VACUUM INTO` backups, harness/migration endpoints.
- `stark-sandbox/src/secrets.rs` — secret detection + child scrubbing.
- `stark-policy/src/lib.rs` — shell denial of env dumps and `.env` reads.
- `stark-config`, `stark-store/src/workspaces.rs` — atomic writes with `.bak`.
- `SettingsModal.tsx` — paths, folder picker, migration UI.
