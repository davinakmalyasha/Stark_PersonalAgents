#!/usr/bin/env bash
# Post-deploy verification. Run on the VPS after a deploy.
set -euo pipefail
BASE=http://127.0.0.1:8123
OUT=/root/stark-verify
mkdir -p "$OUT"

echo "==> service"
systemctl is-active stark
systemctl show stark -p MainPID --value
free -h | head -2

echo "==> health"
HEALTH="$(curl -fsS "$BASE/api/health")"
echo "$HEALTH"
EXPECTED="$(cat /opt/stark/current/REVISION 2>/dev/null || true)"
if [ -n "$EXPECTED" ]; then
  ACTUAL="$(printf '%s' "$HEALTH" | jq -r '.build // empty')"
  if [ "$ACTUAL" != "$EXPECTED" ]; then
    echo "health build '$ACTUAL' does not match deployed revision '$EXPECTED'" >&2
    exit 1
  fi
  echo "build matches $EXPECTED"
fi

echo "==> provider key (boolean only)"
curl -fsS "$BASE/api/provider" | jq '{provider, base_url, has_key, legacy_env}'

echo "==> drain marker"
[ -f /var/lib/stark-data/.drain ] && echo "STILL DRAINING" || echo "clear"

echo "==> export"
CODE=$(curl -s -o "$OUT/verify-backup.zip" -w '%{http_code}' "$BASE/api/data/export")
SIZE=$(stat -c%s "$OUT/verify-backup.zip")
echo "http=$CODE size=$SIZE"
unzip -Z1 "$OUT/verify-backup.zip" > "$OUT/entries.txt"
grep -qx 'manifest.json' "$OUT/entries.txt"
grep -qx '.deep_agents/chats.db' "$OUT/entries.txt"
! grep -Eq '(^|/)\.env$' "$OUT/entries.txt" || { echo "secret leak!"; exit 1; }
! grep -Eq '(-wal|-shm)$' "$OUT/entries.txt" || { echo "sqlite sidecar in backup"; exit 1; }
echo "zip entries: $(wc -l < "$OUT/entries.txt")"

echo "==> restore drill"
bash /opt/stark/restore-drill.sh "$OUT/verify-backup.zip"

echo "==> paired devices"
curl -fsS "$BASE/api/pairing/devices" | jq -r '"devices: \(.devices | length)"'

echo "==> landlock"
grep -q '^STARK_LANDLOCK=' /etc/stark/stark.env && grep '^STARK_LANDLOCK=' /etc/stark/stark.env || echo "landlock: off (opt-in)"
journalctl -u stark -n 3 --no-pager | grep -i sandbox || true

echo "==> OK"
