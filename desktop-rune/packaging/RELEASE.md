# Master release and update feed

The Rune desktop follows `master` directly. Every push to master builds a
signed Windows payload, publishes it to a dedicated feed repo, and the
installed app picks it up on its own. There is no nightly schedule, release
tag, or manual check step.

The same push also triggers `build-linux` and `deploy-vps`, so the VPS runs the
same commit as the desktop. `/api/health` reports the deployed commit in
`build`.

## Layout

```text
StarkRune.exe                 stable GUI-subsystem launcher/supervisor
current.json                  atomic { version, previous }
app/<version>/                stark-rune.exe + stark-server.exe + runtime
models/                       shared voice models, not duplicated per version
updates/                      staged payloads
stark.ico                     shortcut/installer icon
set-appid.ps1                 installer helper for the taskbar identity
```

The launcher, UI and bundled server all embed `stark.ico`. The launcher is a
Windows GUI binary, so a shortcut never opens a console window. The shortcuts
carry `com.stark.rune` as their AppUserModelID and the UI sets the same ID, so
the pinned shortcut groups with the running Rune window.

The version is computed per master build:

```text
<next-patch>-master.<run_number>.<run_attempt>
```

With `desktop-rune/Cargo.toml` at `2.7.0`, the first build is
`2.7.1-master.1.1`; the next is `2.7.1-master.2.1`, and so on. Numeric
pre-release identifiers compare numerically, so `master.10` is newer than
`master.9`.

## One-time setup

1. Create the public feed repo:

   ```text
   davinakmalyasha/Stark_Rune_Releases
   ```

   The build-only ONNX Runtime 1.26 DLL is published once as the `rune-deps`
   prerelease in that repo. It is a dependency, not an update target; the
   `rune-master` workflow downloads it by tag.

2. Generate a passwordless minisign keypair outside the repository:

   ```bash
   minisign -G -W -p minisign.pub -s minisign.key
   ```

   CI cannot unlock a password-protected key. Keep `minisign.key` in a
   password manager or another secure backup. If it is lost, existing installs
   can never verify another update.

3. Add these secrets to the source repo:

   - `MINISIGN_SECRET_KEY` — the `minisign.key` contents.
   - `STARK_UPDATE_PUBKEY` — the `minisign.pub` contents.
   - `RELEASES_TOKEN` — a fine-grained PAT scoped to `Stark_Rune_Releases`
     with **Contents: Read and write**.

   Optional Authenticode signing: `WINDOWS_CERT_BASE64` and
   `WINDOWS_CERT_PASSWORD`. Minisign is required regardless.

4. Add a `models` junction in the install root if voice should reuse the old
   local model bundle:

   ```powershell
   New-Item -ItemType Junction `
     -Path "$env:LOCALAPPDATA\Programs\StarkRune\models" `
     -Target "D:\Stark\resources\models"
   ```

## What a master push does

`.github/workflows/rune-master.yml`:

1. Waits for the commit's `rune-validate` / `rust-validate` checks when those
   workflows were triggered.
2. Edits the workspace version to the next-patch master version.
3. Builds the UI, launcher and bundled server.
4. Signs the payload with minisign.
5. Publishes `latest-rune.json`, the payload, signature, checksum, portable
   zip, installer and `update.pub` to `Stark_Rune_Releases`.
6. Updates the rolling `rune-master` release on the main repo so the
   downloadable installer is always the newest master build.

The feed release is created as a draft and only marked latest after every
asset is uploaded. A cancelled or failed build never becomes the update target.

`.github/workflows/build-linux.yml` builds the server artifact for every master
push, and `deploy-vps.yml` deploys that exact artifact after validation. The
deploy script writes `STARK_BUILD_SHA` into the service environment, and
`verify-cloud.sh` checks that `/api/health` reports the same revision.

## How the desktop updates

`updater.rs` checks `latest-rune.json` on launch and every five minutes. A
newer version is downloaded, checked against its size and SHA-256, verified
with the embedded minisign public key, extracted under `app/<version>`, and
validated before `pending.json` is written.

The app restarts when it is safe (no live run or draft); the launcher then
switches `current.json`, starts the new server and UI, and rolls back to the
previous version if the new UI exits within 20 seconds. `blocked.json` records
a version that failed to start so it is not retried.

The updater payload contains only `app/<version>`. Changes to the launcher,
the installer icon or the shortcut identity need one new Setup run; after that
the normal five-minute master updates continue.

The update row in Settings shows the current version and status. Updates apply
automatically; there is no manual check button. The browser build reports that
web updates arrive with the server deploy.

## Installing

Download the newest installer from the rolling `rune-master` release on the
main repo, or from `Stark_Rune_Releases`. Install per user; it runs
`StarkRune.exe` at the end.

Data lives in `%LOCALAPPDATA%\StarkRune\data` (override with
`STARK_HARNESS_ROOT`). The installer preserves it across updates and
uninstalls. The old Tauri app and `D:\StarkData` are not touched.

## Verify a release

```bash
# The feed resolves to the newest master build.
curl -fsSL \
  https://github.com/davinakmalyasha/Stark_Rune_Releases/releases/latest/download/latest-rune.json

# The installer feed is public and the payload matches its checksum.
gh release view rune-master -R davinakmalyasha/Stark_PersonalAgents

# The VPS runs the same commit.
curl -fsS http://127.0.0.1:8123/api/health   # on the VPS
```

`scripts/verify-cloud.sh` runs the VPS-side health, backup/restore and
build-revision checks after every deploy.

## Local updater test

For a scratch test without production keys:

```bash
STARK_UPDATE_URL=http://127.0.0.1:8765/latest-rune.json \
STARK_UPDATE_PUBKEY="$(sed -n '2p' test-minisign.pub)" \
STARK_INSTALL_ROOT=/tmp/stark-rune-test \
  ./StarkRune.exe
```

Build a staged tree with `bash desktop-rune/packaging/package.sh --stage-only`
and serve its `latest-rune.json` plus payload. To test rollback, stage a fake
newer `stark-rune.exe` that exits immediately; the launcher must return to the
previous version and write `blocked.json`.

## Key custody

- Never commit `minisign.key`.
- Keep one offline backup outside the repo.
- Rotating the key requires a full installer for every existing install; the
  embedded key cannot verify releases signed by a different key.
