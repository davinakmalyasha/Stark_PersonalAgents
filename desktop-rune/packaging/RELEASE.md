# Rune release and update feed

The Rune app ships as a **per-user, versioned install**:

```text
StarkRune.exe                 stable launcher/supervisor
current.json                  atomic { version, previous }
app/<version>/                stark-rune.exe + stark-server.exe + runtime
models/                       shared voice models, not duplicated per version
updates/                      staged payloads
```

Never point Rune at the Tauri `latest.json`. The Tauri workflow publishes an
NSIS app for `frontend/`; Rune has its own feed and its own release repo.

## One-time setup

1. Create a public release repo, by default
   `davinakmalyasha/Stark_Rune_Releases`.
2. Add these secrets to the source repo:
   - `MINISIGN_SECRET_KEY` — the minisign secret key text.
   - `STARK_UPDATE_PUBKEY` — the matching **base64 public key only** (the
     second line of `minisign.pub`, without its comment). It is compiled into
     `updater.rs` with `option_env!` and also shipped as `update.pub`.
   - `RELEASES_TOKEN` — a token with release access to the public Rune repo.
   - Optional `WINDOWS_CERT_BASE64` and `WINDOWS_CERT_PASSWORD` for
     Authenticode. Minisign is required regardless.
3. Make sure `desktop-rune/Cargo.toml` `version` is the next release.

## Release

```bash
git tag rune-v2.6.5
git push origin rune-v2.6.5
```

`.github/workflows/rune-release.yml` builds the UI, server and launcher,
installs NSIS/minisign, stages the tree, creates the updater payload, signs it,
builds the optional installer, and publishes:

- `latest-rune.json` — `releases/latest/download/latest-rune.json`
- `StarkRune-<version>-x64.zip` + `.minisig` + `.sha256` (updater payload)
- `StarkRune-<version>-windows-x86_64.zip` (offline/first install)
- `StarkRune-Setup-<version>.exe` when NSIS is available
- `update.pub`

The manifest signature is the minisign signature over the payload zip. The app
checks the version, `min_version`, SHA-256, size, minisign signature, safe zip
paths, and the staged `--version` before writing `pending.json`.

## Why the launcher

The UI exits first, then `StarkRune.exe`:

1. stops the bundled server,
2. validates `app/<version>`,
3. atomically writes `current.json`,
4. starts the new server and UI,
5. rolls back to `previous` if the new UI fails within 20 seconds.

A running `.exe` is never replaced. The installer is only for first install or
repair. The launcher owns port 8124; `STARK_REUSE_SERVER=1` is the development
escape hatch for attaching to a backend started outside the launcher, and is
never appropriate for an installed build because it breaks UI/server lockstep.

## Local smoke test

```bash
# build the app, launcher and backend release binaries, then:
STARK_SERVER_BIN=/path/to/stark-server.exe \
STARK_UPDATE_PUBKEY_FILE=/path/to/update.pub \
MINISIGN_SECRET_KEY=/path/to/minisign.key \
bash desktop-rune/packaging/package.sh
```

Point a development build at a fixture feed with
`STARK_UPDATE_URL=http://127.0.0.1:.../latest-rune.json` and
`STARK_INSTALL_ROOT=/tmp/stark-rune-install`; use `pending.json` plus
`stark-launcher` to exercise apply/rollback. A dev checkout without
`STARK_INSTALL_ROOT` reports that updates are installed-app-only.
