# Putting the rendezvous server online

Any small Linux VM with a public IPv4 address works. The server uses almost no
CPU or memory; traffic only matters when sessions fall back to the relay
(roughly 100–700 MB per relayed 30-minute set, depending on audio quality).

## DigitalOcean (about $4–6 a month)

1. Sign up at digitalocean.com and create a **Droplet**: Ubuntu 24.04,
   Basic, Regular, the cheapest size, the region closest to the midpoint
   between the two DJs. Pick "Password" for authentication if you don't use
   SSH keys.
2. Open the droplet and click **Console** (a terminal in the browser; no SSH
   needed on Windows).
3. Paste:
   ```sh
   curl -sSf https://raw.githubusercontent.com/briankaspiev-bit/obsidian-b2b/main/services/rendezvous/deploy/install.sh | bash
   ```
   (Until PR #4 merges, put `BRANCH=claude/project-thread-qdtv1w` before
   `bash` and use that branch name in the URL instead of `main`.)
4. It ends by printing the server address, e.g. `203.0.113.5:3478`. That is
   what both DJs type into `obsidian-probe`.

DigitalOcean droplets have no firewall by default, so UDP 3478 is reachable
right away. If you add a Cloud Firewall, allow inbound **UDP 3478**.

## Hetzner, Oracle free tier, or anything else

Same script on any Ubuntu VM. On Oracle Cloud, also add an ingress rule for
UDP 3478 to the subnet's security list and run
`iptables -I INPUT -p udp --dport 3478 -j ACCEPT` (Oracle images block it in
the OS firewall too).

## Checking it works

From any computer: `obsidian-probe create --server IP:3478` should print a room
code within a second. On the VM, `journalctl -u obsidian-rendezvous -f` shows a
stats line every minute.

## Turning it off

Destroy the droplet in the dashboard. Billing stops; nothing else depends on it.
