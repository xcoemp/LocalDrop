//! # `lib.rs` — LocalDrop library root
//!
//! Entry point: wires plugins, spawns the discovery and transfer loops, and
//! hands the frontend a command surface (PRD §6.1).
//!
//! The crate is a library rather than a plain binary so the same code can be
//! built as a `cdylib` for Android, where the JVM loads it and there is no
//! `main` — see `main.rs`.
//!
//! Project: LocalDrop — zero-configuration LAN file and text transfer
//! Author:  Emmanuel Paul <pauldukz@gmail.com>

// Compile-time selection between the real JNI bridge and the no-op stubs
// below. This is the cfg the warning in the stub module's docs refers to: on
// desktop, `android.rs` is not merely inert, it is never compiled.
#[cfg(target_os = "android")]
mod android;

/// Desktop stubs for the JNI lookups.
///
/// Every function here answers the way a desktop OS behaves, so callers need no
/// platform branches of their own — `state.rs` calls `set_keep_screen_on`
/// unconditionally, for instance, and it simply does nothing here.
///
/// Note for anyone editing `android.rs`: on desktop the real module is not
/// compiled at all, so `cargo test` here proves nothing about it. Use
/// `pnpm cargo:android`.
#[cfg(not(target_os = "android"))]
mod android {
    pub fn display_name(_uri: &str) -> Option<String> {
        None
    }

    pub fn device_name() -> Option<String> {
        None
    }

    /// Desktop displays sleeping does not suspend the process, so transfers
    /// survive it and there is nothing to hold.
    pub fn set_keep_screen_on(_on: bool) {}

    /// Desktop processes are not suspended when unfocused, so no equivalent of
    /// the Android foreground service is needed.
    pub fn start_background_service(_detail: Option<&str>) {}

    pub fn stop_background_service() {}

    /// Nothing throttles a desktop process this way, so it is always "exempt".
    pub fn is_ignoring_battery_optimizations() -> bool {
        true
    }

    pub fn request_ignore_battery_optimizations() {}
}

mod commands;
mod discovery;
mod error;
mod history;
mod paths;
mod protocol;
mod registry;
mod settings;
mod state;
mod transfer;

use std::sync::Arc;

use tauri::Manager;

use crate::settings::Store;
use crate::state::AppState;

/// Start LocalDrop: install plugins, spawn the discovery and transfer loops,
/// and hand the webview its command surface.
///
/// # Panics
///
/// If the Tauri runtime cannot start, which is unrecoverable.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Honour `RUST_LOG` when set, otherwise this crate at info and everything
    // else at warn. Without the fallback, a release build would log nothing and
    // the Android startup trace in docs/BUILDING.md would be empty.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "localdrop_lib=info,warn".into()),
        )
        .init();

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init());

    // A second instance would fight the first for ports 57321/57322; focus the
    // existing window instead.
    //
    // Desktop only, and shadowing `builder` rather than branching around it:
    // the plugin has no mobile counterpart, and an Android app cannot be
    // launched twice anyway. The callback fires in the *first* instance.
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
        // All three calls are needed and none can be assumed: the window may be
        // hidden in the tray, minimised, or merely unfocused. Errors are
        // ignored because failing to raise a window must not kill the process.
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    }));

    builder
        .invoke_handler(tauri::generate_handler![
            commands::get_device_info,
            commands::get_peers,
            commands::rescan,
            commands::get_network,
            commands::get_settings,
            commands::update_settings,
            commands::factory_reset,
            commands::send_files,
            commands::send_text,
            commands::respond_to_offer,
            commands::cancel_transfer,
            commands::get_active_transfers,
            commands::get_history,
            commands::clear_history,
            commands::path_exists,
            commands::ensure_download_dir,
            commands::set_download_directory,
            commands::list_directory,
            commands::start_background_listener,
            commands::stop_background_listener,
            commands::get_background_status,
            commands::request_battery_exemption,
        ])
        .setup(|app| {
            // Falling back to a temp directory rather than failing: settings
            // that do not survive a restart are a degraded experience, while
            // refusing to launch over an unresolvable config path is a dead app.
            let config_dir = app
                .path()
                .app_config_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("localdrop"));

            let state = Arc::new(AppState::new(Store::new(config_dir)));
            app.manage(state.clone());

            let handle = app.handle().clone();

            // §6.6 — sweep `.part` files orphaned by a previous crash.
            //
            // Spawned rather than awaited so a large download directory cannot
            // delay the window appearing (§9's cold-start budget). Scoped block
            // so the cloned `state` is moved into the task and dropped there.
            {
                let state = state.clone();
                tauri::async_runtime::spawn(async move {
                    let dir = state.settings.read().await.download_path();
                    // Created up front so the first received file does not have
                    // to; the error is ignored because `ensure_download_dir`
                    // reports it properly when the user actually needs it.
                    let _ = tokio::fs::create_dir_all(&dir).await;
                    transfer::cleanup_orphans(dir).await;
                });
            }

            // The two long-lived loops. Both own a clone of the handle and the
            // state and run for the life of the process.
            tauri::async_runtime::spawn(discovery::run(handle.clone(), state.clone()));
            tauri::async_runtime::spawn(transfer::run_listener(handle.clone(), state.clone()));

            #[cfg(desktop)]
            {
                setup_tray(app)?;

                // FR-6.3 — "Launch Minimized to Tray".
                //
                // `block_on` is acceptable only because this is startup, before
                // any loop is contending for the lock, and the window must be
                // hidden before it is first shown — deferring it to a task
                // would let the window flash on screen first.
                let launch_minimized =
                    tauri::async_runtime::block_on(state.settings.read()).launch_minimized;
                if launch_minimized {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.hide();
                    }
                }
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window parks the app in the tray; transfers in flight
            // keep running. Quit is explicit, from the tray menu.
            //
            // `prevent_close` is what makes this a hide rather than an exit —
            // without it the process would die mid-transfer and leave `.part`
            // files for the next launch's sweep to find.
            #[cfg(desktop)]
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
            // Mobile has no window chrome to close, and both parameters would
            // otherwise be unused there — which is a warning, and the build
            // runs warning-free.
            #[cfg(not(desktop))]
            let _ = (window, event);
        })
        .run(tauri::generate_context!())
        .expect("error while running LocalDrop");
}

/// §7.1 — minimize-to-tray with an explicit quit path.
#[cfg(desktop)]
fn setup_tray(app: &mut tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::TrayIconBuilder;

    let show = MenuItem::with_id(app, "show", "Show LocalDrop", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit LocalDrop", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    let mut tray = TrayIconBuilder::with_id("localdrop")
        .tooltip("LocalDrop")
        .menu(&menu)
        // Selection over the menu item id. The `_ => {}` arm is required
        // because ids are strings, not an enum — there is nothing for the
        // compiler to check exhaustively here.
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "quit" => {
                // Cancel in-flight transfers so `.part` files are cleaned up
                // rather than left behind (§6.6).
                //
                // `block_on` is correct despite being a blocking call on the
                // menu thread: the process is about to exit, and letting
                // `app.exit` run first would kill the cleanup mid-way. `try_state`
                // rather than `state` because quitting must work even if setup
                // never completed.
                if let Some(state) = app.try_state::<Arc<AppState>>() {
                    let state = state.inner().clone();
                    tauri::async_runtime::block_on(state.cancel_all());
                }
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // Narrow pattern: a left *release*, not any click event. Matching
            // the press as well would fire twice, and matching any button would
            // steal the right-click that opens the menu.
            if let tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = event
            {
                if let Some(window) = tray.app_handle().get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
        });

    // The tray is built with or without an icon: a missing icon yields a
    // blank-but-functional tray entry, whereas treating it as fatal would mean
    // no quit path at all. In practice the bundled icon is always present.
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}
