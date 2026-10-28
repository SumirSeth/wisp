#![windows_subsystem = "windows"]

mod config;
mod hotkey;
mod system;
mod ui;

use gpui_kit::{AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

fn main() {
    let welcome = std::env::args().any(|argument| argument == "--welcome");
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(760.), px(540.)),
                    cx,
                ))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Wisp Settings".into()),
                    ..Default::default()
                }),
                window_min_size: Some(size(px(640.), px(440.))),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| ui::Settings::new(welcome, window, cx))
            })
            .expect("failed to open Wisp Settings");
            cx.activate(true);
        });
}
