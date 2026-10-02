fn main() {
    // The room server can be baked in at build time (see lib.rs, room_server).
    println!("cargo:rerun-if-env-changed=OBSIDIAN_DEFAULT_SERVER");
    tauri_build::build()
}
