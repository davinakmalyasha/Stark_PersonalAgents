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
curl -fsS "$BASE/api/health"; echo

echo "==> provider key (boolean only)"
curl -fsS "$BASE/api/provider" | python3 -c "import json,sys; j=json.load(sys.stdin); print({k:j.get(k) for k in ('provider','base_url','has_key','legacy_env')})"

echo "==> drain marker"
[ -f /var/lib/stark-data/.drain ] && echo "STILL DRAINING" || echo "clear"

echo "==> export"
CODE=$(curl -s -o "$OUT/verify-backup.zip" -w '%{http_code}' "$BASE/api/data/export")
SIZE=$(stat -c%s "$OUT/verify-backup.zip")
echo "http=$CODE size=$SIZE"
python3 - "$OUT/verify-backup.zip" <<'PY'
import sys, zipfile
z = zipfile.ZipFile(sys.argv[1])
names = z.namelist()
assert "manifest.json" in names
assert ".deep_agents/chats.db" in names
assert not any(n.endswith(".env") for n in names), "secret leak!"
assert not any(n.endswith("-wal") or n.endswith("-shm") for n in names)
print("zip entries:", len(names), "| .env present:", any(n.endswith(".env") for n in names))
PY

echo "==> restore drill"
bash /opt/stark/restore-drill.sh "$OUT/verify-backup.zip"

echo "==> paired devices"
curl -fsS "$BASE/api/pairing/devices" | python3 -c "import json,sys; j=json.load(sys.stdin); print('devices:', len(j.get('devices',[])))"

echo "==> landlock"
grep -q '^STARK_LANDLOCK=' /etc/stark/stark.env && grep '^STARK_LANDLOCK=' /etc/stark/stark.env || echo "landlock: off (opt-in)"
journalctl -u stark -n 3 --no-pager | grep -i sandbox || true

echo "==> OK"
