//! obsidian-probe: a connection test two DJs can run before any audio exists.
//!
//! One person runs `obsidian-probe create` and reads out the room code; the
//! other runs `obsidian-probe join CODE`. Both laptops then exchange
//! audio-sized packets 100 times a second (the rate the engine will use) over
//! whatever path the rendezvous picked, and print round-trip time, jitter and
//! loss. Run with no arguments for an interactive prompt (double-click on
//! Windows).
//!
//! Server address comes from `--server`, then `OBSIDIAN_SERVER`, then the
//! address baked in at build time via `OBSIDIAN_DEFAULT_SERVER`, then the
//! project's own server ([`DEFAULT_SERVER`]).

use std::io::{self, BufRead, Write};
use std::net::{SocketAddr, ToSocketAddrs};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use obsidian_rendezvous::client::{create_room, join_room, ClientConfig, Connection, Path};
use obsidian_rendezvous::proto::format_code;

/// The project's rendezvous server (DigitalOcean, New York).
const DEFAULT_SERVER: &str = "204.48.26.46:3478";

const MAGIC: [u8; 2] = [0x0B, 0x50];
const PING: u8 = 1;
const PONG: u8 = 2;
const BYE: u8 = 3;
/// Roughly one 10 ms Opus frame at a high bitrate plus headers.
const PACKET_LEN: usize = 320;
const INTERVAL: Duration = Duration::from_millis(10);

enum Mode {
    Create,
    Join(String),
}

struct Args {
    mode: Option<Mode>,
    server: Option<String>,
    seconds: u64,
    force_relay: bool,
    interactive: bool,
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Some(a) => a,
        None => {
            eprintln!(
                "usage: obsidian-probe [create | join CODE] [--server HOST:PORT] [--seconds 30] [--relay]"
            );
            return ExitCode::FAILURE;
        }
    };
    let interactive = args.interactive;
    let code = run(args);
    if interactive {
        // Keep a double-clicked console window open long enough to read.
        println!("\nPress Enter to close.");
        let _ = io::stdin().lock().read_line(&mut String::new());
    }
    code
}

fn parse_args() -> Option<Args> {
    let mut args = Args {
        mode: None,
        server: std::env::var("OBSIDIAN_SERVER")
            .ok()
            .or(option_env!("OBSIDIAN_DEFAULT_SERVER").map(String::from))
            .or(Some(DEFAULT_SERVER.to_string())),
        seconds: 30,
        force_relay: false,
        interactive: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "create" => args.mode = Some(Mode::Create),
            "join" => args.mode = Some(Mode::Join(it.next()?)),
            "--server" => args.server = Some(it.next()?),
            "--seconds" => args.seconds = it.next()?.parse().ok()?,
            "--relay" => args.force_relay = true,
            _ => return None,
        }
    }
    if args.mode.is_none() {
        args.interactive = true;
    }
    Some(args)
}

fn prompt(q: &str) -> String {
    print!("{q}");
    let _ = io::stdout().flush();
    let mut s = String::new();
    let _ = io::stdin().lock().read_line(&mut s);
    s.trim().to_string()
}

fn run(mut args: Args) -> ExitCode {
    let server = match args.server.take() {
        Some(s) => s,
        None => prompt("Rendezvous server (host:port): "),
    };
    let mode = match args.mode.take() {
        Some(m) => m,
        None => match prompt("Type a room code to join, or press Enter to create a room: ") {
            c if c.is_empty() => Mode::Create,
            c => Mode::Join(c),
        },
    };
    let Some(server_addr) = resolve(&server) else {
        eprintln!("Could not resolve server address {server:?}");
        return ExitCode::FAILURE;
    };
    let name = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "DJ".into());
    let mut cfg = ClientConfig::new(server_addr);
    cfg.force_relay = args.force_relay;

    let conn = match mode {
        Mode::Create => {
            let room = match create_room(&cfg, &name) {
                Ok(r) => r,
                Err(e) => return fail(e),
            };
            println!("Room code: {}", format_code(room.code()));
            println!("Send this code to the other DJ. Waiting for them to join...");
            room.wait_for_guest(Duration::from_secs(15 * 60))
        }
        Mode::Join(code) => {
            println!("Joining {code}...");
            join_room(&cfg, &code, &name)
        }
    };
    let conn = match conn {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    let how = match conn.path {
        Path::Direct => format!("DIRECT to {}", conn.peer_addr),
        Path::Relay => format!("RELAY via {}", conn.peer_addr),
    };
    println!("Paired with {}.", conn.peer_name);
    println!(
        "Connected {how} (your public address: {})",
        conn.public_addr
    );
    println!("Measuring for {} seconds...\n", args.seconds);

    let report = match measure(&conn, Duration::from_secs(args.seconds)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Measurement failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    conn.leave();
    report.print(conn.path);
    ExitCode::SUCCESS
}

fn fail(e: obsidian_rendezvous::client::Error) -> ExitCode {
    eprintln!("Could not connect: {e}");
    ExitCode::FAILURE
}

fn resolve(s: &str) -> Option<SocketAddr> {
    let addrs: Vec<SocketAddr> = s.to_socket_addrs().ok()?.collect();
    // Prefer IPv4: home IPv6 often has stricter firewalls and no hole punching gain.
    addrs
        .iter()
        .find(|a| a.is_ipv4())
        .or(addrs.first())
        .copied()
}

#[derive(Default)]
struct Report {
    rtts_ms: Vec<f64>,
    /// Arrival minus sender timestamp for each ping received, in ms. The clock
    /// offset is unknown but constant, so the spread is the one-way jitter.
    transit_ms: Vec<f64>,
    counted_sent: u64,
    counted_answered: u64,
    peer_max_seq: Option<u32>,
    peer_first_seq: Option<u32>,
    peer_received: u64,
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let i = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[i]
}

impl Report {
    fn jitter_p99(&self) -> f64 {
        let min = self
            .transit_ms
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min);
        let mut spread: Vec<f64> = self.transit_ms.iter().map(|t| t - min).collect();
        spread.sort_by(|a, b| a.partial_cmp(b).unwrap());
        percentile(&spread, 0.99)
    }

    fn loss_out(&self) -> f64 {
        if self.counted_sent == 0 {
            return f64::NAN;
        }
        100.0 * (1.0 - self.counted_answered as f64 / self.counted_sent as f64)
    }

    fn loss_in(&self) -> f64 {
        match (self.peer_first_seq, self.peer_max_seq) {
            (Some(a), Some(b)) => {
                let expected = (b - a + 1) as f64;
                100.0 * (1.0 - self.peer_received as f64 / expected).max(0.0)
            }
            _ => f64::NAN,
        }
    }

    fn print(&self, path: Path) {
        let mut r = self.rtts_ms.clone();
        r.sort_by(|a, b| a.partial_cmp(b).unwrap());
        if r.is_empty() {
            println!("No replies came back from the other laptop. The path is not usable.");
            return;
        }
        let p50 = percentile(&r, 0.5);
        let jitter = self.jitter_p99();
        println!("===== Connection report =====");
        println!(
            "Path:              {}",
            if path == Path::Direct {
                "direct"
            } else {
                "relay"
            }
        );
        println!(
            "Round trip:        median {:.1} ms, p95 {:.1} ms, p99 {:.1} ms, worst {:.1} ms",
            p50,
            percentile(&r, 0.95),
            percentile(&r, 0.99),
            r[r.len() - 1]
        );
        println!(
            "One-way jitter:    p99 {:.1} ms (arrival-time wobble of the other side's packets)",
            jitter
        );
        println!(
            "Packet loss:       {:.2}% to them, {:.2}% from them",
            self.loss_out(),
            self.loss_in()
        );
        // Feasibility report F.4: booth delay = one-way p50 + jitter p99 + margin.
        // F.1: ~25 ms of non-network latency (capture, codec, buffers) on a typical setup.
        let network = p50 / 2.0 + jitter + 5.0;
        let total = network + 25.0;
        println!(
            "Estimated fixed delay you would hear the other DJ at: ~{:.0} ms",
            total
        );
        let verdict = if total <= 100.0 {
            "good for a B2B (within the 100 ms target)"
        } else if total <= 250.0 {
            "workable with beat-quantized monitoring"
        } else {
            "beyond the 250 ms ceiling, too far for comfortable B2B"
        };
        println!("Verdict (estimate): {verdict}");
    }
}

fn measure(conn: &Connection, duration: Duration) -> io::Result<Report> {
    let sock = &conn.socket;
    let start = Instant::now();
    sock.set_read_timeout(Some(Duration::from_millis(2)))?;
    let mut report = Report::default();
    let mut sent_at: Vec<Option<(Instant, bool)>> = Vec::new();
    let mut seq: u32 = 0;
    let mut next_send = Instant::now();
    let mut next_line = Instant::now() + Duration::from_secs(1);
    let mut first_rx: Option<Instant> = None;
    let mut peer_done = false;
    let mut buf = [0u8; 2048];
    let mut pkt = [0u8; PACKET_LEN];
    pkt[..2].copy_from_slice(&MAGIC);

    let us = |t: Instant| t.duration_since(start).as_micros() as u64;

    while start.elapsed() < duration && !peer_done {
        let now = Instant::now();
        if now >= next_send {
            pkt[2] = PING;
            pkt[3..7].copy_from_slice(&seq.to_be_bytes());
            pkt[7..15].copy_from_slice(&us(now).to_be_bytes());
            // Pings sent before we hear from the peer may land while it is
            // still finishing the handshake; don't count those as lost.
            let counted = first_rx.is_some();
            sent_at.push(Some((now, counted)));
            if counted {
                report.counted_sent += 1;
            }
            let _ = sock.send_to(&pkt, conn.peer_addr);
            seq += 1;
            next_send += INTERVAL;
        }
        match sock.recv_from(&mut buf) {
            Ok((n, _)) if n >= 15 && buf[..2] == MAGIC => {
                let arrived = Instant::now();
                first_rx.get_or_insert(arrived);
                let kind = buf[2];
                let s = u32::from_be_bytes(buf[3..7].try_into().unwrap());
                let t = u64::from_be_bytes(buf[7..15].try_into().unwrap());
                match kind {
                    PING => {
                        buf[2] = PONG;
                        let _ = sock.send_to(&buf[..n], conn.peer_addr);
                        report
                            .transit_ms
                            .push((us(arrived) as f64 - t as f64) / 1000.0);
                        report.peer_received += 1;
                        report.peer_first_seq = Some(report.peer_first_seq.map_or(s, |f| f.min(s)));
                        report.peer_max_seq = Some(report.peer_max_seq.map_or(s, |m| m.max(s)));
                    }
                    PONG => {
                        if let Some(Some((sent, counted))) =
                            sent_at.get_mut(s as usize).map(|e| e.take())
                        {
                            report
                                .rtts_ms
                                .push(arrived.duration_since(sent).as_secs_f64() * 1000.0);
                            if counted {
                                report.counted_answered += 1;
                            }
                        }
                    }
                    BYE => peer_done = true,
                    _ => {}
                }
            }
            Ok(_) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => {}
            Err(e) => return Err(e),
        }
        if Instant::now() >= next_line {
            next_line += Duration::from_secs(1);
            let recent: Vec<f64> = report.rtts_ms.iter().rev().take(100).copied().collect();
            let mut sorted = recent.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
            // Pings from the last second may still be in flight; only older
            // unanswered ones count as lost.
            let settled = Instant::now() - Duration::from_secs(1);
            let mut lost = 0u64;
            for (sent, counted) in sent_at.iter().flatten() {
                if *counted && *sent <= settled {
                    lost += 1;
                }
            }
            let old = report.counted_answered + lost;
            let live_loss = if old == 0 {
                0.0
            } else {
                100.0 * lost as f64 / old as f64
            };
            println!(
                "t={:>3}s  rtt median {:>6.1} ms  p99 {:>6.1} ms  loss {:>5.2}%  jitter p99 {:>5.1} ms",
                start.elapsed().as_secs(),
                percentile(&sorted, 0.5),
                percentile(&sorted, 0.99),
                live_loss,
                report.jitter_p99()
            );
        }
    }
    // Pings still in flight at the end are not losses.
    let grace = Instant::now() - Duration::from_millis(1000);
    for (sent, counted) in sent_at.iter().flatten() {
        if *counted && *sent > grace {
            report.counted_sent -= 1;
        }
    }
    pkt[2] = BYE;
    for _ in 0..5 {
        let _ = sock.send_to(&pkt, conn.peer_addr);
    }
    Ok(report)
}
