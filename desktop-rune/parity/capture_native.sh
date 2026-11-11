#!/usr/bin/env bash
# Deterministic native capture against the frozen fixture server.
#   STARK_API_PORT=8125 parity/capture_native.sh out.png 800x573 open_hello.json 4
set -euo pipefail
OUT=${1:?usage: capture_native.sh OUT.png [WxH] [SCRIPT] [AT]}
SIZE=${2:-800x573}
SCRIPT=${3:-}
AT=${4:-4}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
BIN=${STARK_RUNE_BIN:-D:/cargo-target/stark/debug/stark-rune.exe}
ARGS=(--rune-shell-snap "$OUT" --size "$SIZE" --at "$AT")
if [ -n "$SCRIPT" ]; then
  ARGS+=(--script "$ROOT/desktop-rune/parity/scripts/$SCRIPT")
fi
STARK_API_PORT=${STARK_API_PORT:-8125} "$BIN" "${ARGS[@]}"
