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

# 3. native app against the same fixtures
cd desktop-rune && cargo build -p stark-rune-app --features rune-real
```

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
| frozen fixtures, 1× | **2.306** | **2.986** | (0, 0) |

The remaining difference is text rasterisation (Chrome glyph AA vs Rune's
DirectWrite/Skia-gamma atlas), not geometry. Any `align` other than `(0, 0)`
is a real layout shift and should be fixed first.

## State scripts

`parity/scripts/*.json` are Rune headless input scripts (`tap`, `wheel`,
`drag`, `type`, …). Add one per visual state so both sides capture the same
scene. The React side may need a matching DOM scroll (`scrollTop`) or a click;
keep those instructions next to the script.
