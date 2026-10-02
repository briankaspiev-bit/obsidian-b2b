#!/usr/bin/env bash
# Installs the rendezvous server on a fresh Ubuntu VM as a systemd service on
# UDP 3478. Run as root, e.g. from the provider's browser console:
#   curl -sSf https://raw.githubusercontent.com/briankaspiev-bit/obsidian-b2b/main/services/rendezvous/deploy/install.sh | bash
# Re-running it updates to the latest code. BRANCH=... picks another branch.
set -euo pipefail

BRANCH=${BRANCH:-main}
REPO=https://github.com/briankaspiev-bit/obsidian-b2b.git
SRC=/opt/obsidian-b2b

apt-get update -q
apt-get install -yq build-essential git curl
if ! command -v cargo >/dev/null; then
  curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
fi
. "$HOME/.cargo/env"

if [[ -d $SRC/.git ]]; then
  git -C "$SRC" fetch -q origin "$BRANCH"
  git -C "$SRC" checkout -q -B "$BRANCH" "origin/$BRANCH"
else
  git clone -q --branch "$BRANCH" "$REPO" "$SRC"
fi
cargo build --release --manifest-path "$SRC/Cargo.toml" -p obsidian-rendezvous --bin rendezvous-server
install -m 755 "$SRC/target/release/rendezvous-server" /usr/local/bin/

id obsidian >/dev/null 2>&1 || useradd --system --no-create-home --shell /usr/sbin/nologin obsidian
cat >/etc/systemd/system/obsidian-rendezvous.service <<'UNIT'
[Unit]
Description=Obsidian B2B rendezvous (rooms, hole punching, relay)
After=network-online.target
Wants=network-online.target

[Service]
User=obsidian
ExecStart=/usr/local/bin/rendezvous-server --bind 0.0.0.0:3478
Restart=always
RestartSec=2
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
UNIT
systemctl daemon-reload
systemctl enable --now obsidian-rendezvous
systemctl restart obsidian-rendezvous

if command -v ufw >/dev/null && ufw status | grep -q active; then
  ufw allow 3478/udp
fi

IP=$(curl -s4 https://api.ipify.org || hostname -I | awk '{print $1}')
echo
echo "Rendezvous server is running."
echo "Server address for obsidian-probe: $IP:3478"
echo "Logs: journalctl -u obsidian-rendezvous -f"
