# Stark parity rig

Deterministic React-vs-Rune captures at the same size, against a **frozen**
API snapshot. Live data drifting between the two captures was the single
biggest source of fake diffs; this removes it.

## One-time / per-scene recording

```bash
python desktop-rune/parity/record_fixtures.py --base http://127.0.0.1:8124
```

Writes `parity/fixtures/*.json` (git-ignored: they contain chat text) for
`/api/ui/state`, workspaces, each workspace's chat list, each open tab's chat,
provider/models, running state and pending questions.

## Run the replay

```bash
# 1. fixture server
python desktop-rune/parity/mock_server.py --port 8125

# 2. React against the same fixtures (separate Vite, same repo)
cd frontend && STARK_PORT=8125 npm run dev -- --port 5200 --strictPort

# 3. native app against the same fixtures (release; the capture script
#    prefers target/release, then target/debug)
cd desktop-rune && cargo build --release -p stark-rune-app --features rune-real
```

`capture_native.sh` uses `D:/cargo-target/stark/{release,debug}/stark-rune.exe`
(this repo sets `CARGO_TARGET_DIR`); override with `STARK_RUNE_BIN`.

Rune's scripted `wheel` sign is inverted from Chrome's: `dy: 420` scrolls the
transcript **up**; negative values scroll down.

## Capture a state

```bash
# native: 800x573, scripted input, 4s virtual clock
STARK_API_PORT=8125 bash desktop-rune/parity/capture_native.sh \
  /tmp/native.png 800x573 open_hello.json 4

# React: 800x573, 1x, scrollbars hidden, transcript scrolled to its end
# (both sides default STARK_PARITY_NOW to 2026-11-12T12:00:00Z)
python desktop-rune/parity/capture_react.py /tmp/react.png \
  --url http://localhost:5200/ --settle 4

# compare
python desktop-rune/parity/compare.py \
  --native /tmp/native.png --react /tmp/react.png --align --out /tmp/diff
```

`compare.py` prints full/right/band means, writes `heatmap.png` and `mask.png`,
and with `--align` reports the best ±6 px offset (a layout shift). `--blur 1`
compares softened images so font anti-aliasing stops dominating.

## Baseline (2026-11-11, 800×573, "hello" chat)

| capture | full mean | right mean | align |
|---|---|---|---|
| 800×573 boot | **2.103** | **2.732** | (0, 0) |
| 1280×900 boot | **2.064** | **2.254** | (0, 0) |

The renderer embeds the exact `InterVariable.ttf` the browser loads (the
built-in static Inter shifted every text box), matches React's Tailwind
letter-spacing, and draws the send/stop icons as the same Lucide geometry.
Those plus the live camera took the baseline from 2.306 to 2.103. At
1280×900 the fitted canvas cards are visible, so the mini chat cards are
held to the same standard: flex-1 conversation, `CanvasChatBubble` sizes,
and the bordered paperclip/input/25pt-send dock.

Release native renders pixel-identical to debug (mean 0.0) at ~4× the
speed: animating p50 4.8 ms, forced repaint p50 2.5 ms, zero draws at rest.
Canvas pan/zoom publishes through Rune `Live` values (world offset/scale,
grid offset) and says `tx.live_only()`, so panning or zooming never rebuilds
the view or relayouts text. The release wasm is 12.06 MB with the embedded
face (debug was 76.9 MB).

`STARK_FONT_SMOOTHING=off|on|directwrite` is a rig-only knob for the shell
snapshot. DirectWrite is the shipped default and measures best (2.117 vs
2.28 Off / 2.33 On against the same React capture).

Relative-time labels (`20d`, `Today`) are computed from the wall clock at
capture time. Both capture paths pin that clock: `capture_native.sh` exports
`STARK_PARITY_NOW` (read once by the app; Unix seconds, Unix milliseconds, or
RFC 3339) and `capture_react.py` installs the matching `Date` shim before the
app loads and then reloads the page. Both default to
`2026-11-12T12:00:00Z`. Export `STARK_PARITY_NOW=` (empty) to compare live
behaviour instead; the app itself keeps the real clock when it is unset.

`boot_home.json` is deliberately empty: the fixture `/api/ui/state` already
opens the "hello" chat, so a capture must not tap the sidebar (a tap calls
`focusCard`, changes the camera to 0.65×, and hides the fitted map). The
React capture launches Chrome with `--hide-scrollbars` so the initial
`fitView` sees the same stage width as the native window; injecting that CSS
after load leaves the map 5pt off-centre.

The remaining difference is glyph rasterisation, not geometry. Chrome draws
subpixel (LCD) antialiasing on opaque surfaces such as the prompt dock
(visible RGB fringes there), while Rune's pinned renderer draws grayscale;
those pixels can never match exactly. Any `align` other than `(0, 0)` is a
real layout shift and should be fixed first.

## Markdown showcase parity (2026-11-13)

Synthetic, committed fixtures under `parity/scenarios/md/` cover the three
tall markdown blocks. Start the fixture server with the overlay and one active
chat:

```bash
python desktop-rune/parity/mock_server.py --port 8125 --scenario md \
  --active-chat chat-md-code   # then chat-md-table / chat-md-diff
```

Native boots with the transcript stuck to the bottom, so every state starts
with a wheel-up. The wheel must land in the transcript's **left gutter**, not
inside a block: code wells and especially the 420pt diff well consume vertical
wheels and leave the transcript at the bottom. At 800×573 the gutter point is
`(330, 300)`; at 1280×900 it is `(450, 500)`. Scripts:

| state | 800×573 | 1280×900 |
|---|---|---|
| top | `md_*_top.json` (code/table) or `md800_top_left.json` (diff) | `md1280_top.json` |
| mid | `md_code.json` / `md_table.json` at 210/225, `md800_210_left.json` (diff) | `md1280_210.json` |
| end | — | `md1280_189.json` (clamps to native max) |

React's scroller clamps `scrollTop` to its own max (copy and table: 189 and
14 at 1280×900; the diff transcript is taller). Native content ends at the
same heights, but its max scroll can be a fraction higher (189.34 vs 189), so
the end captures sit within a third of a pixel. The markdown response ends
after its **last block's bottom margin** (`mb-12`/`mb-20`); the token/time
footer is rendered only when `metadata.usage` exists and only while the
message is not streaming, exactly as `MessageBubble.tsx` does.

| capture | full mean | right mean | align |
|---|---|---|---|
| 800×573 code top | **2.144** | **3.172** | (0, 0) |
| 800×573 code @210 | **2.376** | **3.567** | (0, 0) |
| 800×573 table top | **1.863** | **2.629** | (0, 0) |
| 800×573 table @225 | **2.502** | **3.631** | (0, 0) |
| 800×573 diff top | **2.318** | **3.478** | (0, 0) |
| 800×573 diff @210 | **2.684** | **4.143** | (0, 0) |
| 1280×900 code top | **2.032** | **2.444** | (0, 0) |
| 1280×900 code end | **2.060** | **2.409** | (0, 0) |
| 1280×900 table top | **1.873** | **2.184** | (0, 0) |
| 1280×900 table end | **1.876** | **2.192** | (0, 0) |
| 1280×900 diff top | **1.885** | **2.359** | (0, 0) |
| 1280×900 diff @210 | **1.887** | **2.254** | (0, 0) |

Re-verified 2026-11-18 on the release binary with the bordered inline
code (`chat-md-table` carries one): table top/end reproduce the recorded
right means exactly (2.184/2.192), as do code end and diff @210
(2.409/2.254); all align (0, 0). The remaining top-pose deltas
(+0.06–0.11 full-frame, background bands only) sit inside session raster
variance: the changed render paths (first-block margin, quotes,
checkboxes, tight lists, inline-code wells) do not execute on fixture
bodies that start with a plain paragraph and contain no lists, quotes,
code spans or checkboxes — except the table's inline span, whose new
border is verified present and pixel-faithful on zoom.

Geometry matches exactly: code card 385.5 vs 385.406, diff wrapper 463.2 vs
463.1875, table wrapper 210.19 vs 210.1875; diff rows 18.59 and header 39.2
match. The diff's remaining delta is Chrome's LCD subpixel antialiasing on
the opaque card (Rune draws grayscale) plus a one-device-pixel baseline
difference, corrected with Rune's non-layout `.offset(0, 1)` on diff glyphs.
Code/table bands are dominated by Consolas-vs-Consolas rasterisation at 12–13.5pt.

## Tasks showcase parity (2026-11-18)

`parity/scenarios/md/` also serves `chat-md-tasks`: h1/h2, a checked +
unchecked task list with a nested follow-up, a blockquote, an inline-styles
paragraph (bold/italic/strike/code/link), h3/h4 and a rule. Same rig
(`--active-chat chat-md-tasks`, `md1280_top.json` wheel-up to top):

| capture | full mean | right mean | align |
|---|---|---|---|
| 1280×900 tasks top | **2.206** | **2.490** | (0, 0) |
| 1280×900 tasks end (h1-anchored) | **2.345** | **2.796** | (0, 0) |

Closing the tasks gap took ten implementation fixes (all follow React's
collapsing-margin rules, verified against CDP rects + pixel probes):

- First-block top margin: React's `mt-*` collapses out of the unpadded
  response, so the first child keeps `mt` (h1 `mt-6` = 24px); the native
  12pt response pad covers the first 12, first blocks keep `max(mt-12, 0)`.
- Blockquotes are fully italic with padded-well margins: full first/last
  margins plus an unbordered bottom spacer (React `p` stays `my-3` inside
  `py-2`, margins never collapse through padding).
- Task checkboxes follow `align-middle` (centered in the 28.675pt line box)
  and render the disabled look React actually paints: gray `#9a9a9b` fill
  with a 2px white check when checked, `#a9a9aa` 1px border when not
  (the `accent-indigo-500` class is overridden by the disabled graying).
- Tight/loose lists: the parser marks items without a `Paragraph` event as
  tight (no React `<p>` wrapper), so nested `ul` keeps its own `my-2.5`
  (10px) instead of collapsing with phantom paragraph margins — nested
  pitch lands on React's exact 67.3px.
- Inline code renders the `border-zinc-700/50` 1px ring on the same rounded
  rect as the zinc-800/80 well, via the framework's paint-only
  `text_highlights` (`TextHighlight` ring + 6px pad for `px-1.5`).
  Vertical chrome is framework-controlled and measured, not inferred:
  React CDP reports the `code` box at 21px tall around 15px text
  (`py-0.5` 2px + 1px border per side); native pixels show the same text
  rows under a 15px well (asc/descent + 1px, no `pad_y` API in
  `decorations.rs`). The 6px delta needs a pinned-Rune fork, so the well
  stays horizontally exact and vertically short by design.
- Quote border side: `border_sides` is CSS-ordered `[top,right,bottom,left]`
  (confirmed in the Rune shader + `set_border` + pixel probes), so the
  `border-l-2` bar is `[0,0,0,2]`; the well uses `rounded-r-lg` (right
  corners only). The old `[2,0,0,0]` painted a *top* border that consumed
  2px of layout room; the side fix is covered by
  `quote_well_mounts_content_at_pad`.
- Non-heading blocks after a heading collapse with `max`, not the heading's
  bottom margin alone: a para after h3 keeps `max(12,8)=12` and the rule
  after h4 keeps `max(24,6)=24`. The old `after_heading` shortcut dropped
  4px + 18px below h3/h4; native tree vs React rects now match within 0.3pt.
- Task items keep react-markdown's source space (`<input> text` renders a
  real space), and list content carries the `li`'s `pl-0.5` (2px, placed
  before the checkbox so boxes move too): without them every task glyph sat
  ~6px left of React's while boxes already matched. Glyph centerlines now
  coincide exactly; the remaining text delta is stroke-coverage raster.
- Disabled checkboxes render as Chrome paints them: checked is gray
  `#9a9a9b` fill + white 2px check behind a `#868687` 1px border, and the
  unchecked box is NOT hollow — it carries the same gray `#a9a9aa` fill
  with the darker border (verified pixel-by-pixel; the hollow rendering
  was the largest single checkbox-col contributor).
- Quote wells use `rounded-r-lg` (right corners only); the bar side stays
  square like React's.
- `voice-stt` follows the backend's MSVC gate at the manifest level:
  `moonshine-rs` is a `target.'cfg(target_env = "msvc")'` optional dep, so
  GNU `--features voice-stt` compiles to the honest meter-only stub instead
  of failing in `bindgen`/MSVC link. On-device transcription still needs an
  MSVC host; models are reused in place from `D:\Stark\resources\models`.
- Dictation prefers the **backend bridge** (`voice_ws.rs`), restoring the
  pre-rewrite split (`7b5810b`: client PCM, server VAD/STT, turn commit):
  captured 16 kHz mono frames stream as `i16` LE over `/api/voice/ws`, and
  the server's `transcript` / `turn_final` events map onto
  `VoiceMsg::Transcript` — the same overlay/live-bubble/draft path with no
  speech model (and no MSVC prebuilt) in the app. Verified against a real
  tungstenite server: handshake, one 160-sample frame = 320 bytes, interim
  then final text. A refused/unreachable server falls back to the on-device
  producer (MSVC builds), then to meter-only with an honest note; a GNU
  server answers `session.error`, which surfaces as a note while the meter
  keeps running. Capture no longer requires a device 16 kHz mode: an exact
  mode is preferred, otherwise the stream is linearly resampled to 16 kHz
  with phase continuity across blocks (this box's mic is 48 kHz-only, as is
  WASAPI shared mode generally), so dictation works on stock Windows audio.
- Real-model verification (2026-11-19, this machine): the MSVC backend was
  built without Visual Studio via `cargo-xwin` + LLVM 23.1.6 (`clang-cl`,
  `lld-link`, `libclang`) and ONNX Runtime 1.26; `stark-server` logged
  `Moonshine STT model: medium` from `D:\Stark\resources\models`. Streaming
  `scratch/pure_test_capture.wav` (16 kHz mono) through
  `voice_ws::{try_open, spawn_with}` — the app's actual bridge — produced
  partials and a `turn_final` transcript, proving client → ws → server
  VAD/STT → `VoiceMsg::Transcript` end to end. No model was downloaded.
- Artifact review is viewport-aware like the lightbox: the card caps at
  `min(860px, vw-48)` / `min(760px, vh-48)` (React's modal caps) with the
  scroll body taking the remainder. Still a centered modal over the
  chats-only transcript — no map/coding workspace takeover.

Remaining tasks delta is glyph-level rasterisation: positions are exact
(best integer shift is (0, 0), background pixels match at 1.8), but the two
grayscale rasterisers paint different coverage curves (same peak 229), so
dense 15.5px text diffs more than sparse tables. Same accepted class as the
code/table/diff deltas, plus the native-only Latest pill at the top pose
(React hides its pill); the end pose is pill-free on both sides.
`align (0, 0)` throughout: no layout shifts remain.

## Activity trace parity (2026-11-14)

`parity/scenarios/trace/` is a committed, synthetic chat whose assistant row
carries `metadata.liveTimeline` (reasoning + one completed tool), `isTrace`,
`duration: "8s"` and a usage chip. It captures the completed "Worked for 8s"
accordion and the answer/trace split without any real transcript text:

```bash
python desktop-rune/parity/mock_server.py --port 8125 --scenario trace \
  --active-chat chat-trace
STARK_RUNE_BIN=D:/cargo-target/stark/debug/stark-rune.exe \
  STARK_API_PORT=8125 bash desktop-rune/parity/capture_native.sh \
  /tmp/trace-native.png 1280x900 "" 4
python desktop-rune/parity/capture_react.py /tmp/trace-react.png \
  --url http://localhost:5200/ --size 1280x900 --settle 4
```

The completed divider is the refined full-width `.trace-complete-header`:
42pt tall, 13pt/500 `#a4a4ad`, a `rgba(255,255,255,.085)` bottom hairline,
and a 9pt collapsed slot (23pt when open). Like React, a timeline whose last
segment is a tool leaves `finalText` empty; the answer is owned by the trace,
not duplicated below it.

`parity/scenarios/ask/` is a synthetic two-question batch (described options,
multi-select, custom-answer row) for the inline `AskQuestionPanel`. The batch
is owned by `chat-ask`; the panel replaces the prompt dock and polls
`/api/ask/pending` every 1.5s:

```bash
python desktop-rune/parity/mock_server.py --port 8125 --scenario ask
STARK_RUNE_BIN=D:/cargo-target/stark/release/stark-rune.exe \
  STARK_API_PORT=8125 bash desktop-rune/parity/capture_native.sh \
  /tmp/ask-native.png 1280x900 "" 2
D:/cargo-target/stark/release/stark-rune.exe \
  --rune-shell-snap /tmp/ask-native-route.png --size 1280x900 \
  --rune-route ask --at 2
python desktop-rune/parity/capture_react.py /tmp/ask-react.png \
  --url http://localhost:5200/ --size 1280x900 --settle 4
```

Native-only demo routes (`--rune-route stream` / `trace` / `ask`) render the
same components without a fixture server for quick visual checks.

## Sidebar parity (2026-11-17)

`parity/scenarios/sidebar/api_ui_state.json` overlays the recorded chat lists
with a pinned chat, one open tab and the `general` filter. Deletion uses the
wall clock, so these captures must run with `--parity-now ""` (React) and no
`STARK_PARITY_NOW` (native); the live clock only shifts the relative stamps.

```bash
# React (Vite on :5200 with STARK_PORT=8126)
python desktop-rune/parity/capture_react.py /tmp/sidebar-react-rest.png \
  --url http://localhost:5200/ --size 1280x900 --settle 4 --parity-now ""
python desktop-rune/parity/capture_react.py /tmp/sidebar-react-hover.png \
  --url http://localhost:5200/ --size 1280x900 --settle 4 --parity-now "" \
  --hover 200,166
python desktop-rune/parity/capture_react.py /tmp/sidebar-react-confirm.png \
  --url http://localhost:5200/ --size 1280x900 --settle 4 --parity-now "" \
  --click 243,166
python desktop-rune/parity/capture_react.py /tmp/sidebar-react-fade.png \
  --url http://localhost:5200/ --size 1280x900 --settle 4 --parity-now "" \
  --click 243,166 --click2 243,166 --click2-delay 0.05
python desktop-rune/parity/capture_react.py /tmp/sidebar-react-gone.png \
  --url http://localhost:5200/ --size 1280x900 --settle 4 --parity-now "" \
  --click 243,166 --click2 243,166 --click2-delay 0.35

# native (release; the delete taps use wall time)
python desktop-rune/parity/mock_server.py --port 8126 --scenario sidebar
STARK_API_PORT=8126 D:/cargo-target/stark/release/stark-rune.exe \
  --rune-shell-snap /tmp/sidebar-native-rest.png --size 1280x900 --at 3.0
STARK_API_PORT=8126 D:/cargo-target/stark/release/stark-rune.exe \
  --rune-shell-snap /tmp/sidebar-native-hover.png --size 1280x900 --at 3.0 \
  --script parity/scripts/sidebar_hover_pin.json
STARK_API_PORT=8126 D:/cargo-target/stark/release/stark-rune.exe \
  --rune-shell-snap /tmp/sidebar-native-confirm.png --size 1280x900 --at 3.0 \
  --script parity/scripts/sidebar_delete_confirm.json
for at in 3.50 3.56 3.62 3.80; do
  STARK_API_PORT=8126 D:/cargo-target/stark/release/stark-rune.exe \
    --rune-shell-snap /tmp/sidebar-native-fade-$at.png --size 1280x900 \
    --at $at --script parity/scripts/sidebar_delete_fade.json
done
```

The delete tap in both scripts lands on the row's trash button at `243,166`
(the hover cluster is `hidden group-hover:flex`, so the pointer settles over
the row first). The hover pose parks on the cloud button at `200,166`
(`parity/scripts/sidebar_hover_pin.json`). At 1280×900 (`full` = whole frame,
`band` = sidebar 60–300 × 130–300):

| pose | full mean | band mean | align |
| --- | ---: | ---: | --- |
| rest | **1.763** | **1.235** | (0, 0) |
| hover (`--hover 200,166`, over the cloud button) | **1.757** | **1.561** | (0, 0) |
| delete confirm (first tap) | **1.758** | **1.608** | (0, 0) |
| fade (~50ms in, `--click2-delay 0.05` vs `--at 3.50`) | **2.656** | **12.849** | (0, 0) |
| after the 220ms removal (`--click2-delay 0.35` vs `--at 5.0`) | **1.747** | **1.539** | (0, 0) |

The mock server answers `DELETE /api/workspaces/*/chats/*` with
`{"ok": true}` like the real server. Without it the native `DELETE` fails,
sends `RefreshNeeded`, and the static fixture list resurrects the just-deleted
row (unpinned) on refetch — the gone pose would show it back in second place.

The fade is React's `house-row-out`: 220ms, opacity 1→0, `translateX(0→-8px)`
and `max-height 40px→0` (padding/margin to 0), so the rows below slide up as
the row fades. The fade band (12.849) is that structural slot collapse, not a
shift — `compare.py --align` still reports (0, 0). Rune applies the same
opacity and offset but keeps the row's slot until its existing 220ms removal
timer. Every attempt to shrink or clip a painted row mid-fade — `max_h`/`h` on
the wrapper or the inner cell, rounded or rectangular, with or without its
hover group — left the fading row's last paint (title and action cluster)
under the next row. The slot collapse is therefore a documented transient
delta; the gone pose above compares the post-removal list and is clean. The
static poses (rest/hover/confirm) match with no ghost at rest, hover or
confirm.

## Coding filter parity (2026-11-18)

`parity/scenarios/coding/` is a committed synthetic workspace set: project
Alpha (10 chats, exists on disk) with a spawn nest (`Orchestrator` →
`worker-a`/`worker-b`), a cross-workspace child (`Remote child` spawned from
`General Plan`), and six filler chats so the group pages; project Beta is
missing on disk (amber name, `No chats yet`). The mock serves it with
`--scenario coding`. The Coding filter renders collapsible per-project groups
(chevron header + uppercase name + count + per-project `+`), 7 rows per page
with Load more, same-workspace nests oldest-first, and the `auto`/`from X`
badge on cross-workspace tops. Non-General rows use the `</>` icon, matching
`renderRow`. The Main filter pages at 15 rows with its own Load more.

```bash
# React (Vite on :5200 with STARK_PORT=8126)
python desktop-rune/parity/capture_react.py /tmp/coding-react-rest.png \
  --url http://localhost:5200/ --size 1280x900 --settle 4 --parity-now ""
python desktop-rune/parity/capture_react.py /tmp/coding-react-expanded.png \
  --url http://localhost:5200/ --size 1280x900 --settle 4 --parity-now "" \
  --click 71,189
python desktop-rune/parity/capture_react.py /tmp/coding-react-collapsed.png \
  --url http://localhost:5200/ --size 1280x900 --settle 4 --parity-now "" \
  --click 69,160
python desktop-rune/parity/capture_react.py /tmp/coding-react-loadmore.png \
  --url http://localhost:5200/ --size 1280x900 --settle 4 --parity-now "" \
  --click 172,490

# native (release; scripts tap the same controls)
python desktop-rune/parity/mock_server.py --port 8126 --scenario coding
STARK_API_PORT=8126 D:/cargo-target/stark/release/stark-rune.exe \
  --rune-shell-snap /tmp/coding-native-rest.png --size 1280x900 --at 3.0
STARK_API_PORT=8126 D:/cargo-target/stark/release/stark-rune.exe \
  --rune-shell-snap /tmp/coding-native-expanded.png --size 1280x900 --at 3.0 \
  --script parity/scripts/coding_expand_nest.json
STARK_API_PORT=8126 D:/cargo-target/stark/release/stark-rune.exe \
  --rune-shell-snap /tmp/coding-native-collapsed.png --size 1280x900 --at 3.0 \
  --script parity/scripts/coding_collapse_group.json
STARK_API_PORT=8126 D:/cargo-target/stark/release/stark-rune.exe \
  --rune-shell-snap /tmp/coding-native-loadmore.png --size 1280x900 --at 3.0 \
  --script parity/scripts/coding_load_more.json
```

At 1280×900 (`band` = sidebar 40–320 × 100–620; the full frame includes the
canvas, where the native map and React's coding IDE view legitimately differ —
workspace canvas work is out of scope, so parity is read on the band):

| pose | full mean | band mean | align |
| --- | ---: | ---: | --- |
| rest | **4.247** | **1.004** | (0, 0) |
| expanded nest | **4.373** | **1.898** | (0, 0) |
| collapsed Alpha | **4.201** | **0.558** | (0, 0) |
| load more pressed | **4.288** | **1.242** | (0, 0) |

Group geometry comes from exact CDP rects: header `227×24` at 0-gap to the
first row, top rows `205×34.75` on a 40.75 pitch, the from-badge block 17.5
tall (9pt pill + `py-px` + border), nests gapless under the parent with 2pt
between leaves. The Main poses are unchanged by the page limit and badges
(rest 1.745/1.235, hover 1.756/1.561, confirm 1.758/1.608, fade band 12.849,
gone 1.747/1.539, all align (0, 0)).

## State scripts

`parity/scripts/*.json` are Rune headless input scripts (`tap`, `wheel`,
`drag`, `type`, …). Add one per visual state so both sides capture the same
scene. The React side may need a matching DOM scroll (`scrollTop`) or a click;
keep those instructions next to the script.

## Settings parity (2026-11-19)

`parity/scenarios/settings/api_migration_status.json` is a committed,
deterministic migration status (one active path, two import candidates) so
both sides render the same rows without touching a real harness. The sheet is
the Tauri `SettingsModal.tsx` port: App updates, Backup & move, One-time
migration, the styled file control and exact status copy. Rune has no updater
yet, so the desktop Check action reports that honestly; the row itself matches
Tauri at rest.

```bash
# fixture server + React (Vite on :5200 with STARK_PORT=8126)
python desktop-rune/parity/mock_server.py --port 8126 --scenario settings
cd frontend && STARK_PORT=8126 npm run dev -- --port 5200 --strictPort

# React desktop pose (rail gear, no launch-update error)
python desktop-rune/parity/capture_react.py /tmp/settings-react.png \
  --url http://localhost:5200/ --size 1280x900 --settle 6 \
  --desktop --open-settings

# native
STARK_API_PORT=8126 D:/cargo-target/stark/release/stark-rune.exe \
  --rune-shell-snap /tmp/settings-native.png --rune-route settings \
  --size 1280x900 --at 2

# hover pose over Check
python desktop-rune/parity/capture_react.py /tmp/settings-react-hover.png \
  --url http://localhost:5200/ --size 1280x900 --settle 6 \
  --desktop --open-settings --hover 888,285
STARK_API_PORT=8126 D:/cargo-target/stark/release/stark-rune.exe \
  --rune-shell-snap /tmp/settings-native-hover.png --rune-route settings \
  --size 1280x900 --at 2 --script parity/scripts/settings_hover_check.json

python desktop-rune/parity/compare.py \
  --native /tmp/settings-native.png --react /tmp/settings-react.png --align \
  --out /tmp/settings-diff
```

`parity/scripts/settings_open.json` taps the rail gear at 0.6s, so
`--at 0.70`/`0.80` catch the panel's fade-and-rise entrance.
`settings_import_appdata.json` opens the sheet and imports the `appdata`
candidate; the mock answers with the same `{message, copied}` shape as the
real server, and the status line lands at the bottom of the scroll body.

`parity/scripts/message_entrance.json` focuses `composer-input`, types
`motion probe`, and presses `composer-send` at 1.0s. Capture `--at 1.05` or
`1.07` for the user row's +14 pt slide and fade; at 1.05 the group is
+12.6 pt with opacity 0.10, at 1.07 it is +10.1 pt with opacity 0.28. The
fixture's `/api/run` answers without an SSE stream, so the shell refreshes
the chat a few frames later and the staged rows disappear. The remount rule
is covered by
`rune_transcript::tests::a_new_user_row_slides_in_once_and_never_replays_on_remount`,
which also asserts that a removed row unmounts without a fading ghost.

`--desktop` sets `window.__TAURI__` after the launch update check has settled
and hides the modal's `[role="status"]` line via a stylesheet, so the web
React capture shows the desktop-only Check row without faking an update
result. It does not alter the app; it only reaches the same branch a Tauri
window renders.

| capture | full mean | right mean | panel mean | align |
| --- | ---: | ---: | ---: | --- |
| 1280×900 rest | **1.183** | **1.109** | **1.748** | (0, 0) |
| 1280×900 hover Check | **1.183** | **1.109** | **1.748** | (0, 0) |
| 800×573 rest | **1.596** | **1.407** | **1.740** | (0, 0) |

`panel mean` is the modal rectangle (680×640 at 1280, 680×533 at 800). The
full-frame means include the app behind the modal, where Rune's map/coding
canvas legitimately differs from React's route; the modal is the parity
surface here. Geometry is exact: panel/header/body, the 57.75pt update and
export rows, the 124.75pt import row, the 74.25pt candidate rows, the 24.5pt
file control and the 26.5pt root input all land on React's CDP rects. Remaining
panel delta is glyph rasterisation plus the two Tailwind `tracking`
approximations (Rune rounds the label and button advances to whole points).

## Empty board (2026-11-21)

`--scenario empty` serves a successful boot with no tabs and no chats. Both
sides now render the 24/16 `canvas-empty` stack (27 pt white circle, 11.5/500
label, 10 pt copy), the always-mounted empty home card ("What to do Boss?"),
and `No chats yet.` in the sidebar. The add FAB and the map hint are absent
while the board is empty, matching `items.length > 0`.

## Group project surface (2026-11-21)

`--scenario group` is the deterministic detail fixture:

```bash
python desktop-rune/parity/mock_server.py --port 8128 --scenario group
# Rune
cd desktop-rune
STARK_API_PORT=8128 D:/cargo-target/stark/debug/stark-rune.exe \
  --rune-shell-snap group.png --rune-route group --size 1280x900 --at 1.8
# React (rail Group → first project)
python desktop-rune/parity/capture_react.py group-react.png \
  --url http://localhost:5201/ --size 1280x900 \
  --click 22,178 --click2 200,110
```

The fixture carries two milestones, four items (one awaiting approval), three
members, persisted messages and an event log, so the same capture covers the
chat, approval card, progress rail, board, team and activity tabs.

Geometry matches React: the modal is `1180×860` at `(50,20)` on 1280×900 and
`776×553` at `(12,10)` below 900 px; the rail is 350 pt on desktop and hidden
behind the mobile toggle; the skeleton is 16/12/12 plus 24/36/36/36 blocks on
the same 24 pt gutters. The 3 s detail poll reloads without replaying the modal
entrance. The chrome uses the same Lucide paths as React (`FolderKanban`,
`FolderOpen`, `StickyNote`, `PanelRight*`, `Pause`/`Play`, `X`, plus the
message/rail icons at their React sizes). The `group` route now navigates like
a click: it opens the Group rail, loads `/api/groups`, and mounts the modal
once the live list contains `p1`, so the shell behind the backdrop carries the
real project card and loaded home card. Remaining full-frame difference is the
dense text rasterisation and the canvas behind the blurred backdrop.

## Interaction sweep (2026-11-20)

The motion pass is split into shared primitives (`motion.rs`) and per-surface
entrances. CSS transitions that Rune has no direct hover binding for — scale,
offset and border colour — use `HoverSet` + `Live` tracks; `on_hover_change`
publishes the target and `tx.live_only()` keeps pointer moves out of rebuilds.
These scripted poses exercise the paths against the fixture server:

| script | pose |
| --- | --- |
| `message_entrance.json` | focus composer, type, send; capture 1.05/1.07 for the user slide |
| `composer_send_hover.json` | type, then hover Send; capture 1.04/1.30 for the 1.04 scale |
| `provider_hover.json` | hover the provider pill; its border rides a `Live` track |
| `trace_open.json` | open `msg-0a4560f6`'s trace; groups fade/rise, the body fades 180 ms, the header chevron rotates |
| `trace_close.json` | open, then close the trace; the body clips/fades over 180 ms inside the 280 ms row fold |
| `sidebar_delete.json` | two deletes; the 34.8 pt row fades, slides and collapses |
| `group_rail_open.json` | below 900 px, open the mobile details rail |
| `group_details.json` | open the Details tab and the activity log |
| `canvas_add_menu.json` | tap `canvas-add`; capture 1.06/1.20 for the `.985` menu settle |
| `agent_config_open.json` | tap `composer-provider`; backdrop 200 ms, panel 240 ms |
| `canvas_card_hover.json` | hover the right-hand card; 1 pt lift + border track |
| `group_header_hover.json` | switch to Coding, hover a project header; wash + 1.08 chevron |
| `new_session_entrance.json` | create a tab; the new tab mounts (sidebar rows use the 260 ms entrance when a chat is created) |
| `settings_open.json` | rail gear; panel 240 ms |

Measured checkpoints:

| pose | frame | observation |
| --- | --- | --- |
| user message | 1.05 | group +12.6 pt, opacity 0.10 |
| user message | 1.07 | group +10.1 pt, opacity 0.28 |
| trace group | 1.06 / 1.12 / 1.40 | opacity 0.17 / 0.56 / 1.00 |
| trace body | 1.06 / 1.12 / 1.40 | opacity 0.078 / 0.632 / 0.997 (180 ms `ease`) |
| trace close | 1.66 / 1.70 / 1.78 | body 38.67 / 26.14 / 7.05 pt, opacity 0.914 / 0.532 / 0.041; no ghost |
| group 1280 | rest | full-frame 7.54 / 6.68; modal and rail geometry exact |
| group 800 | rest | full-frame 9.07 / 3.88; modal 776×553 at `(12,10)` |
| sidebar delete | unit probe | 34.8 pt slot → 7.8 pt with opacity 0.22 on the same frame |
| send hover | 1.30 | scale 1.0382 (settles at 1.04), white face |
| provider border | 1.6 | edge RGB 42→62 over the dock |
| group chevron | 1.9 | scale 1.0800 under the pointer |
| canvas card | 1.6 | 1 pt lift (0.45 pt at 0.45 camera zoom), border on the Live track |
| canvas add menu | 1.06 / 1.20 | scale 0.9906 / 0.9998, opacity 0.38 / 0.99 |
| AgentConfig | 1.12 / 1.30 | opacity 0.52 / 0.975, backdrop and card on separate tracks |

### Accepted deltas

No accepted motion delta remains on the trace close or the group dialog:
`.trace-collapsible` now folds the body inside its own clipped growth row
(`46.75 → 38.67 → 26.14 → 7.05 pt`) on the 280 ms disclosure spring while its
opacity follows the 180 ms `ease` clock, and the group chrome uses the real
Lucide geometry at React's sizes.

The sidebar delete now collapses its 34.8 pt slot on the CSS bezier (content
absolute, the list lands layout immediately), the trace body fades in over
180 ms, and the voice dot carries the full 1.2 s scale/opacity keyframe.
`.canvas-empty` is the React 24/16 stack with the 27 pt white circle; its
`scale(1.04)` and `#fff → #e8e8ea` fill are real hover tracks. A successful
empty boot snapshot now clears the demo cards and shows the empty home card.

Hover transforms, borders, image zoom, the project chevron, the project-header
wash and the chat-row/subagent 1 px slides are now implemented. The later
`index.css` send-button rule is the target: white 34 pt circle, hover
`scale(1.04)`, disabled at 72%, running square. The earlier gradient/brightness
block is overridden in the current cascade.

### Loop audit

This table is built from actual React usage, not every keyframe in the CSS.
Dead keyframes (`.voice-orb` 1.5 s, `canvas-dot-pulse` 1.4 s) are omitted.

| loop | React period / usage | Rune |
| --- | --- | --- |
| thinking dots | 1.1 s, 0.14/0.28 stagger (`house.css:73-76`) | `opacity(1.1, 0/0.14/0.28)` |
| retry glow | 1.4 s (`index.css:411`) | `opacity(1.4)` |
| session run dot | 1.6 s (`house.css:128`) | `opacity(1.6)` |
| running card sheen | 2.6 s linear (`canvas.css:138`) | `slide(2.6, …, LINEAR)` |
| voice live dot | 1.2 s, 5 px + 2 px ring (`index.css:223`) | 5 px white + 9 px ring, `Live` scale/opacity at 60 Hz |
| cloud pulse | 2 s `animate-pulse` (PromptDock/CloudBadge) | `opacity(2.0)` |
| caret | 0.9 s `steps(1)` (`index.css:555`) | near-step `Cycle`, 0.9 s |
| trace-live spinner | 1.8 s (`index.css:245`) | `rotation(1.8)` |
| subagent spinner | 1.6 s (`index.css:317`) | `rotation(1.6)` |
| tool/retry spinner | 1 s `animate-spin` | `rotation(1.0)` |
| group skeleton | 1.4 s (`house.css:737`) | linear gradient slide, 1.4 s |

Rune's canvas status dot is static, matching `.canvas-card-status-dot`; the
extra 2.2 s badge pulse was removed. Reduce Motion zeroes looping amplitude
through Rune's motion gate; finite sequences still land.
