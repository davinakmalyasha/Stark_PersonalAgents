# 12 — Setup & Branching

Everything needed before the first Rust commit: toolchain, repo safety,
branch, reference checkout, and workspace bootstrap.

---

## 1. Install Rust (Windows)

> **Executed choice:** local Phase 0 uses **GNU on D:** (`1.99.1`,
> `x86_64-pc-windows-gnu`, WinLibs). C: stays untouched. CI uses MSVC. The
> MSVC instructions below are the alternative/CI path.

### 1.1 MSVC build tools (CI / alternative)

Rust on Windows uses the MSVC linker. Install Visual Studio Build Tools with
the C++ workload:

```powershell
winget install Microsoft.VisualStudio.2022.BuildTools
```

In the installer select **Desktop development with C++** (MSVC v143 + Windows
SDK). Without it, `cargo build` fails with `link.exe not found`.

### 1.2 rustup / stable

```powershell
winget install --id Rustlang.Rustup -e
rustup default stable-msvc
rustup update stable
```

Manual fallback: download `rustup-init.exe` from https://win.rustup.rs/x86_64,
run it, accept:
- default host: `x86_64-pc-windows-msvc`
- default toolchain: `stable`
- modify PATH: yes

Restart the shell after installation.

### 1.3 Verify

```bash
rustc -V
cargo -V
rustup show
```

Expected: `rustc 1.99.x` (or newer); `stable-x86_64-pc-windows-msvc`.

### 1.4 Components and helpers

```bash
rustup component add rustfmt clippy rust-analyzer
cargo install cargo-nextest --locked
cargo install cargo-watch --locked
```

Optional:
```bash
cargo install cargo-audit --locked
cargo install cargo-deny --locked
cargo install cargo-expand --locked
```

### 1.5 Smoke test

```bash
cargo new hello-rust
cd hello-rust
cargo run
```

You should see `Hello, world!`. Delete the test project afterward.

### 1.6 Editor

- VS Code + extension `rust-lang.rust-analyzer`
- Or JetBrains RustRover (free non-commercial)

Enable format-on-save with `rustfmt`; use `clippy` as the default check.

---

## 2. Repo state before branching

Safety sequence completed on 2026-10-22:

| Fact | Value |
|---|---|
| Current branch | `rust-phase-0` (from `rust-rewrite`) |
| Tracking | `origin/rust-phase-0` |
| Base | `rust-rewrite` = `master` = `8b40ce4` |
| Dirty entries | 0 before Phase 0 work |
| Default branch | `master` (no `main`) |
| `.env` | ignored, untracked (safe) |
| `pre-rust-rewrite` tag | `8b40ce4` |
| Python suite | **328 passed** |
| Frontend build | passed |
| Rust toolchain | **1.99.1 GNU** on D:, WinLibs MinGW on D:, C: ~41 GB free |

The working tree was snapshotted as `ca6b8a0`, docs as `b09fd4d`, and merged to
`master` as `8b40ce4`. No content was left uncommitted.

**Important:** there is no `main` branch. Treat **`master` as main** unless you
decide to rename it.

---

## 3. Safety sequence (executed 2026-10-22; keep for reference)

> Do not re-run this blindly. It is the historical sequence that produced the
> current branches and tag.

```bash
git fetch --all --prune
git status
git diff --stat

# 1) Verify the snapshot is sane (recommended, not strictly blocking)
python -m pytest tests/ -q
cd frontend && npm run build && cd ..

# 2) Checkpoint the current work on development
git add -A
git commit -m "checkpoint: canvas, notes, schedules, chat continuity before rust rewrite"
git push origin development

# 3) Bring master (the mainline) up to date
git checkout master
git pull --ff-only origin master
git merge --no-ff development -m "merge development before rust rewrite"
git push origin master

# 4) Tag the pre-rewrite state
git tag pre-rust-rewrite
git push origin pre-rust-rewrite

# 5) Create the rewrite branch
git checkout -b rust-rewrite master
git push -u origin rust-rewrite
```

Rules:
- If tests/build fail, **push `development`** (safety), but **do not merge to
  `master`** until green. The branch can be created from `development` in that
  case.
- Review deletions (`git status` shows 9) before committing; they appear to be
  intentional task→schedule/canvas replacements.
- Never commit `.env`, `venv*`, `node_modules`, `.deep_agents`, or `target/`.

### If you insist on a literal `main`

GitHub → Settings → Branches → change the default branch, then:

```bash
git branch -m master main
git push -u origin main
# delete the old remote branch only after verifying everything
git push origin --delete master
```

This is a separate migration; `master` is recommended.

---

## 4. Clone Codex for reference (outside the repo)

```bash
git clone --depth 1 https://github.com/openai/codex.git D:\ref\codex
```

Read-only. Do not add it as a submodule, dependency, or vendored tree.

Optional references:

```bash
git clone --depth 1 https://github.com/aaif-goose/goose.git D:\ref\goose
git clone --depth 1 https://github.com/0xPlaygrounds/rig.git D:\ref\rig
git clone --depth 1 https://github.com/bosun-ai/swiftide.git D:\ref\swiftide
```

Use them to compare design, not as dependencies.

---

## 5. Bootstrap the Rust workspace

Phase 0 deliberately creates **only `stark-contract`**. Providers, core, tools,
policy, store, agents, groups, memory, server, and CLI are added in Phase 1+
when they have code. Empty crates add compile time and hide dependency drift.

From the repo root, on `rust-phase-0`:

```bash
mkdir -p backend-rs/crates/stark-contract/src backend-rs/specs backend-rs/tests/parity
```

`backend-rs/Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = ["crates/stark-contract"]

[workspace.package]
edition = "2024"
license = "Apache-2.0"
version = "0.1.0"
rust-version = "1.99"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
# schemars, ts-rs, uuid, chrono are added when the generated types need them.
```

`rust-toolchain.toml`:

```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy", "rust-analyzer"]
profile = "minimal"
```

Do not pin a target here: local GNU and CI MSVC must both work.

`.gitignore` additions:

```gitignore
# Rust
backend-rs/target/
target/
**/*.rs.bk

# Keep the generated contract despite the global specs/ ignore
!backend-rs/specs/
!backend-rs/specs/**
```

Commit `Cargo.lock` (it is a binary workspace).

---

## 6. Tool spec export

`scripts/export_tool_specs.py` loads `tools/_schemas.py` by file path and
writes the frozen 42-tool/8-scope contract. It needs the project venv
(`requirements-test.txt` in CI) because the registry imports `llm`:

```bash
venv/Scripts/python.exe -B scripts/export_tool_specs.py
# writes backend-rs/specs/tool-specs.json
venv/Scripts/python.exe -B scripts/export_tool_specs.py --check
# exits non-zero if the checked-in copy differs
```

Commit `backend-rs/specs/tool-specs.json`. CI installs `requirements-test.txt`
and runs the same `--check`.

---

## 7. Daily commands

```bash
# From backend-rs/
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
# cargo run -p stark-server   # Phase 1; not present in Phase 0

# Watch mode
cargo watch -x "test --workspace"

# Single crate
cargo test -p stark-groups
cargo run -p stark-server -- --port 8124
```

---

## 8. Environment

| Variable | Purpose |
|---|---|
| `STARK_PORT` | Server port (default 8123; Rust dev 8124) |
| `VITE_API_BASE` | Frontend → backend base URL |
| `RUST_LOG` | `tracing` filter (e.g. `stark=debug`) |
| `HARNESS_ROOT` | Harness root for `.deep_agents` |
| `DEEPSEEK_API_KEY` / provider keys | Provider auth |
| `PAI_TOOL_RETRIES` / `PAI_OUTPUT_RETRIES` | Loop retry budgets |
| `CARGO_TARGET_DIR` | Optional build output dir |

Keep secrets in `.env` (ignored). Never log keys.

---

## 9. Troubleshooting

| Symptom | Fix |
|---|---|
| `link.exe not found` | Install VS Build Tools C++ workload; `rustup default stable-msvc` |
| `cargo` not found | Restart shell; check `%USERPROFILE%\.cargo\bin` on PATH |
| Long build times | Exclude `target/` in Defender; set `CARGO_TARGET_DIR` on a fast disk |
| Path too long | `git config --system core.longpaths true`; shorten paths |
| Permission denied on port | Another Stark instance is running; stop it or change `STARK_PORT` |
| SQLite locked | Ensure WAL + `busy_timeout`; one group writer |
| CRLF patch mismatch | Normalize line endings in the patch matcher |
| rust-analyzer slow | Exclude `frontend/node_modules`, `venv*`, `.deep_agents` |

---

## 10. First-day checklist

- [x] Rust stable installed and verified (`1.99.1` GNU on D:)
- [x] `hello-rust` runs
- [ ] Codex cloned to `D:\ref\codex` (reference read; next)
- [x] Python tests run (328 passed)
- [x] Current work committed on `development`
- [x] `development` pushed
- [x] `master` merged/pushed
- [x] `pre-rust-rewrite` tag pushed
- [x] `rust-rewrite` branch created and pushed
- [x] `rust-phase-0` branch created and pushed
- [x] `backend-rs` workspace created (`stark-contract` only)
- [x] `tool-specs.json` exported (`42` tools, `8` scopes)
- [x] `cargo test --workspace` passes (2 tests)
