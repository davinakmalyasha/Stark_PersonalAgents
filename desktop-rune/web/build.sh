#!/usr/bin/env bash
# Stark web shell: wasm -> wasm-bindgen glue -> desktop-rune/web/pkg.
# Mirrors anomalyco/rune scripts/web-build.sh for our package.
#
#   desktop-rune/web/build.sh [--release] [--out DIR]
#
# Prerequisites: wasm32-unknown-unknown target, wasm-bindgen-cli 0.2.129
# (must match Cargo.lock). Serve desktop-rune/web over HTTP and open
# index.html in a WebGPU browser (Chrome/Edge 113+, localhost is fine).
set -euo pipefail

PROFILE=dev
OUT=desktop-rune/web/pkg
while [ $# -gt 0 ]; do
  case "$1" in
    --release) PROFILE=release; shift ;;
    --out) OUT="$2"; shift 2 ;;
    *) echo "usage: build.sh [--release] [--out DIR]" >&2; exit 2 ;;
  esac
done

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

if [ "$PROFILE" = release ]; then
  cargo build --manifest-path "$ROOT/Cargo.toml" -p stark-rune-web --target wasm32-unknown-unknown --release
  WASM="$TARGET_DIR/wasm32-unknown-unknown/release/stark_rune_web.wasm"
else
  cargo build --manifest-path "$ROOT/Cargo.toml" -p stark-rune-web --target wasm32-unknown-unknown
  WASM="$TARGET_DIR/wasm32-unknown-unknown/debug/stark_rune_web.wasm"
fi
rm -rf "$OUT"
mkdir -p "$OUT"
wasm-bindgen --target web --out-dir "$OUT" --out-name stark_rune_web "$WASM"
ls -la "$OUT"
