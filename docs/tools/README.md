# Tool docs

The model-facing contract is `backend-rs/specs/tool-specs.json`. It is
validated by `stark-contract` and is the single source of truth for names,
descriptions, parameters, required arguments, and scope ACLs.

- Developer v2 surface: [18 — DeepSeek / Codex Parity](../rust-rewrite/18-deepseek-parity.md)
- Tool system: [04 — Tool System](../rust-rewrite/04-tool-system.md)
- Contract notes: `backend-rs/specs/README.md`

The previous per-tool Markdown pages were generated from the removed Python
tree and had drifted from the Rust contract. They were deleted rather than
kept as a second, stale source of truth.
