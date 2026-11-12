#![windows_subsystem = "windows"]

mod assets;
#[path = "../../src/dock/pins.rs"]
mod pins;

// Shared with the launcher; this app uses only some of the names.
#[allow(dead_code)]
#[path = "../../src/channel.rs"]
mod channel;
mod config;
mod hotkey;
#[path = "../../src/install.rs"]
mod install;
mod palette;
mod system;
mod ui;
mod update;
#[path = "../../src/update_notice.rs"]
mod update_notice;

use gpui_kit::{AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--check-updates") {
        update::check_hidden();
        return;
    }
    let welcome = args.iter().any(|a| a == "--welcome");
    // `--page search` opens a page directly, for screenshots and testing.
    let page = args
        .iter()
        .position(|a| a == "--page")
        .and_then(|i| args.get(i + 1))
        .cloned();
    update::remove_old_files();
    gpui_kit::application()
        .with_assets(crate::assets::SettingsAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let (title, width, height) = if welcome {
                ("Welcome to Wisp", 560., 460.)
            } else {
                ("Wisp Settings", 820., 580.)
            };
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(width), px(height)),
                    cx,
                ))),
                titlebar: Some(TitlebarOptions {
                    title: Some(title.into()),
                    ..Default::default()
                }),
                window_min_size: Some(size(px(520.), px(420.))),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| ui::Settings::new(welcome, page.as_deref(), window, cx))
            })
            .expect("failed to open Wisp Settings");
            cx.activate(true);
        });
}
