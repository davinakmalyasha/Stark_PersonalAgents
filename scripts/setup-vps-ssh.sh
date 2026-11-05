#!/usr/bin/env bash
# One-time VPS-side setup for key-only deploys. Run as root ON THE VPS.
#
#   bash setup-vps-ssh.sh /root/laptop.pub [/root/actions.pub]
#   bash setup-vps-ssh.sh --harden
#
# Phase 1 installs one key per actor (laptop, GitHub Actions) and the release
# scripts. Phase 2 disables password auth. Run phase 2 only after a key login
# works in a second terminal — the provider console stays the break-glass.
set -euo pipefail

APP=/opt/stark

usage() {
  cat <<'EOF'
usage: bash setup-vps-ssh.sh <public-key-file> [<public-key-file> ...]
       bash setup-vps-ssh.sh --harden
       bash setup-vps-ssh.sh --tailscale-ssh
EOF
  exit 2
}

install_key() {
  local file="$1"
  install -d -m 700 /root/.ssh
  touch /root/.ssh/authorized_keys
  chmod 600 /root/.ssh/authorized_keys
  if ! grep -qxF "$(cat "$file")" /root/.ssh/authorized_keys; then
    cat "$file" >> /root/.ssh/authorized_keys
    echo "    key added: $(awk '{print $3}' "$file")"
  else
    echo "    key already present: $(awk '{print $3}' "$file")"
  fi
}

configure_fail2ban() {
  if systemctl list-unit-files fail2ban.service >/dev/null 2>&1; then
    cat > /etc/fail2ban/jail.d/stark-sshd.local <<'EOF'
[sshd]
enabled = true
maxretry = 5
findtime = 10m
bantime = 1h
EOF
    systemctl enable --now fail2ban >/dev/null 2>&1 || true
    systemctl restart fail2ban 2>/dev/null || true
    echo "    fail2ban sshd jail: maxretry=5, bantime=1h"
  fi
}

if [ "${1:-}" = "--tailscale-ssh" ]; then
  command -v tailscale >/dev/null 2>&1 \
    || { echo "!! tailscale is not installed"; exit 1; }
  # Recovery path: a new device that joins the tailnet can SSH using its
  # Tailscale identity, with no key to copy. ACL must allow the ssh rule.
  if tailscale set --ssh=true 2>/dev/null; then
    echo "==> Tailscale SSH enabled (tailscale set)"
  else
    echo "==> enabling via tailscale up (may reset other up flags)"
    tailscale up --ssh
  fi
  if ufw status 2>/dev/null | grep -q "Status: active"; then
    ufw allow in on tailscale0 to any port 22 proto tcp >/dev/null
    echo "    22/tcp allowed on tailscale0"
  fi
  tailscale status | head -3 || true
  echo "    ACL: allow ssh from your tailnet identity, e.g."
  echo '    "ssh": [{"action":"accept","src":["autogroup:member"],"dst":["autogroup:self"],"users":["root"]}]'
  exit 0
fi

if [ "${1:-}" = "--harden" ]; then
  test -s /root/.ssh/authorized_keys \
    || { echo "!! no key installed; run phase 1 first"; exit 1; }

  cat > /etc/ssh/sshd_config.d/99-stark-hardening.conf <<'EOF'
# Stark: key-only SSH. Keep key login for root as break-glass.
PubkeyAuthentication yes
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitEmptyPasswords no
PermitRootLogin prohibit-password
AllowAgentForwarding no
X11Forwarding no
EOF
  sshd -t
  # Ubuntu 26.04 socket-activates ssh; sshd re-reads config per connection.
  systemctl reload ssh 2>/dev/null || systemctl restart ssh 2>/dev/null || true

  configure_fail2ban
  echo "==> hardened: password login disabled, root is key-only"
  echo "    provider console remains the last-resort recovery"
  exit 0
fi

[ "$#" -ge 1 ] || usage
for key in "$@"; do
  [ -f "$key" ] || { echo "!! key file not found: $key"; usage; }
  grep -Eq '^(ssh-ed25519|ssh-rsa|ecdsa-sha2-)' "$key" \
    || { echo "!! not an SSH public key: $key"; exit 1; }
done

echo "==> authorized keys"
for key in "$@"; do install_key "$key"; done

echo "==> release scripts"
install -d -m 755 "$APP"
for script in deploy-vps.sh verify-cloud.sh restore-drill.sh; do
  if [ -f "$(dirname "$0")/$script" ]; then
    install -m 755 "$(dirname "$0")/$script" "$APP/$script"
    echo "    installed /opt/stark/$script"
  fi
done

echo "==> firewall"
if ufw status 2>/dev/null | grep -q "Status: active"; then
  ufw allow 20128/tcp >/dev/null
  echo "    20128/tcp allowed (deploy)"
  echo "    8123 stays tailnet-only"
fi

configure_fail2ban

echo "==> phase 1 done"
echo "    test now, in a second terminal:"
echo "      ssh -i ~/.ssh/stark_deploy_ed25519 -p 20128 root@<vps> 'echo KEY_LOGIN_OK'"
echo "    then disable passwords:"
echo "      bash $0 --harden"
