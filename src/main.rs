// Release builds are GUI apps (no console window); debug builds keep the console for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod calc;
mod config;
mod editor;
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
        util::error_box(&format!("Wisp failed to start:\n{}", e.message()));
    }
}
