//! Embeds the icon and version information in wisp.exe.

fn main() {
    println!("cargo:rerun-if-changed=assets/wisp.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        // Wisp loads resource ID 1 for the tray and launcher icons.
        res.set_icon_with_id("assets/wisp.ico", "1");
        res.set("FileDescription", "Wisp launcher");
        res.set("ProductName", "Wisp");
        res.set("LegalCopyright", "Copyright (c) 2026 Sumir Seth. MIT License.");
        res.compile().expect("failed to embed Windows resources");
    }
}
