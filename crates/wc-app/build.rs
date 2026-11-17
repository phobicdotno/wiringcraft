//! Gives the Windows program WiringCraft's icon (assets/icon, made by tools/gen_icon.py), so
//! Explorer, the taskbar and the Start menu show it. Other systems take the icon from their
//! package (see packaging/).
fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon/wiringcraft.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon/wiringcraft.ico");
        // A missing resource compiler only costs the icon, never the build.
        if let Err(e) = res.compile() {
            println!("cargo:warning=the program is built without its icon: {e}");
        }
    }
}
