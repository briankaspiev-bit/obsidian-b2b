//! Obsidian desktop: the Tauri shell around the booth UI (`apps/desktop/ui`).
//!
//! The UI talks to this process only through the commands and events below;
//! `src/session/tauriEngine.ts` is the other half of the contract (see
//! `apps/desktop/ui/BRIDGE.md`).
//!
//! What is real today: audio device lists and the input meter (cpal/WASAPI),
//! room codes and pairing (services/rendezvous), and the booth link (round
//! trip, jitter, loss, coordination messages and levels between the two
//! apps). Not yet: live audio between the booths, which waits for the
//! engine's device mode (crates/engine).

pub mod devices;
pub mod link;
pub mod nettest;

use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use obsidian_rendezvous::client::{self as rendezvous, ClientConfig, Connection, Path};
use obsidian_rendezvous::proto::format_code;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use link::{BoothLink, LinkSink, LinkStatus};

/// How long a created room waits for the second DJ.
const HOST_WAIT: Duration = Duration::from_secs(4 * 60 * 60);

/// What this build can do for real. The UI uses demo data for anything
/// reported missing.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EngineInfo {
    version: &'static str,
    /// The rendezvous server this build talks to, if one is configured.
    room_server: Option<String>,
    live_audio: bool,
}

#[derive(Default)]
struct Booth {
    meter: Mutex<Option<devices::InputMeter>>,
    link: Mutex<Option<BoothLink>>,
    /// Kept to tell the server we left.
    conn: Mutex<Option<Connection>>,
    /// Bumped on leave, so a host still waiting for a guest ignores a late join.
    generation: AtomicU64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Paired {
    code: String,
    peer_name: String,
    is_host: bool,
    relay: bool,
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum RoomEvent {
    Paired(Paired),
    Error { message: String },
}

#[derive(Clone, Serialize)]
struct Level {
    left: f32,
    right: f32,
}

impl From<(f32, f32)> for Level {
    fn from((left, right): (f32, f32)) -> Self {
        // JSON has no -inf; the UI treats null as silence.
        Level { left, right }
    }
}

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// `OBSIDIAN_SERVER` at run time wins over the address baked in at build
/// time (`OBSIDIAN_DEFAULT_SERVER`), so a tester can point a build anywhere.
fn room_server() -> Option<String> {
    std::env::var("OBSIDIAN_SERVER")
        .ok()
        .or_else(|| option_env!("OBSIDIAN_DEFAULT_SERVER").map(str::to_owned))
        .filter(|s| !s.trim().is_empty())
}

fn client_config() -> CmdResult<ClientConfig> {
    let server = room_server().ok_or("This build has no room server set up yet.")?;
    let addr: SocketAddr = server
        .to_socket_addrs()
        .map_err(|_| "Can't find the room server. Check your internet connection.".to_string())?
        .next()
        .ok_or("Can't find the room server.")?;
    Ok(ClientConfig::new(addr))
}

/// Plain-language versions of the rendezvous errors, for the Home screen.
fn friendly(e: rendezvous::Error) -> String {
    use rendezvous::Error::*;
    match e {
        RoomNotFound => "No room with that code. It may have closed. Ask for a new one.".into(),
        RoomFull => "That room already has two DJs.".into(),
        RateLimited => "Too many rooms from this network. Try again in a minute.".into(),
        ServerUnreachable => "Can't reach the room server. Check your internet connection.".into(),
        Timeout => "The other booth didn't answer in time. Try again.".into(),
        Io(e) => format!("Network error: {e}"),
        Protocol(m) => format!("Something went wrong connecting ({m})."),
    }
}

struct TauriSink(AppHandle);

impl LinkSink for TauriSink {
    fn control(&self, json: String) {
        let _ = self.0.emit("peer-control", json);
    }
    fn status(&self, status: LinkStatus) {
        let _ = self.0.emit("link-status", status);
    }
    fn remote_level(&self, level: (f32, f32)) {
        let _ = self.0.emit("remote-level", Level::from(level));
    }
}

/// Hands the paired socket to the booth link and remembers the connection.
fn connect(app: &AppHandle, booth: &Booth, conn: Connection, code: &str) -> CmdResult<Paired> {
    let relay = conn.path == Path::Relay;
    let sock = conn.socket.try_clone().map_err(err)?;
    let link = BoothLink::start(sock, conn.peer_addr, relay, TauriSink(app.clone())).map_err(err)?;
    let paired = Paired {
        code: format_code(code),
        peer_name: conn.peer_name.clone(),
        is_host: conn.is_host,
        relay,
    };
    *booth.link.lock().map_err(err)? = Some(link);
    *booth.conn.lock().map_err(err)? = Some(conn);
    Ok(paired)
}

#[tauri::command]
fn engine_info() -> EngineInfo {
    EngineInfo { version: env!("CARGO_PKG_VERSION"), room_server: room_server(), live_audio: false }
}

#[tauri::command]
async fn list_devices() -> devices::DeviceList {
    // Enumeration can take a moment on Windows; async keeps the UI thread free.
    devices::list()
}

#[tauri::command]
async fn start_input_meter(id: String, booth: State<'_, Booth>) -> CmdResult<()> {
    let mut slot = booth.meter.lock().map_err(err)?;
    *slot = None; // close the old device first
    *slot = Some(devices::meter_input(&id).map_err(err)?);
    Ok(())
}

#[tauri::command]
fn stop_input_meter(booth: State<'_, Booth>) {
    if let Ok(mut slot) = booth.meter.lock() {
        *slot = None;
    }
}

/// Creates a room and returns its code at once; `room-event` says when the
/// other DJ arrives.
#[tauri::command]
async fn create_room(name: String, app: AppHandle) -> CmdResult<String> {
    let cfg = client_config()?;
    let generation = app.state::<Booth>().generation.load(Ordering::SeqCst);
    let room = tauri::async_runtime::spawn_blocking(move || rendezvous::create_room(&cfg, &name))
        .await
        .map_err(err)?
        .map_err(friendly)?;
    let code = room.code().to_owned();
    let shown = format_code(&code);
    thread::Builder::new()
        .name("wait-for-guest".into())
        .spawn(move || {
            let result = room.wait_for_guest(HOST_WAIT);
            let booth = app.state::<Booth>();
            if booth.generation.load(Ordering::SeqCst) != generation {
                if let Ok(c) = result {
                    c.leave(); // we left before they arrived
                }
                return;
            }
            let event = match result.map_err(friendly).and_then(|c| connect(&app, &booth, c, &code)) {
                Ok(p) => RoomEvent::Paired(p),
                Err(message) => RoomEvent::Error { message },
            };
            let _ = app.emit("room-event", event);
        })
        .map_err(err)?;
    Ok(shown)
}

#[tauri::command]
async fn join_room(code: String, name: String, app: AppHandle) -> CmdResult<Paired> {
    let cfg = client_config()?;
    let c = code.clone();
    let conn = tauri::async_runtime::spawn_blocking(move || rendezvous::join_room(&cfg, &c, &name))
        .await
        .map_err(err)?
        .map_err(friendly)?;
    connect(&app, &app.state::<Booth>(), conn, &code)
}

#[tauri::command]
fn leave_room(booth: State<'_, Booth>) {
    booth.generation.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut l) = booth.link.lock() {
        *l = None; // says goodbye to the other booth
    }
    if let Ok(mut c) = booth.conn.lock() {
        if let Some(c) = c.take() {
            c.leave();
        }
    }
}

#[tauri::command]
fn send_control(json: String, booth: State<'_, Booth>) -> CmdResult<()> {
    let link = booth.link.lock().map_err(err)?;
    link.as_ref().ok_or("Not connected to the other booth")?.send_control(json)
}

#[tauri::command]
async fn run_network_test(seconds: f64, booth: State<'_, Booth>) -> CmdResult<nettest::NetworkResult> {
    let d = Duration::from_secs_f64(seconds.clamp(1.0, 30.0));
    let tester = {
        let link = booth.link.lock().map_err(err)?;
        link.as_ref().ok_or("Not connected to the other booth")?.tester()
    };
    tauri::async_runtime::spawn_blocking(move || tester.run(d))
        .await
        .map_err(err)?
        .ok_or_else(|| "The link closed during the test".to_string())
}

/// Sends meter readings to the UI (and to the other booth) while a meter is open.
fn spawn_level_pump(app: AppHandle) {
    thread::Builder::new()
        .name("level-pump".into())
        .spawn(move || loop {
            thread::sleep(Duration::from_millis(33));
            let booth = app.state::<Booth>();
            let local = match booth.meter.lock() {
                Ok(m) => m.as_ref().map(|m| m.peaks.take_dbfs()),
                Err(_) => None,
            };
            if let Some(local) = local {
                let _ = app.emit("local-level", Level::from(local));
                if let Ok(link) = booth.link.lock() {
                    if let Some(link) = link.as_ref() {
                        link.set_local_level(local);
                    }
                }
            }
        })
        .expect("spawn level pump");
}

pub fn run() {
    tauri::Builder::default()
        .manage(Booth::default())
        .setup(|app| {
            spawn_level_pump(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            engine_info,
            list_devices,
            start_input_meter,
            stop_input_meter,
            create_room,
            join_room,
            leave_room,
            send_control,
            run_network_test,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Obsidian");
}
