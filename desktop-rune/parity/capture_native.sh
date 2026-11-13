#!/usr/bin/env bash
# Deterministic native capture against the frozen fixture server.
#   STARK_API_PORT=8125 parity/capture_native.sh out.png 800x573 open_hello.json 4
set -euo pipefail
OUT=${1:?usage: capture_native.sh OUT.png [WxH] [SCRIPT] [AT]}
SIZE=${2:-800x573}
SCRIPT=${3:-}
AT=${4:-4}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
if [ -z "${STARK_RUNE_BIN:-}" ]; then
  if [ -x "D:/cargo-target/stark/release/stark-rune.exe" ]; then
    BIN=D:/cargo-target/stark/release/stark-rune.exe
  else
    BIN=D:/cargo-target/stark/debug/stark-rune.exe
  fi
else
  BIN=$STARK_RUNE_BIN
fi
ARGS=(--rune-shell-snap "$OUT" --size "$SIZE" --at "$AT")
if [ -n "$SCRIPT" ]; then
  ARGS+=(--script "$ROOT/desktop-rune/parity/scripts/$SCRIPT")
fi
# Freeze relative-time labels for both sides. Set STARK_PARITY_NOW= (empty)
# to use the live clock instead.
STARK_PARITY_NOW=${STARK_PARITY_NOW-2026-11-12T12:00:00Z}
STARK_API_PORT=${STARK_API_PORT:-8125} STARK_PARITY_NOW="$STARK_PARITY_NOW" "$BIN" "${ARGS[@]}"
