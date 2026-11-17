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
| 1280×900 tasks top | **2.627** | **2.697** | (0, 0) |
| 1280×900 tasks end (h1-anchored) | **3.781** | **3.481** | (0, 0) |

Closing the tasks gap took six implementation fixes (all follow React's
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
- Quote border side: `border_sides` is CSS-ordered `[top,right,bottom,left]`
  (confirmed in the Rune shader + `set_border` + pixel probes), so the
  `border-l-2` bar is `[0,0,0,2]`. The old `[2,0,0,0]` painted a *top*
  border that also consumed 2px of layout room and pushed the quote content
  down; the side fix is covered by `quote_well_mounts_content_at_pad`.

Remaining tasks delta is glyph-level rasterisation (Inter grayscale AA on
both sides, different rasterisers) plus the native-only Latest pill at the
top pose (React hides its pill); the end pose is pill-free on both sides.
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
