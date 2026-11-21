// Keep a console for debug logs. Release builds run without a console.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod calc;
mod channel;
mod commands;
mod config;
mod config_write;
#[path = "dock/status_options.rs"]
mod status_options;

mod deferred;
mod dock;
mod drag;
mod ease;
mod editor;
mod files;
mod history;
mod hotkey;
mod indexer;
mod panic_log;
mod win_key;
// Settings also uses package-manager detection and update instructions.
#[allow(dead_code)]
mod install;
mod launch;
mod messages;
mod morph;
mod native_settings;
mod render;
mod screens;
mod search;
#[allow(dead_code)]
#[path = "../settings/src/config.rs"]
mod settings_config;
#[allow(dead_code)]
#[path = "../settings/src/system.rs"]
mod settings_system;
mod surface;
mod system;
mod taskbar;
mod theme;
mod tray;
mod tray_host;
mod update_notice;
mod util;

fn main() {
    panic_log::install();
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--settings-native-preview") {
        if let Err(e) = native_settings::preview(&args) {
            util::error_box(&e.message());
        }
        return;
    }
    if taskbar::dispatch_watchdog() {
        return;
    }
    if let Err(e) = app::run() {
        util::error_box(&format!("Wisp could not start:\n{}", e.message()));
    }
}
