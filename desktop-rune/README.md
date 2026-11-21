# Stark — Rune UI

Stark's native interface, built with [Rune](https://github.com/anomalyco/rune).
The React frontend in `../frontend` is the visual and interaction reference;
it is not a runtime dependency.

This README is context only. It explains the project, the Rune concepts the
port relies on, and the architecture. The code is the source of truth.

## Rune

Pinned revision:

```
a7f276c3630f85a06a0230295da4d2bd172d8d3b
```

The dependency name is `rune` (package `rune`, crate `rune`), with `rune-kit`
used only for its syntax/diff scanners. The local Cargo checkout is the
authority for behavior; do not infer API shape from this document.

Rune builds a native element tree, lays it out, paints it, and owns motion,
focus, text shaping, scrolling and accessibility. A Stark view is a function
`&State -> El`; handlers mutate the state, and the next commit rebuilds the
tree. Layout, paint, animation and input stay inside Rune.

## Repository map

- `crates/stark-rune-app` — the native application: shell, canvas,
  transcript, composer, group projects, settings, cloud, agents and fixtures.
- `crates/stark-kit` — pure, UI-free models: markdown parsing, transcript row
  math, edge paths and shared data types.
- `crates/stark-theme` — Stark tokens and shared theme values.
- `crates/stark-rune-web` — wasm entry point; it mounts the same app.
- `parity/` — capture fixtures and scripts used to compare against React.
  There is intentionally no parity write-up or historical log here.

## Rune capability map

These are the behavior primitives that replace CSS assumptions. Prefer them
over approximating a CSS property.

- **Paint layers** — `El::box_shadow` appends to a shadow list; outer and
  inset layers all render. `El::border_gradient`, `linear_gradient` and
  `radial_gradient` cover layered surfaces without stacking fake elements.
- **Overlays** — `anchored`, `anchored_fit`, `anchored_round` place
  out-of-flow content beside an earlier keyed anchor and flip/fit at the
  window edge. Attach at the root to escape clipping ancestors.
- **Menus and focus** — a button-opened menu can be `modal(true, close)`,
  which traps focus and restores the opener when removed. Use `focus_group`,
  `focus_selected`, `focus_return`, `focus_highlight` and `default_button`.
- **Virtual rows** — `virtual_rows` builds only mounted rows;
  `virtual_rows_version` skips key calls when the key sequence is unchanged;
  `on_visible_rows` reports the visible index range once. `memo` inside a
  `virtual_rows` row is dropped when the row leaves the mounted window (rows
  built eagerly with `virtualize` keep their cached subtree).
- **Text and selection** — `selection_group` joins selectable blocks into one
  selection. Put it on the **virtual list itself** (not each row) and Rune
  pins both endpoints, rebuilds unmounted rows on demand, and copies across
  them (up to 10,000 rows / 4 MiB). `selection_copy` can return source text;
  `help` and `help_when_truncated` draw native tooltips; `live_region`
  announces status changes to assistive technology.
- **Scrolling** — `stick_bottom`, `scroll_anchor`, `scroll_margin`,
  `scroll_edge_fade`, `Tx::scroll_to_bottom` and `Tx::scroll_into_view`.
  Virtual rows already preserve the first visible row.
- **Clipping and blur** — `clip_rounded`, `clip_fade_linear`,
  `cutout_circle`; `backdrop_blur(sigma)` blurs Rune's own content in-window,
  matching CSS `backdrop-filter`.
- **Growth and caching** — growth stacks and `fold_glide` animate disclosure
  rows; `cache` rasterizes a settled subtree until its draw output changes.
- **Native dialogs** — `Tx::open_file`, `Tx::save_file` and `Tx::open_folder`
  take `FileDialogOptions`; headless runs script the answers instead of
  showing a picker. The browser build uses a real `<input type="file">` and
  the same multipart upload endpoint.
- **Compositing** — `App::srgb_blend` is on by default, matching CSS alpha
  compositing over dark surfaces. `El::focus_within_flat` drops an element's
  shadows and hover fill while focus is inside it.
- **Browser shell** — the wasm transport resolves the API origin from
  `__STARK_API_ORIGIN__` or the page origin, carries the pairing token on
  every request, and gates the app on a 401. The phone layout hides the rail,
  adds the bottom navigation with safe-area padding, and turns the side panel
  into a backdrop-backed drawer.

### Known boundaries

- No exact `mask-composite` equivalent. The running canvas-card sweep is a
  clipped sheen, not a true excluded border mask.
- No audio element; audio attachments cannot play inside Rune.
- Focus-within can add a ring or flatten shadows, but cannot swap arbitrary
  shadow stacks.
- Auto-update is an honest stub: there is no signed Rune release feed or
  in-app installer yet. The portable package ships a versioned zip.
- Rune's native `role("button")` responds to Space; Return goes to links,
  highlighted rows, list rows and `default_button`. This port adds Enter to
  buttons with an `on_key` helper (`keys::EnterPress`) for web parity; keep
  using that helper instead of a bare `on_press`.
- `memo` inside `virtual_rows` caches only built rows; it is not an offscreen
  cache (Rune's virtual selection rebuilds rows it needs by key).

## Architecture

The shell is one page: a session tab strip, an icon rail, a side panel, a
canvas map and a transcript/chat surface. The canvas and transcript are
always mounted; panels switch content rather than routes. Overlays mount at
the shell or modal root so they escape the clipping of the surface that
opened them.

Canvas cards are small chat surfaces, not previews: each card dock owns its
own attachment draft and send path, pending questions replace the dock with
the compact Ask panel, and the latest bubbles carry copy and two-step rewind
actions. Background cards poll Ask alongside the open transcript, so a
question raised on the map is visible without opening the card.

State is plain Rust. `TranscriptHost`, `CanvasHost`, `ProjectsHost`,
`SettingsHost` and related traits let shared views resolve the active thread
or panel without borrowing the whole app. Handlers capture stable IDs or
keys, never row indices, because virtual rows and memo reuse can move a row.

React CSS values remain the reference for colors, spacing, radii, type and
timing. Where CSS names a property Rune does not expose directly, look for a
behavior primitive in Rune's source first. When the behavior genuinely does
not exist, keep the closest honest implementation and record the boundary
here rather than approximating silently.
