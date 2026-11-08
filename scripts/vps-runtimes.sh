#!/usr/bin/env bash
# Complete VPS agent runtime, Ubuntu 22.04/24.04/26.04. Run as root:
#   bash vps-runtimes.sh
#
# Installs current language runtimes so agents can build/test anything the
# harness delegates: Rust, Go, Node 26 (Lithium), PostgreSQL, and the Vercel
# Labs agent-browser CLI with Chrome for Testing. Idempotent.
#
# No PHP, no Java. Voice STT/TTS stays off the VPS.
set -euo pipefail

echo "==> base packages"
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y --no-install-recommends \
  build-essential pkg-config libssl-dev git curl ca-certificates gnupg \
  python3 python3-venv python3-pip jq unzip xz-utils \
  libasound2t64 libnss3 libatk1.0-0 libatk-bridge2.0-0 libcups2 \
  libdrm2 libxkbcommon0 libxcomposite1 libxdamage1 libxfixes3 libxrandr2 \
  libgbm1 libpango-1.0-0 libcairo2 libasound2-plugins fonts-liberation

echo "==> rust (stable)"
if ! command -v rustup >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --profile minimal --default-toolchain stable
fi
# shellcheck disable=SC1091
. /root/.cargo/env

echo "==> go (latest)"
GO_VERSION="$(curl -fsSL https://go.dev/VERSION?m=text | head -n1)"
if [ -n "$GO_VERSION" ] && ! /usr/local/go/bin/go version 2>/dev/null | grep -q "$GO_VERSION"; then
  ARCH="$(dpkg --print-architecture)"
  curl -fsSLo /tmp/go.tgz "https://go.dev/dl/${GO_VERSION}.linux-${ARCH}.tar.gz"
  rm -rf /usr/local/go
  tar -C /usr/local -xzf /tmp/go.tgz
  rm -f /tmp/go.tgz
fi
ln -sfn /usr/local/go/bin/go /usr/local/bin/go
ln -sfn /usr/local/go/bin/gofmt /usr/local/bin/gofmt

echo "==> node 26 (Lithium)"
if ! command -v node >/dev/null 2>&1 || ! node -v | grep -q '^v26\.'; then
  curl -fsSL https://deb.nodesource.com/setup_26.x | bash -
  apt-get install -y nodejs
fi

echo "==> postgresql"
apt-get install -y postgresql postgresql-contrib
systemctl enable --now postgresql
# 2GB-box profile: modest shared buffers, few connections.
PG_CONF="$(find /etc/postgresql -name postgresql.conf | sort -V | tail -n1)"
if [ -n "$PG_CONF" ] && ! grep -q '^# stark tuning' "$PG_CONF"; then
  {
    echo '# stark tuning'
    echo 'shared_buffers = 128MB'
    echo 'effective_cache_size = 512MB'
    echo 'work_mem = 4MB'
    echo 'maintenance_work_mem = 32MB'
    echo 'max_connections = 40'
  } >> "$PG_CONF"
  systemctl restart postgresql
fi

echo "==> agent-browser (latest) + Chrome for Testing"
npm install -g agent-browser@latest
# Chrome needs a writable HOME; the stark service user owns the harness.
if command -v sudo >/dev/null 2>&1 && id stark >/dev/null 2>&1; then
  sudo -u stark -H npx --yes agent-browser install || true
else
  agent-browser install || true
fi

echo "==> versions"
rustc --version || true
go version || true
node -v
npm -v
psql --version
agent-browser --version || true
free -h
