# Desktop release + auto-update

## How auto-update works

- Tauri updater plugin, signed artifacts only. The public key is embedded in
  `frontend/src-tauri/tauri.conf.json` (`plugins.updater`); the private key
  lives outside the repo (`~/.tauri/stark.key`) and in the
  `TAURI_SIGNING_PRIVATE_KEY` repo secret. Lose the private key and shipped
  apps can never verify a future update — back it up.
- Feed: `releases/latest/download/latest.json` on the **public** repo
  `davinakmalyasha/Stark_Releases`. The source repo is private, and GitHub
  returns 404 for unauthenticated release downloads, so artifacts live in the
  public releases repo while the code stays private. The app checks on every
  launch (`frontend/src/utils/appUpdate.ts`), prompts once, downloads,
  installs, and relaunches. Offline or unreachable feed = silent no-op.
- Coverage: the NSIS bundle **and** the `stark-server` sidecar update
  together. Versions compare by semver against `tauri.conf.json` `version`.

## Cutting a release

1. Bump `version` in `frontend/src-tauri/tauri.conf.json`.
2. Commit, then tag the same number: `git tag v2.1.0 && git push origin v2.1.0`.
3. CI (`release.yml`) builds the backend release binary, the frontend, and the
   signed NSIS bundle, then publishes the GitHub Release with `latest.json` to
   `davinakmalyasha/Stark_Releases` (public).
4. First-run note: CI runners have no voice models, so CI releases ship
   without bundled STT/TTS (the app runs; STT needs a models-bearing build).
   Local `npx tauri build` on a machine with `backend-rs/models` bundles them.

## Local rebuild + reinstall (dev loop)

```
cd backend-rs && cargo build --release -p stark-server
cd ../frontend && npx tauri build   # runs build + prepare-sidecar
```

Run the `.exe` installer from `frontend/src-tauri/target/release/bundle/nsis/`.
Close the running app first — Windows locks the installed `.exe`.
