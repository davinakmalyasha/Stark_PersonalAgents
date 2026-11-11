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
| frozen fixtures, 1× | **2.130** | **2.751** | (0, 0) |

The renderer embeds the exact `InterVariable.ttf` the browser loads (the
built-in static Inter shifted every text box); that plus the live camera took
the baseline from 2.306 to 2.130.

Release native renders pixel-identical to debug (mean 0.0) at ~4× the
speed: animating p50 3.0–4.0 ms, forced repaint p50 2.5 ms, zero draws at
rest. Canvas pan/zoom publishes through Rune `Live` values and says
`tx.live_only()`, so dragging the map never rebuilds the view. The release
wasm is 11.2 MB before the embedded font (debug was 76.9 MB).

The remaining difference is text rasterisation (Chrome glyph AA vs Rune's
DirectWrite/Skia-gamma atlas), not geometry. Any `align` other than `(0, 0)`
is a real layout shift and should be fixed first.

## State scripts

`parity/scripts/*.json` are Rune headless input scripts (`tap`, `wheel`,
`drag`, `type`, …). Add one per visual state so both sides capture the same
scene. The React side may need a matching DOM scroll (`scrollTop`) or a click;
keep those instructions next to the script.
