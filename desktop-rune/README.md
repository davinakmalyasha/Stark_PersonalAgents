# Stark — Rune UI

Stark's native and browser interface, built with
[Rune](https://github.com/anomalyco/rune). This is the only UI.

This README is context only. It explains the project, the Rune concepts the
port relies on, and the architecture. The code is the source of truth.

## Rune

Pinned upstream revision:

```
a7f276c3630f85a06a0230295da4d2bd172d8d3b
```

The app depends on a vendored fork under `vendor/rune` whose only behavioural
change is the `El::border_sweep` paint primitive (the running-card gradient
ring). It tracks that upstream revision; update the fork deliberately, not as
a side effect of a Cargo refresh. `rune-kit` supplies the syntax scanners.
The code is the authority for behavior; do not infer API shape from this
document.

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
- `crates/stark-launcher` — stable supervisor: versioned installs, server
  lifecycle, pending-update application and rollback.
- `vendor/rune` — pinned Rune fork, excluded from the app workspace.
- `packaging/` — portable/versioned tree, updater payload and NSIS bootstrap.

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
- **Border sweep** — `El::border_sweep` samples the CSS linear gradient per
  fragment and masks it with Rune's border-ring coverage. It is a paint-only
  loop: no child overlay, no layout change, no per-frame app rebuild.
- **Audio** — `audio::AudioEngine` owns playback. Native uses rodio/cpal with
  Symphonia; web uses a hidden `HtmlAudioElement` fed by an authenticated
  Blob URL. The engine lives in the thread, so virtual rows can unmount
  without stopping a clip.
- **Window chrome** — native uses Rune's custom `Titlebar`: one 44pt band with
  the navigation toggle, chat search, session tabs, compact `+` and Rune's
  Windows caption buttons. `Cx::chrome_insets` reserves the caption area;
  the band drags the window and keeps Aero Snap, snap layouts and the window
  menu. The browser renders the same band without caption buttons; Linux keeps
  its native frame.
- **Updates** — `updater.rs` follows the master feed. It checks on launch and
  every five minutes, downloads and verifies the signed payload, and stages it
  under `app/<version>`; `stark-launcher` switches `current.json` after the UI
  exits and rolls back on a failed launch. Every push to master publishes a
  new signed version, so no tag or manual check is involved.
- **Browser shell** — the wasm transport resolves the API origin from
  `__STARK_API_ORIGIN__` or the page origin, carries the pairing token on
  every request, and gates the app on a 401. The phone layout hides the rail,
  adds the bottom navigation with safe-area padding, and turns the side panel
  into a backdrop-backed drawer.

### Known boundaries

- The public Rune installer is Windows x86_64 today. The versioned layout and
  feed carry a platform key so other targets can be added without changing the
  app protocol.
- First-run SmartScreen trust needs Authenticode signing; minisign protects
  update integrity even when no Authenticode certificate is configured.
- Web updates are server deployments; the browser build reports that instead
  of checking the desktop feed.
- Updates are full payloads, not deltas. Models live in a shared `models/`
  directory so they are not duplicated per version.
- Focus-within can add a ring or flatten shadows, but cannot swap arbitrary
  shadow stacks.
- Rune's native `role("button")` responds to Space; Return goes to links,
  highlighted rows, list rows and `default_button`. This port adds Enter to
  buttons with an `on_key` helper (`keys::EnterPress`) for web parity; keep
  using that helper instead of a bare `on_press`.
- `memo` inside `virtual_rows` caches only built rows; it is not an offscreen
  cache (Rune's virtual selection rebuilds rows it needs by key).

## Architecture

The shell is one page: an app-drawn titlebar with the open-session tabs, an
icon rail, a side panel, a canvas map and a transcript/chat surface. The
canvas and transcript are always mounted; panels switch content rather than
routes. Overlays mount at the shell or modal root so they escape the clipping
of the surface that opened them.

Canvas cards are small chat surfaces, not previews: each card dock owns its
own attachment draft and send path, pending questions replace the dock with
the compact Ask panel, and the latest bubbles carry copy and two-step rewind
actions. Background cards poll Ask alongside the open transcript, so a
question raised on the map is visible without opening the card.

Installed builds run through `StarkRune.exe` (the launcher). It owns the
bundled `stark-server`, starts the UI from `app/<version>`, and applies a
staged update only after the UI exits. `current.json` is the atomic switch;
`pending.json` is the handoff; `blocked.json` records a version that failed to
start. A failed new build rolls back to the previous version automatically.

The launcher is a Windows GUI-subsystem binary, so the shortcut does not open a
terminal. The launcher, UI and bundled server embed `stark.ico`; the shortcuts
use it and carry the `com.stark.rune` AppUserModelID, and the Rune UI sets the
same ID plus its live window icon. Launcher or shortcut changes are delivered
by the Setup; the five-minute auto-update payload replaces only
`app/<version>`.
Every push to master publishes a signed build to the dedicated public feed
repo (`davinakmalyasha/Stark_Rune_Releases`), and the installed app checks that
feed on launch and every five minutes. The rolling `rune-master` release on the
main repo carries the matching installer. The VPS is deployed from the same
commit by `build-linux`/`deploy-vps`, and its `/api/health` reports the
revision in `build`.

Rune keeps its data in `%LOCALAPPDATA%\StarkRune\data`
(override with `STARK_HARNESS_ROOT`); use Settings' backup import to adopt an
existing `D:\StarkData` tree instead of running two servers on one database.
The launcher refuses to reuse a backend on port 8124 unless it reports the
same app version, so a development server cannot silently capture the
installed app.

Development uses `scripts/dev-rune.ps1` (latest master against the repo harness
root) and `scripts/check-rune.ps1` (format, clippy, tests). Packaging and feed
details live in `packaging/RELEASE.md`.

State is plain Rust. `TranscriptHost`, `CanvasHost`, `ProjectsHost`,
`SettingsHost` and related traits let shared views resolve the active thread
or panel without borrowing the whole app. Handlers capture stable IDs or
keys, never row indices, because virtual rows and memo reuse can move a row.

`stark-theme` and the Rune layout primitives are the source of truth for
colors, spacing, radii, type and timing. Where a CSS-like name describes
behavior Rune does not expose directly, look for a behavior primitive in
Rune's source first. When the behavior genuinely does not exist, keep the
closest honest implementation and record the boundary here rather than
approximating silently.
