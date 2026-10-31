// Embeds the neoprint icon in the Windows executable, so Explorer, the taskbar,
// shortcuts and the installer show the app icon. A no-op on other platforms.
fn main() {
    println!("cargo:rerun-if-changed=assets/neoprint.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/neoprint.ico");
        res.set("ProductName", "neoprint");
        res.set("FileDescription", "neoprint");
        res.compile().expect("embed assets/neoprint.ico in neoprint.exe");
    }
}
