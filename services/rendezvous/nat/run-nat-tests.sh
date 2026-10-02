#!/usr/bin/env bash
# Simulates home routers with Linux network namespaces and checks that the
# rendezvous picks the right path for each NAT combination, end to end with
# the real server and obsidian-probe binaries.
#
#                    "internet" bridge 198.51.100.0/24
#        ┌───────────────┬──────────────┬──────────────┐
#   server .10        routerA .21     routerB .22
#                     10.1.0.0/24     10.2.0.0/24
#                    djA .2, djA2 .3   djB .2
#
# Router types:
#   cone       Linux masquerade: same public port for every destination,
#              replies only from addresses we sent to (port-restricted cone,
#              like most home routers).
#   symmetric  masquerade fully-random: a new public port per destination
#              (like many mobile carriers / CGNAT / some enterprise firewalls).
#
# Needs root, iproute2 and nftables. Usage: sudo nat/run-nat-tests.sh
set -euo pipefail

cd "$(dirname "$0")/.."
# CI builds as the normal user first (root has no rustup toolchain there).
[[ -n ${SKIP_BUILD:-} ]] || cargo build --quiet -p obsidian-rendezvous --bins
# A member of the repo-root workspace, so binaries land in the root target/.
BIN="$(cd ../.. && pwd)/target/debug"
WORK="$(mktemp -d)"
NS=(inet srv rtA rtB djA djA2 djB)
SERVER=198.51.100.10:3478
PROBE_SECONDS=${PROBE_SECONDS:-4}

cleanup() {
  for n in "${NS[@]}"; do ip netns del "obs-$n" 2>/dev/null || true; done
  [[ -n "${SERVER_PID:-}" ]] && kill "$SERVER_PID" 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT

x() { local n=$1; shift; ip netns exec "obs-$n" "$@"; }

# Connect namespace $1 (iface $2) to namespace $3 (iface $4).
link() {
  ip link add "$2" netns "obs-$1" type veth peer name "$4" netns "obs-$3"
  x "$1" ip link set "$2" up
  x "$3" ip link set "$4" up
}

setup() {
  for n in "${NS[@]}"; do
    ip netns del "obs-$n" 2>/dev/null || true
    ip netns add "obs-$n"
    x "$n" ip link set lo up
  done
  x inet ip link add br0 type bridge
  x inet ip link set br0 up
  for spec in srv:10 rtA:21 rtB:22; do
    local n=${spec%%:*} ip=${spec##*:}
    link "$n" wan0 inet "p-$n"
    x inet ip link set "p-$n" master br0
    x "$n" ip addr add "198.51.100.$ip/24" dev wan0
  done
  link djA eth0 rtA lanA
  link djA2 eth0 rtA lanA2
  link djB eth0 rtB lanB
  for r in rtA:1 rtB:2; do
    local n=${r%%:*} net=${r##*:}
    x "$n" ip link add br-lan type bridge
    x "$n" ip link set br-lan up
    x "$n" ip addr add "10.$net.0.1/24" dev br-lan
    x "$n" sysctl -qw net.ipv4.ip_forward=1
  done
  x rtA ip link set lanA master br-lan
  x rtA ip link set lanA2 master br-lan
  x rtB ip link set lanB master br-lan
  x djA ip addr add 10.1.0.2/24 dev eth0
  x djA2 ip addr add 10.1.0.3/24 dev eth0
  x djB ip addr add 10.2.0.2/24 dev eth0
  for h in djA djA2; do x "$h" ip route add default via 10.1.0.1; done
  x djB ip route add default via 10.2.0.1
}

# nat_mode ROUTER cone|symmetric
nat_mode() {
  local flag=""
  [[ $2 == symmetric ]] && flag="fully-random"
  x "$1" nft flush ruleset
  x "$1" nft -f - <<EOF
table ip nat {
  chain post {
    type nat hook postrouting priority srcnat;
    oifname "wan0" masquerade $flag
  }
}
# Like every home router: nothing on the internet side may open a connection
# to the router itself. Besides being realistic this matters for punching:
# without it, an early probe from the other DJ gets a conntrack entry here and
# forces our own outgoing mapping onto a different port.
table inet filter {
  chain input {
    type filter hook input priority filter; policy accept;
    iifname "wan0" ct state new drop
  }
}
EOF
  x "$1" conntrack -F 2>/dev/null || true
}

# Adds delay on each router's uplink; a DJ-to-DJ round trip crosses two uplinks.
delay() {
  for r in rtA rtB; do
    x "$r" tc qdisc del dev wan0 root 2>/dev/null || true
    [[ $1 != 0 ]] && x "$r" tc qdisc add dev wan0 root netem delay "${1}ms" "${2:-0}ms"
  done
  return 0
}

start_server() {
  # Not via x(): a backgrounded function is a subshell, and killing it would orphan the server.
  ip netns exec obs-srv "$BIN/rendezvous-server" --bind "$SERVER" --quiet &
  SERVER_PID=$!
  sleep 0.3
}

PASS=0
FAIL=0

# scenario NAME HOST_NS GUEST_NS EXPECT(DIRECT|RELAY)
scenario() {
  local name=$1 a=$2 b=$3 expect=$4
  local out_a="$WORK/$name.a" out_b="$WORK/$name.b"
  ip netns exec "obs-$a" "$BIN/obsidian-probe" create --server "$SERVER" --seconds "$PROBE_SECONDS" >"$out_a" 2>&1 &
  local pa=$!
  local code=""
  for _ in $(seq 50); do
    code=$(sed -n 's/^Room code: //p' "$out_a")
    [[ -n $code ]] && break
    sleep 0.1
  done
  if [[ -z $code ]]; then
    echo "FAIL $name: no room code"; cat "$out_a"; FAIL=$((FAIL + 1)); return
  fi
  x "$b" "$BIN/obsidian-probe" join "$code" --server "$SERVER" --seconds "$PROBE_SECONDS" >"$out_b" 2>&1 || true
  wait "$pa" || true
  local got_a got_b
  got_a=$(grep -o 'Connected [A-Z]*' "$out_a" | awk '{print $2}')
  got_b=$(grep -o 'Connected [A-Z]*' "$out_b" | awk '{print $2}')
  local rtt
  rtt=$(grep -o 'median [0-9.]* ms' "$out_b" | tail -1)
  if [[ $got_a == "$expect" && $got_b == "$expect" ]] && grep -q 'Connection report' "$out_b"; then
    echo "PASS $name: both $expect, round trip $rtt"
    PASS=$((PASS + 1))
  else
    echo "FAIL $name: expected $expect, host=$got_a guest=$got_b"
    echo "--- host"; cat "$out_a"; echo "--- guest"; cat "$out_b"
    FAIL=$((FAIL + 1))
  fi
}

setup
start_server

nat_mode rtA cone; nat_mode rtB cone; delay 0
scenario cone-to-cone djA djB DIRECT

nat_mode rtA cone; nat_mode rtB symmetric
scenario cone-to-symmetric djA djB RELAY

nat_mode rtA symmetric; nat_mode rtB symmetric
scenario symmetric-to-symmetric djA djB RELAY

# Both DJs behind one router that does not hairpin: the LAN candidate wins.
nat_mode rtA cone
scenario same-house djA djA2 DIRECT

if ! x rtA tc qdisc add dev wan0 root netem delay 1ms 2>/dev/null; then
  echo "SKIP delay scenarios: this kernel has no netem (sch_netem)"
  echo; echo "$PASS passed, $FAIL failed"
  [[ $FAIL == 0 ]]; exit
fi
x rtA tc qdisc del dev wan0 root

# Cross-country-like path: 2 x 20 ms = 40 ms round trip, with jitter.
nat_mode rtA cone; nat_mode rtB cone; delay 20 2
scenario cone-to-cone-40ms djA djB DIRECT
nat_mode rtB symmetric
scenario relay-40ms djA djB RELAY
delay 0

echo
echo "$PASS passed, $FAIL failed"
[[ $FAIL == 0 ]]
