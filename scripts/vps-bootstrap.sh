#!/usr/bin/env bash
# One-shot VPS bootstrap (Ubuntu 22.04/24.04/26.04). Run as root:
#   bash vps-bootstrap.sh <ssh-port>
# Idempotent: safe to re-run after a rebuild.
set -euo pipefail

SSH_PORT="${1:-22}"

echo "==> packages"
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get upgrade -y
apt-get install -y libasound2t64 fail2ban ufw git curl ca-certificates

echo "==> timezone"
timedatectl set-timezone Asia/Jakarta || true

echo "==> swap (2GB)"
if ! swapon --show=NAME --noheadings | grep -q /swapfile; then
  fallocate -l 2G /swapfile 2>/dev/null || dd if=/dev/zero of=/swapfile bs=1M count=2048 status=none
  chmod 600 /swapfile
  mkswap /swapfile >/dev/null
  swapon /swapfile
fi
grep -q '/swapfile' /etc/fstab || echo '/swapfile none swap sw 0 0' >> /etc/fstab
sysctl -w vm.swappiness=10 >/dev/null
sysctl -w vm.vfs_cache_pressure=50 >/dev/null
grep -q 'vm.swappiness=10' /etc/sysctl.conf 2>/dev/null || echo 'vm.swappiness=10' >> /etc/sysctl.conf
grep -q 'vm.vfs_cache_pressure=50' /etc/sysctl.conf 2>/dev/null || echo 'vm.vfs_cache_pressure=50' >> /etc/sysctl.conf

echo "==> remove desktop-only bloat"
apt-get purge -y snapd modemmanager udisks2 || true
apt-get autoremove -y || true

echo "==> firewall (deny incoming; SSH + tailnet only)"
ufw allow "${SSH_PORT}"/tcp comment 'SSH - do not remove'
ufw allow in on tailscale0 to any port 8123 proto tcp
ufw --force enable

echo "==> service user + dirs"
id stark >/dev/null 2>&1 || useradd -r -m -s /usr/sbin/nologin stark
mkdir -p /opt/stark/releases /var/lib/stark-data /srv/stark-projects
chown -R stark:stark /opt/stark /var/lib/stark-data /srv/stark-projects
install -d -m 750 -o stark -g stark /etc/stark

echo "==> fail2ban"
systemctl enable --now fail2ban || true

echo "==> done"
free -h
ufw status verbose
