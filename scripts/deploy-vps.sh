#!/usr/bin/env bash
# Drain-safe Stark deploy. Runs ON THE VPS as root.
#
#   bash deploy-vps.sh /root/stark-linux-x86_64.tar.gz [git-sha]
#
# Layout: /opt/stark/releases/<stamp>/{stark-server,frontend,REVISION}
#         /opt/stark/current -> releases/<stamp>
# Rollback keeps /opt/stark/previous pointing at the last healthy release.
#
# When invoked from outside /opt/stark (CI uploads to a temp dir), the script
# installs itself and its companions into /opt/stark and re-execs the newest
# copy, so updated deploy logic always takes effect on the same run.
set -euo pipefail

APP=/opt/stark
RELEASES="$APP/releases"
CURRENT="$APP/current"
PREVIOUS="$APP/previous"
DATA=/var/lib/stark-data
ARCHIVE="${1:?usage: deploy-vps.sh <stark-linux-x86_64.tar.gz> [git-sha]}"
REVISION="${2:-unknown}"
STAMP="$(date +%Y%m%d-%H%M%S)"
SELF="$(readlink -f "$0" 2>/dev/null || echo "$0")"
SELF_DIR="$(cd "$(dirname "$SELF")" && pwd)"

if [ "$SELF" != "$APP/deploy-vps.sh" ]; then
  install -d -m 755 "$APP"
  for script in deploy-vps.sh verify-cloud.sh restore-drill.sh; do
    [ -f "$SELF_DIR/$script" ] && install -m 755 "$SELF_DIR/$script" "$APP/$script"
  done
  exec "$APP/deploy-vps.sh" "$@"
fi

WORK="$(mktemp -d)"

cleanup() {
  local code=$?
  rm -rf "$WORK"
  if [ "$code" -ne 0 ]; then
    rm -f "$DATA/.drain"
  fi
}
trap cleanup EXIT

# One deploy at a time; the second call waits instead of racing the symlink.
LOCK=/var/lock/stark-deploy.lock
exec 9>"$LOCK"
flock -w 600 9 || { echo "!! another deploy holds $LOCK"; exit 1; }

echo "==> extract ($REVISION)"
tar -xzf "$ARCHIVE" -C "$WORK"
test -f "$WORK/opt/stark/stark-server"
test -f "$WORK/opt/stark-frontend/index.html" \
  || test -f "$WORK/opt/stark-frontend/index.htm" \
  || { echo "!! archive has no frontend"; exit 1; }

mkdir -p "$RELEASES/$STAMP"
mv "$WORK/opt/stark/stark-server" "$RELEASES/$STAMP/stark-server"
mv "$WORK/opt/stark-frontend" "$RELEASES/$STAMP/frontend"
printf '%s\n' "$REVISION" > "$RELEASES/$STAMP/REVISION"
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
if [ ! -f /etc/stark/stark.env ]; then
  mkdir -p /etc/stark
  cat > /etc/stark/stark.env <<'EOF'
STARK_HARNESS_ROOT=/var/lib/stark-data
STARK_FRONTEND_DIST=/opt/stark/current/frontend
STARK_HOST=0.0.0.0
STARK_PORT=8123
STARK_MAX_CONCURRENT_RUNS=10
RUST_LOG=stark_server=info,tower_http=info
EOF
  chown stark:stark /etc/stark/stark.env
fi
sed -i "s#^STARK_FRONTEND_DIST=.*#STARK_FRONTEND_DIST=/opt/stark/current/frontend#" /etc/stark/stark.env || true
grep -q '^STARK_FRONTEND_DIST=' /etc/stark/stark.env || echo 'STARK_FRONTEND_DIST=/opt/stark/current/frontend' >> /etc/stark/stark.env
systemctl daemon-reload

echo "==> drain (refuse new runs, wait for active ones)"
mkdir -p "$DATA"
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
  echo "rolled back; investigate: journalctl -u stark -n 80 --no-pager"
  exit 1
fi

rm -f "$DATA/.drain"
# Keep the five newest releases.
ls -1dt "$RELEASES"/*/ | tail -n +6 | xargs -r rm -rf
echo "==> deployed $STAMP ($REVISION)"
systemctl status stark --no-pager | head -6
