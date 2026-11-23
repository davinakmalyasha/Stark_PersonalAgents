# Stark

Stark is a Rust agent harness with a Rune desktop and browser interface.

- `backend-rs/` — the Rust server: agent loop, providers, tools, MCP, storage,
  policy, sandbox, and voice.
- `desktop-rune/` — the Rune native and wasm UI, launcher, and packaging.
- `docs/architecture.md` — the current component and architecture reference.

The server is API-only; it does not serve a browser application. The Rune wasm
build is hosted separately. See `docs/architecture.md` for the run contract,
component boundaries, and build/run details.

## Install

Download the newest master installer from the rolling
[`rune-master` release](https://github.com/davinakmalyasha/Stark_PersonalAgents/releases/tag/rune-master).

The desktop follows `master`: every push builds a signed payload, and the app
checks the feed on launch and every five minutes. Updates are staged, applied
after the UI exits, and rolled back automatically if the new build fails to
start. The installed shortcut launches a GUI supervisor, so no terminal window
opens, and the launcher/UI/server carry the Stark icon and taskbar identity.
The Rune window uses one custom titlebar: navigation, open-session tabs, the
new-session action and the Windows caption buttons share a single 44pt row.

## Develop

```powershell
# Latest master: start/reuse the dev backend, then launch the Rune UI.
.\scripts\dev-rune.ps1

# Pull master first, then build and run.
.\scripts\dev-rune.ps1 -Pull

# Format, clippy and tests for the Rune and Rust workspaces.
.\scripts\check-rune.ps1
```

The dev script uses the repository harness root and `D:\Stark\resources\models`;
it never touches the installed app's data. The installed app lives in
`%LOCALAPPDATA%\Programs\StarkRune` and stores data in
`%LOCALAPPDATA%\StarkRune\data`.

The VPS deploys the same master commit through `build-linux` and `deploy-vps`;
its `/api/health` response reports the deployed revision in `build`.
