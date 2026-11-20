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

## Real Rune dep (pinned, offline-optional)

```toml
[dependencies.rune]
git = "https://github.com/anomalyco/rune"
rev = "a7f276c3630f85a06a0230295da4d2bd172d8d3b" # `dev`, never rewritten
package = "rune"
default-features = false  # headless + core rendering, no OS integrations
optional = true
```

```bash
cargo test -p stark-kit -p stark-theme                              # offline, GPU-free
cargo test -p stark-rune-app --features rune-real                    # real Rune Runtime, GPU-free
cargo run -p stark-rune-app --features rune-real -- \
  --rune-snap target/rune-goldens/transcript.png --rune-scene screenshot  # real wgpu PNG (needs GPU)
```

## Motion foundation

Static parity was the first pass; interaction motion is now ported at the
shell level. `crates/stark-rune-app/src/motion.rs` carries both families Rune
uses: hover `Curve`s (`150 ms cubic-bezier(.4,0,.2,1)` by default, 140/200/280
ms Stark eases) and presence `Spring`s matched to the final CSS cascade
(user message 200 ms, agent 240 ms, generic fade 140 ms, modal 240 ms, menu
160 ms, trace 340 ms). The shell and transcript roots set
`hover_motion_within`, so every existing `hover`/`group_color`/
`group_opacity` fades instead of snapping; rail icons, session tabs, the close
button, New session, send/stop press and the Settings sheet use them.

Message entrances match `MessageBubble.tsx`: a user row slides in from +14 pt
over 200 ms, an agent row rises 8 pt over 240 ms, and system or other
non-agent rows fade with a 2 pt rise over 140 ms. The virtual transcript records each row key in
`seen_rows` when it first becomes visible, so scrolling a row out and back —
or refreshing history — never replays its entrance. Rows leave in the frame
they unmount, like React; the `modal_enter`/`menu_enter` primitives carry the
same instant-removal spring pair for the surfaces that follow.

Trace shells (340 ms), trace groups (280 ms), tool panels (180 ms), retry
notices (200 ms) and delegation cards (300 ms) enter on their CSS clocks.
Disclosure chevrons rotate 180 ms instead of swapping glyphs. Menus and
popovers (`160`/`180` ms), the canvas add menu (`160` ms with its `.985`
settle), attachment cards (`260` ms, `8 pt`, `.96`), AgentConfig/TokenStats
modals (200 ms backdrop, 240 ms panel) and the canvas card hover border all
use the same primitives. Ask options keep their border box through a wrapper
so the hover border is not inset by their padding; markdown code-copy and
inline links brighten on hover. Spinners, pulse dots and sheens keep their
loop specs. Hover and spatial transitions respect Reduce
Motion through Rune's motion gate (offsets snap, opacity still fades).

The remaining deltas are recorded in `parity/README.md`: 1 pt hover lifts,
the 1.08 project-chevron scale, the sidebar delete slot (opacity fades, the
slot does not collapse), inline-image 1.04 zoom, and the opacity timing of
open/close bodies that CSS animates with `grid-template-rows`.

## Transcript (MessageStream port)

- Pure math: `crates/stark-kit/src/transcript.rs` — role-based height
  estimates, overscanned `realize()` window (`f64` extents, bottom-exact),
  `stick_bottom` follow-the-end. 5 tests, incl. 10k-row subset proof.
- Real view: `crates/stark-rune-app/src/rune_transcript.rs` — scroll viewport
  (`.scroll(stick_bottom)`, bounded `.h_full().min_h(0.0)`) + growth stack
  (`.growth().virtualize(EST)`) with keyed bubble rows, title, live status,
  composer. `spec()` ships `screenshot` + `stress` scenes with a debug
  parity assert tying `realize()` to the shipped scene.

## Commands

```bash
cargo test -p stark-theme -p stark-kit -p stark-rune-app
cargo run -p stark-rune-app -- --headless --dump-tree target/rune-goldens/tree.txt --golden target/rune-goldens/stark.svg

# Settings: Tauri `SettingsModal.tsx` port (global sheet, rail gear)
cargo run -p stark-rune-app --features rune-real -- \n  --rune-shell-snap target/rune-goldens/settings.png --rune-route settings \n  --size 1280x900 --at 2
```
