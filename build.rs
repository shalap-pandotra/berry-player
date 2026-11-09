// Bakes the icon into berry.exe on Windows, so it shows in Explorer, the taskbar, and alt-tab.
fn main() {
    println!("cargo:rerun-if-changed=assets/berry.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/berry.ico");
        // an icon isn't worth failing the build over
        if let Err(e) = res.compile() {
            println!("cargo:warning=couldn't embed the icon: {e}");
        }
    }
}
