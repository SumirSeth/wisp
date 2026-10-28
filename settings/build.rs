fn main() {
    println!("cargo:rerun-if-changed=../assets/wisp.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon_with_id("../assets/wisp.ico", "1");
        res.set("FileDescription", "Wisp Settings");
        res.set("ProductName", "Wisp");
        res.set("LegalCopyright", "Copyright (c) 2026 Sumir Seth. MIT License.");
        res.compile().expect("failed to embed Windows resources");
    }
}
