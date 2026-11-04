#!/usr/bin/env bash
# Drain-safe Stark deploy. Runs ON THE VPS as root.
#
#   bash deploy-vps.sh /root/stark-linux-x86_64.tar.gz
#
# Layout: /opt/stark/releases/<stamp>/{stark-server,frontend}
#         /opt/stark/current -> releases/<stamp>
# Rollback keeps /opt/stark/previous pointing at the last healthy release.
set -euo pipefail

ARCHIVE="${1:?usage: deploy-vps.sh <stark-linux-x86_64.tar.gz>}"
APP=/opt/stark
RELEASES="$APP/releases"
CURRENT="$APP/current"
PREVIOUS="$APP/previous"
DATA=/var/lib/stark-data
STAMP="$(date +%Y%m%d-%H%M%S)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

echo "==> extract"
tar -xzf "$ARCHIVE" -C "$WORK"
test -f "$WORK/opt/stark/stark-server"

mkdir -p "$RELEASES/$STAMP"
mv "$WORK/opt/stark/stark-server" "$RELEASES/$STAMP/stark-server"
mv "$WORK/opt/stark-frontend" "$RELEASES/$STAMP/frontend"
chmod +x "$RELEASES/$STAMP/stark-server"
chown -R stark:stark "$RELEASES/$STAMP"

# Unit points at the `current` symlink so releases are immutable.
cat > /etc/systemd/system/stark.service <<'EOF'
[Unit]
Description=Stark harness server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=stark
Group=stark
WorkingDirectory=/var/lib/stark-data
EnvironmentFile=/etc/stark/stark.env
ExecStart=/opt/stark/current/stark-server
Restart=always
RestartSec=3
NoNewPrivileges=true
PrivateTmp=true
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
EOF
sed -i "s#^STARK_FRONTEND_DIST=.*#STARK_FRONTEND_DIST=/opt/stark/current/frontend#" /etc/stark/stark.env || true
systemctl daemon-reload

echo "==> drain (refuse new runs, wait for active ones)"
touch "$DATA/.drain"
for _ in $(seq 1 90); do
  ACTIVE="$(curl -s --max-time 3 http://127.0.0.1:8123/api/runs/active || true)"
  if ! printf '%s' "$ACTIVE" | grep -q '"running":true'; then
    break
  fi
  echo "    a run is active; waiting 10s"
  sleep 10
done

echo "==> swap"
if [ -L "$CURRENT" ] || [ -d "$CURRENT" ]; then
  PREV_TARGET="$(readlink -f "$CURRENT" || true)"
  [ -n "$PREV_TARGET" ] && ln -sfn "$PREV_TARGET" "$PREVIOUS"
fi
ln -sfn "$RELEASES/$STAMP" "$CURRENT"
systemctl restart stark

echo "==> health"
HEALTHY=0
for _ in $(seq 1 30); do
  if curl -fsS --max-time 3 http://127.0.0.1:8123/api/health | grep -q '"ok":true'; then
    HEALTHY=1
    break
  fi
  sleep 2
done

if [ "$HEALTHY" -ne 1 ]; then
  echo "!! new release is unhealthy — rolling back"
  if [ -L "$PREVIOUS" ]; then
    ln -sfn "$(readlink -f "$PREVIOUS")" "$CURRENT"
    systemctl restart stark
  fi
  rm -f "$DATA/.drain"
  echo "rolled back; investigate: journalctl -u stark -n 80 --no-pager"
  exit 1
fi

rm -f "$DATA/.drain"
# Keep the five newest releases.
ls -1dt "$RELEASES"/*/ | tail -n +6 | xargs -r rm -rf
echo "==> deployed $STAMP"
systemctl status stark --no-pager | head -6
