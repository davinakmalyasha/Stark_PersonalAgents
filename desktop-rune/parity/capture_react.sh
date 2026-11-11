#!/usr/bin/env bash
# Deterministic React capture: headless Chrome, 1x scale, no scrollbars.
#   parity/capture_react.sh out.png [URL] [800,573]
set -euo pipefail
OUT=${1:?usage: capture_react.sh OUT.png [URL] [W,H]}
URL=${2:-http://localhost:5200/}
SIZE=${3:-800,573}
CHROME=${CHROME:-C:/Program Files/Google/Chrome/Application/chrome.exe}
PROFILE=${CHROME_PROFILE:-C:/Users/MY LENOVO/AppData/Local/Temp/opencode/chrome-parity}
# Chrome wants a Windows path for --screenshot.
if command -v cygpath >/dev/null 2>&1; then
  OUT_WIN=$(cygpath -w "$OUT")
else
  OUT_WIN=$OUT
fi
"$CHROME" --headless=new --disable-gpu --no-first-run \
  --user-data-dir="$PROFILE" \
  --window-size="$SIZE" --force-device-scale-factor=1 --hide-scrollbars \
  --virtual-time-budget=12000 \
  --screenshot="$OUT_WIN" "$URL"
