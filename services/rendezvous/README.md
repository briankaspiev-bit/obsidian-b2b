# Rendezvous: how two DJ laptops find each other

One small Rust service plus a client library. A DJ creates a room and gets a
eight-character code (`KQFD-W2X7`), the other DJ types it in, and both apps end up
holding a UDP socket and an address to send audio to: the other laptop when a
direct path works, or this server's relay when a home router blocks it.

Status labels follow the feasibility report: **[PROVEN]** tested here,
**[PLAUSIBLE]** expected but not yet measured on the real internet.

## Pieces

| Path | What it is |
|---|---|
| `src/server.rs`, `rendezvous-server` | Rooms, public-address discovery, path decision, blind relay. One UDP port, in-memory state, std only. |
| `src/client.rs` | `create_room` / `join_room` → `Connection { socket, peer_addr, path, .. }`. Blocking, std only, Windows/macOS/Linux. |
| `src/proto.rs` | Control packet format (`0x0B 0x52` magic). |
| `obsidian-probe` | Connection test two people can run today, before any audio exists. |
| `nat/run-nat-tests.sh` | Simulated home routers with Linux network namespaces. |

## How a connection is made

1. **Create / join.** The host's app sends `Create` with its display name; the server answers with a
   room code, a secret member token, and the public address it saw the packet
   come from (what STUN does). The guest sends `Join` with the code.
2. **Exchange addresses.** Both poll the server until it hands each the other's
   public address and LAN address. Polling also keeps the router mapping to the
   server alive.
3. **Punch.** Both spray small probes at the other's public and LAN addresses
   every 20 ms for up to 3 s. A path only counts once a probe is *acknowledged*,
   which proves both directions work. Replies go to wherever a probe actually
   came from, so routers that rewrite ports still work when they allow it.
4. **Agree.** Each side reports to the server whether direct worked. Both
   direct → direct. Either failed → both use the relay. The server decides so
   the two laptops can never pick different paths.
5. **Relay (fallback).** The server forwards any packet that does not start
   with its own magic, from one member's address to the other's, unchanged. The
   engine just sends to the server's address instead of the peer's; no headers,
   no extra copy.

Everything runs on the one socket the engine later uses for media, so the
address the server observed, the punched mapping and the relay registration
all belong to it.

## Using it from the engine

```rust
use obsidian_rendezvous::client::{create_room, join_room, ClientConfig};
use obsidian_rendezvous::proto::format_code;

let cfg = ClientConfig::new(server_addr);
// Host:
let room = create_room(&cfg, "Val")?;
show_code(&format_code(room.code())); // "KQFD-W2X7"
let link = room.wait_for_guest(Duration::from_secs(15 * 60))?;
// Guest: let link = join_room(&cfg, &typed_code, "Dana")?; // dash and case optional
// link.peer_name is the other DJ's name.

let report = obsidian_engine::run_peer_with_socket(
    PeerConfig { peer: link.peer_addr, bind: link.socket.local_addr()?, ..cfg },
    link.socket,
)?;
```

The engine must ignore packets that do not start with its own magic `0x0B5D`
(it already does): late punch probes (`0x0B52`) can still arrive just after the
handoff. `link.is_host` is a stable tiebreak for who leads first;
`link.session` is identical on both sides.

## Try it

```sh
cargo run --bin rendezvous-server                     # udp 0.0.0.0:3478
cargo run --bin obsidian-probe -- create --server 127.0.0.1:3478
cargo run --bin obsidian-probe -- join ABCD-2345 --server 127.0.0.1:3478
```

`obsidian-probe` with no arguments asks for the server and a code, so a
double-clicked Windows `.exe` works. Build with
`OBSIDIAN_DEFAULT_SERVER=host:3478 cargo build --release` to bake the server in.
It sends 320-byte packets 100 times a second (the engine's rate) for 30 s and
prints round trip, one-way jitter, loss each way, and the fixed delay the
other DJ would be heard at (feasibility report F.4: one-way p50 + jitter p99 +
margin, plus ~25 ms of non-network latency from F.1).

## Tests

```sh
cargo test                       # protocol, server logic, loopback end-to-end
sudo nat/run-nat-tests.sh        # simulated routers (Linux, root, nftables)
```

Simulated results **[PROVEN in simulation]**:

| Scenario | Path |
|---|---|
| Both behind typical home routers (port-restricted cone) | direct |
| One home router, one symmetric NAT (mobile / CGNAT style) | relay |
| Both symmetric | relay |
| Both DJs in the same house, router without hairpinning | direct (LAN address) |

The script also has 40 ms round-trip scenarios that need the kernel's `netem`
module; they skip where it is missing.

One finding worth keeping: a Linux router whose firewall *accepts* new
connections to itself from the internet breaks punching. An early probe from
the other DJ creates a conntrack entry that forces our outgoing mapping onto a
different port. Real routers drop such packets, so the simulation does too;
if field tests show unexpected relay fallbacks on Linux-based routers, look
here first.

## Not done yet

- **Real-internet numbers.** **[PLAUSIBLE]** The direct-vs-relay split on real
  home networks (the report expects 10–20% relayed) needs `obsidian-probe` runs
  between real houses.
- **Encryption.** Packets are not encrypted. The relay never needs to read
  media, so the engine can add end-to-end encryption on top without changing
  this service. `link.session` can feed key derivation, but on its own it is
  visible to the server, so it is not a secret.
- **Picking the faster path.** The report suggests probing direct and relay and
  keeping the lower p99. Today direct always wins when it works.
- **Reconnecting** after a network change mid-set, IPv6 candidates beyond the
  primary interface, multiple relay regions, and auth on room creation (only
  per-IP rate limiting today).
- **Hosting.** Needs one small VM with a public IPv4 and UDP port 3478 open; see [deploy/DEPLOY.md](deploy/DEPLOY.md).
