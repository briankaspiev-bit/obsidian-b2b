# Putting the rendezvous server online

Any small Linux VM with a public IPv4 address works. The server uses almost no
CPU or memory; traffic only matters when sessions fall back to the relay
(roughly 100–700 MB per relayed 30-minute set, depending on audio quality).

## DigitalOcean, click by click (about $6 a month)

You never need to send anyone a password. The only thing to share at the end
is the server's IP address.

1. Go to **cloud.digitalocean.com** and sign in (or sign up and add a card).
2. Top right, click the green **Create** button, then **Droplets**.
3. **Choose Region:** click **New York**. Any datacenter number is fine.
4. **Choose an image:** the **OS** tab, **Ubuntu**, version **24.04 (LTS) x64**.
5. **Choose Size:** **Basic**, then **Regular** under CPU options, then the
   **$6/mo** box (1 GB memory). The $4 box usually works too, but the $6 one
   has room to build the server without running out of memory.
6. **Choose Authentication Method:** **Password**. Make up a strong root
   password and keep it somewhere safe. Don't send it to anyone; nobody needs it.
7. Scroll to **Advanced Options** and click it. Tick **Add Initialization
   scripts (free)**. A text box appears.
8. Paste the entire contents of
   [`cloud-init.sh`](cloud-init.sh) into that box, from `#!/bin/bash` to the
   last `fi`.
9. **Hostname:** type `obsidian-rendezvous`.
10. Click **Create Droplet** at the bottom.
11. Wait for the progress bar to finish, then **wait about 5 more minutes**
    while it installs itself.
12. Copy the Droplet's **ipv4** address (shown next to its name, like
    `203.0.113.5`) and send just that. The server address is that IP followed
    by `:3478`.

**To check it worked:** click the Droplet's name, then **Console** (top right).
A black terminal opens in your browser. The banner at the top should say
`Obsidian rendezvous: RUNNING` and the server address. If it says "still
installing", close the console and open it again in a few minutes. If it says
"INSTALL FAILED", type `tail -50 /var/log/cloud-init-output.log`, press Enter,
and send a screenshot.

DigitalOcean droplets have no firewall by default, so UDP 3478 is reachable
right away. If you ever add a Cloud Firewall, allow inbound **UDP 3478**.

**Updating later:** open the Console and paste
`curl -sSf https://raw.githubusercontent.com/briankaspiev-bit/obsidian-b2b/main/services/rendezvous/deploy/install.sh | bash`
(after PR #4 merges; before that, see the branch note under "Any Ubuntu VM").

## Any Ubuntu VM (manual)

Run as root:
```sh
curl -sSf https://raw.githubusercontent.com/briankaspiev-bit/obsidian-b2b/main/services/rendezvous/deploy/install.sh | bash
```
Until PR #4 merges, use `claude/project-thread-qdtv1w` instead of `main` in the
URL and put `BRANCH=claude/project-thread-qdtv1w` before `bash`. It prints the
server address when done.

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
