// Keep a console for debug logs. Release builds run without a console.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod calc;
mod commands;
mod config;
mod editor;
mod files;
mod history;
mod hotkey;
mod indexer;
mod launch;
mod render;
mod search;
mod system;
mod theme;
mod tray;
mod util;

fn main() {
    if let Err(e) = app::run() {
        util::error_box(&format!("Wisp could not start:\n{}", e.message()));
    }
}
