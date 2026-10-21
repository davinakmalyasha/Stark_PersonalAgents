# 12 — Setup & Branching

Everything needed before the first Rust commit: toolchain, repo safety,
branch, reference checkout, and workspace bootstrap.

---

## 1. Install Rust (Windows)

### 1.1 MSVC build tools (required)

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

Verified on the current checkout:

| Fact | Value |
|---|---|
| Current branch | `development` |
| Tracking | `origin/development`, **1 commit ahead** |
| vs `master` | **46 ahead, 1 behind** |
| Dirty entries | **101** (71 modified, 9 deleted, 21 untracked) |
| Default branch | `master` (no `main`) |
| `.env` | ignored, untracked (safe) |
| Latest commit | `408c075 feat(group): stop retries from repeating gated external writes (AU-05)` |

The working tree contains a coherent feature set: canvas, notes, schedules,
agent wakes, chat continuity, and related UI. It should be snapshotted before
the rewrite.

**Important:** there is no `main` branch. Treat **`master` as main** unless you
decide to rename it.

---

## 3. Safety sequence (run after approving the docs)

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

From the repo root, on `rust-rewrite`:

```bash
mkdir -p backend-rs/crates
cd backend-rs
cargo new --lib crates/stark-contract
cargo new --lib crates/stark-providers
cargo new --lib crates/stark-core
cargo new --lib crates/stark-tools
cargo new --lib crates/stark-policy
cargo new --lib crates/stark-store
cargo new --lib crates/stark-agents
cargo new --lib crates/stark-groups
cargo new --lib crates/stark-memory
cargo new --bin crates/stark-server
cargo new --bin crates/stark-cli
mkdir -p specs tests/parity
```

Minimum `backend-rs/Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = ["crates/*"]

[workspace.package]
edition = "2024"
license = "Apache-2.0"
version = "0.1.0"

[workspace.dependencies]
# See docs/rust-rewrite/03-tech-stack.md
```

`rust-toolchain.toml`:

```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy", "rust-analyzer"]
targets = ["x86_64-pc-windows-msvc"]
profile = "minimal"
```

`.gitignore` additions:

```gitignore
# Rust
backend-rs/target/
target/
**/*.rs.bk
```

Commit `Cargo.lock` (it is a binary workspace).

---

## 6. Tool spec export

Add `scripts/export_tool_specs.py` (Python, read-only import of
`tools._schemas`):

```bash
python scripts/export_tool_specs.py
# writes backend-rs/specs/tool-specs.json
python scripts/export_tool_specs.py --check
# exits non-zero if the embedded/checked copy differs
```

Commit `backend-rs/specs/tool-specs.json`.

---

## 7. Daily commands

```bash
# From backend-rs/
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p stark-server

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

- [ ] Rust stable installed and verified
- [ ] `hello-rust` runs
- [ ] Codex cloned to `D:\ref\codex`
- [ ] Python tests run (note failures, if any)
- [ ] Current work committed on `development`
- [ ] `development` pushed
- [ ] `master` merged/pushed (or branch created from `development` if red)
- [ ] `pre-rust-rewrite` tag pushed
- [ ] `rust-rewrite` branch created and pushed
- [ ] `backend-rs` workspace created
- [ ] `tool-specs.json` exported and committed
- [ ] `cargo test` passes
