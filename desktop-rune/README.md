# Stark on Rune — byte-identical rewrite (`rune-rewrite` branch)

`main` stays on Tauri+React (`frontend/` frozen). All Rune work lives in
`desktop-rune/` on branch `rune-rewrite` until headless goldens prove parity.

## What Rune brings (and why it differs)

- **One `wgpu` backend** for macOS/Windows/Linux/web. GPUI splits
  Metal/DX11/Vulkan; Electron/Tauri ship a browser. Rune draws everything
  itself with an instanced uber-quad SDF pass + glyph atlas + Slug +
  `vello_gpu` strips.
- **Layout/Text that fit agents:** `Taffy` (flex/grid/block) on its own arena
  + sum-tree height index for huge transcripts; `Parley` + `HarfRust` +
  `skrifa` + `fontique`; Zed `sum_tree` + rope for editing with native
  `NSTextInputClient` / `UITextInput` protocols.
- **State:** keyed retained tree rebuilt from plain `fn view(s:&State)->El`;
  typed models; reads subscribe; handlers queued `&mut`. No GPUI entities,
  no signals, no `RefCell` re-entry.
- **Motion:** render thread owns the presented tree; every property is an
  analytic spring (duration + bounce, SwiftUI formula) with velocity retarget.
  A panel slide computes its destination once — no per-frame view rebuild.
- **Theming:** unbranded core, token-driven (`Theme::from_json`). OpenCode's
  theme is the first consumer. `stark-theme` here is Stark's theme crate:
  exact `#0a0a0a` / hairlines / `468x420` cards / Inter / mono tokens.
- **Shells:** `winit` now, thin per-OS shells next (borrowed from Zed's
  Apache-2.0 `gpui_*`). Native behaviour (IME, a11y, clipboard, scroll
  momentum), one look everywhere.
- **Agent-first headless:** every app runs headless — render to PNG, dump the
  element tree, drive with scripted input. `AccessKit` tree IS the inspector
  tree. Gallery + screenshot tests per component.

## What headless gives Stark

- `stark-rune --headless --dump-tree tree.txt --golden golden.svg [--script …]`
- Element-tree dump = agent assertion surface (card count, roles, live status).
- Scripted input (`type:`, `mode:`, `toggle:`) = deterministic replay.
- SVG golden = GPU-free CI parity check (real Rune PNG replaces it 1:1).
- Live mode (against `127.0.0.1:8124`, read-only GETs) lists real
  projects/sessions and follows the event stream; goldens stay synthetic.

## Layout

- `crates/stark-theme` — byte-identical tokens from `index.css`/`house.css`/`canvas.css`.
- `crates/stark-kit` — pure canvas math ported from `CanvasStage.tsx`/`canvasTypes.ts`.
- `crates/stark-rune-app` — `El` tree (mirrors `rune::El`), `view()`, scripted input, goldens.

## Real Rune dep (needs network)

```toml
rune = { git = "https://github.com/anomalyco/rune", rev = "<dev rev>" }
```

Pin a `dev` rev (`dev` never rewritten). Until then, `El`/`view()` here are
structurally identical so the swap is mechanical.

## Commands

```bash
cargo test -p stark-theme -p stark-kit -p stark-rune-app
cargo run -p stark-rune-app -- --headless --dump-tree target/rune-goldens/tree.txt --golden target/rune-goldens/stark.svg
```
