#!/bin/bash
# Paste this whole file into DigitalOcean's "Add Initialization scripts" box
# (or any provider's "user data" field) when creating an Ubuntu VM. It
# installs and starts the Obsidian rendezvous server on UDP 3478, then writes
# the result into the login banner so anyone opening the console sees it.
# Progress log: /var/log/cloud-init-output.log
export HOME=/root
BRANCH=claude/project-thread-qdtv1w
echo "Obsidian rendezvous: still installing, check again in a few minutes." >/etc/motd
if curl -sSf "https://raw.githubusercontent.com/briankaspiev-bit/obsidian-b2b/$BRANCH/services/rendezvous/deploy/install.sh" \
    | BRANCH=$BRANCH bash; then
  IP=$(curl -s4 https://api.ipify.org)
  printf '\nObsidian rendezvous: RUNNING\nServer address: %s:3478\n\n' "$IP" >/etc/motd
else
  printf '\nObsidian rendezvous: INSTALL FAILED\nSee /var/log/cloud-init-output.log\n\n' >/etc/motd
fi
