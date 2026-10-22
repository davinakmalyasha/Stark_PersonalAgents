# backend-rs

Codex-style Rust backend for Stark. Phase 0 contains the frozen tool contract
and nothing else; providers, loop, tools, store, and server arrive in Phase 1.

## Toolchain

Local development on this machine uses the GNU target on D: because C: is the
small OS volume:

```text
CARGO_HOME=D:\cargo
RUSTUP_HOME=D:\rustup
CARGO_TARGET_DIR=D:\cargo-target\stark
```

The compiler is `rustc 1.99.1` (`x86_64-pc-windows-gnu`) with WinLibs GCC in
`D:\toolchains\winlibs\mingw64\bin`. CI uses the MSVC target on `windows-latest`
to keep the code portable across both Windows toolchains.

## Commands

Run from `backend-rs/`:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Regenerate and verify the frozen contract from the repo root:

```bash
venv/Scripts/python.exe -B scripts/export_tool_specs.py
venv/Scripts/python.exe -B scripts/export_tool_specs.py --check
```

`specs/tool-specs.json` is generated from `tools/_schemas.py`. Do not edit it by
hand. Phase 0 freezes the 42 registered tools and the eight agent scopes.
