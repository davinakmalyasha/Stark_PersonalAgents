# Stark

Stark is a Rust agent harness with a Rune desktop and browser interface.

- `backend-rs/` — the Rust server: agent loop, providers, tools, MCP, storage,
  policy, sandbox, and voice.
- `desktop-rune/` — the Rune native and wasm UI, launcher, and packaging.
- `docs/architecture.md` — the current component and architecture reference.

The server is API-only; it does not serve a browser application. The Rune wasm
build is hosted separately. See `docs/architecture.md` for the run contract,
component boundaries, and build/run details.
