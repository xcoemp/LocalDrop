//! # `build.rs` — Cargo build script
//!
//! Runs `tauri_build::build()`, which generates the context the `tauri::Builder`
//! consumes at runtime: the parsed `tauri.conf.json`, the embedded frontend
//! assets, the capability/permission set from `capabilities/`, and — on Windows
//! — the resource file carrying the icon and version metadata.
//!
//! There is no conditional logic here on purpose. Anything platform-specific is
//! decided by `tauri_build` from the target triple, so adding branches would
//! duplicate decisions it already makes correctly.
//!
//! Project: LocalDrop — zero-configuration LAN file and text transfer
//! Author:  Emmanuel Paul <pauldukz@gmail.com>

fn main() {
    tauri_build::build()
}
