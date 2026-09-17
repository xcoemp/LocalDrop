//! # `main.rs` — binary entry point
//!
//! Thin wrapper around [`localdrop_lib::run`]. The real work lives in the
//! library crate so that the same code can be built as a `cdylib` for Android,
//! where there is no `main` at all — the JVM loads `liblocaldrop_lib.so` and
//! calls in through JNI instead.
//!
//! Project: LocalDrop — zero-configuration LAN file and text transfer
//! Author:  Emmanuel Paul <pauldukz@gmail.com>

// Compile-time selection, not a runtime branch: in any build without debug
// assertions (i.e. release), link as a Windows GUI subsystem binary so that
// launching the app does not also open a console window behind it (PRD §7.1).
// Debug builds keep the console, because that is where `tracing` output goes.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    localdrop_lib::run()
}
