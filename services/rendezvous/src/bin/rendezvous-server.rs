//! `rendezvous-server [--bind ADDR] [--quiet]`
//!
//! Runs the room/hole-punch/relay service on one UDP port (default 0.0.0.0:3478).

use std::net::UdpSocket;
use std::process::ExitCode;

use obsidian_rendezvous::server::{run, ServerConfig};

fn main() -> ExitCode {
    let mut bind = "0.0.0.0:3478".to_string();
    let mut log = true;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--bind" => match args.next() {
                Some(v) => bind = v,
                None => return usage(),
            },
            "--quiet" => log = false,
            _ => return usage(),
        }
    }
    let socket = match UdpSocket::bind(&bind) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot bind {bind}: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!(
        "obsidian rendezvous listening on udp {}",
        socket.local_addr().unwrap()
    );
    match run(socket, ServerConfig::default(), log) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("server stopped: {e}");
            ExitCode::FAILURE
        }
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: rendezvous-server [--bind 0.0.0.0:3478] [--quiet]");
    ExitCode::FAILURE
}
