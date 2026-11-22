# Vendored Rune

Source: `https://github.com/anomalyco/rune`
Revision: `a7f276c3630f85a06a0230295da4d2bd172d8d3b`

This is a minimal workspace copy of the crates the Stark app builds against.
It exists for one reason: the running-card border sweep needs a fragment-level
linear gradient masked to the border ring, which upstream does not expose yet.

Local change:

- `El::border_sweep(BorderSweep)` (`el/visuals.rs`, `el/visual.rs`, `el.rs`)
  paints a periodic 1 pt gradient ring over the base border.
- `KIND_BORDER_SWEEP` (`render.rs`, `runtime/paint/build.rs`, `shader.wgsl`)
  evaluates two gradient segments per fragment with `border_over` coverage.
- `LoopKind::BorderSweep` / `LoopSpec::border_sweep` (`rune-motion`) give the
  renderer a paint-only 0..1 phase without waking the app.
- `runtime/commit/memo_reuse.rs` includes the new cold field in reuse equality.

Everything else tracks the upstream revision. Re-vendor deliberately and
re-run the canvas parity scenes before updating the revision in
`crates/stark-rune-app/Cargo.toml`.
