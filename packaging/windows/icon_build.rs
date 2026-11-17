//! Build script shared by `wiringcraft-app` and `wiringcraft` (their Cargo.toml `build` key).
//! A Windows build gets WiringCraft's icon (assets/icon, made by tools/gen_icon.py) as its
//! icon resource, so Explorer, the taskbar and the Start menu show it. Other systems take the
//! icon from their package (see packaging/).
//!
//! The target system decides, not the build machine (`CARGO_CFG_TARGET_OS`), so a Windows
//! program built elsewhere gets its icon too when a resource compiler is available.
fn main() {
    // Both programs live at crates/<name>, so the path is the same for each.
    let ico = "../../assets/icon/wiringcraft.ico";
    println!("cargo:rerun-if-changed={ico}");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico);
    // A missing resource compiler only costs the icon, never the build.
    if let Err(e) = res.compile() {
        println!("cargo:warning=the program is built without its icon: {e}");
    }
}
