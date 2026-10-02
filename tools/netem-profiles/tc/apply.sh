#!/usr/bin/env bash
# Apply a lab profile with Linux tc netem on a router box between two machines.
# Usage: sudo ./apply.sh <iface> <profile>     (run on BOTH directions' egress interfaces)
#        sudo ./apply.sh <iface> clear
# Mirrors tools/netem-profiles/profiles.json for real-hardware tests. netem's loss
# models differ slightly from obsidian-netem's (gemodel params are percentages).
set -euo pipefail
IF=${1:?iface}; P=${2:?profile}
tc qdisc del dev "$IF" root 2>/dev/null || true
case "$P" in
  clear) exit 0 ;;
  same-city)    tc qdisc add dev "$IF" root netem delay 8ms 1ms distribution normal ;;
  nyc-lon)      tc qdisc add dev "$IF" root netem delay 38ms 4ms distribution normal loss 0.3% ;;
  nyc-tyo)      tc qdisc add dev "$IF" root netem delay 90ms 8ms distribution normal loss 0.5% ;;
  bad-wifi)     tc qdisc add dev "$IF" root netem delay 40ms 5ms distribution normal loss gemodel 0.4% 20% 100% 0% ;;
  route-change) tc qdisc add dev "$IF" root netem delay 38ms 4ms distribution normal loss 0.3%
                ( sleep 50; tc qdisc change dev "$IF" root netem delay 60ms 4ms distribution normal loss 0.3% ) & ;;
  *) echo "unknown profile $P" >&2; exit 1 ;;
esac
# Note: netem with jitter reorders packets unless rate-limited; add "rate 1gbit" to keep FIFO.
tc qdisc show dev "$IF"
