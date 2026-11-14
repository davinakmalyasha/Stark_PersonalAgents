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

Geometry matches exactly: code card 385.5 vs 385.406, diff wrapper 463.2 vs
463.1875, table wrapper 210.19 vs 210.1875; diff rows 18.59 and header 39.2
match. The diff's remaining delta is Chrome's LCD subpixel antialiasing on
the opaque card (Rune draws grayscale) plus a one-device-pixel baseline
difference, corrected with Rune's non-layout `.offset(0, 1)` on diff glyphs.
Code/table bands are dominated by Consolas-vs-Consolas rasterisation at 12–13.5pt.

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

Native-only demo routes (`--rune-route stream` / `trace`) render the same
components without a fixture server for quick visual checks.

## State scripts

`parity/scripts/*.json` are Rune headless input scripts (`tap`, `wheel`,
`drag`, `type`, …). Add one per visual state so both sides capture the same
scene. The React side may need a matching DOM scroll (`scrollTop`) or a click;
keep those instructions next to the script.
