#![windows_subsystem = "windows"]

mod config;
mod hotkey;
mod palette;
mod system;
mod ui;

use std::borrow::Cow;

use gpui_kit::assets::{Assets, icon_assets};
use gpui_kit::component::TitleBar;
use gpui_kit::{AppContext, AssetSource, Bounds, SharedString, TitlebarOptions, WindowBounds, WindowOptions, px, size};

icon_assets!(
    WispIcons,
    [
        CircleAlert,
        CircleCheck,
        FileCog,
        FolderSearch,
        Github,
        Globe,
        Keyboard,
        Layers,
        Minus,
        Plus,
        Power,
        Rows3,
        SunMoon,
        TriangleAlert,
        X,
    ]
);

/// The component library's icons plus the ones the modules use.
struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<Cow<'static, [u8]>>> {
        match WispIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        let mut paths = WispIcons.list(path)?;
        paths.extend(Assets.list(path)?);
        Ok(paths)
    }
}

fn main() {
    let welcome = std::env::args().any(|argument| argument == "--welcome");
    gpui_kit::application().with_assets(AppAssets).run(move |cx| {
        gpui_kit::init(cx);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let (width, height) = if welcome { (640., 600.) } else { (860., 720.) };
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(width), px(height)),
                cx,
            ))),
            titlebar: Some(TitlebarOptions {
                title: Some(if welcome { "Welcome to Wisp" } else { "Wisp Settings" }.into()),
                ..TitleBar::title_bar_options()
            }),
            window_min_size: Some(size(px(600.), px(480.))),
            // The window draws its own title bar in the app colors.
            ..TitleBar::window_options()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| ui::Settings::new(welcome, window, cx))
        })
        .expect("failed to open Wisp Settings");
        cx.activate(true);
    });
}
