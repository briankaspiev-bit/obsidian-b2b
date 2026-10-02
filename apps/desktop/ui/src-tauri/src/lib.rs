//! Obsidian desktop: the Tauri shell around the booth UI (`apps/desktop/ui`).
//!
//! The UI talks to this process only through the commands below and the
//! `levels` event; `src/session/tauriEngine.ts` is the other half of the
//! contract (see `docs/desktop-bridge.md`).

pub mod devices;
pub mod nettest;

use std::net::{SocketAddr, UdpSocket};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use serde::Serialize;
use tauri::{Emitter, Manager, State};

/// What this build can do for real. The UI falls back to demo data for
/// anything reported missing.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EngineInfo {
    version: &'static str,
    devices: bool,
    network_test: bool,
    room_service: bool,
    live_audio: bool,
}

#[derive(Default)]
struct Booth {
    meter: Mutex<Option<devices::InputMeter>>,
    /// The socket and address the room service paired us with. The network
    /// test runs on it so it measures the same path the session will use.
    link: Mutex<Option<(UdpSocket, SocketAddr)>>,
}

#[derive(Clone, Serialize)]
struct Levels {
    local: (f32, f32),
}

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[tauri::command]
fn engine_info() -> EngineInfo {
    EngineInfo {
        version: env!("CARGO_PKG_VERSION"),
        devices: true,
        network_test: true,
        room_service: false,
        live_audio: false,
    }
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

#[tauri::command]
async fn run_network_test(seconds: f64, booth: State<'_, Booth>) -> CmdResult<nettest::NetworkResult> {
    let (sock, peer) = {
        let link = booth.link.lock().map_err(err)?;
        let (s, p) = link.as_ref().ok_or("Not connected to the other booth yet")?;
        (s.try_clone().map_err(err)?, *p)
    };
    let d = Duration::from_secs_f64(seconds.clamp(1.0, 30.0));
    tauri::async_runtime::spawn_blocking(move || nettest::run(&sock, peer, d))
        .await
        .map_err(err)?
        .map_err(err)
}

/// Pushes meter readings to the UI at 30 Hz while a meter is open.
fn spawn_level_pump(app: tauri::AppHandle) {
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
                let _ = app.emit("levels", Levels { local });
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
            run_network_test,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Obsidian");
}
