//! Embeds the icon and version information in RemoteDesktopRider.exe.

#[path = "../rider-core/src/icon.rs"]
#[allow(dead_code)]
mod icon;
#[path = "../../packaging/windows/winres.rs"]
mod winres;

fn main() {
    println!("cargo:rerun-if-changed=../rider-core/src/icon.rs");
    println!("cargo:rerun-if-changed=../../packaging/windows/winres.rs");
    println!("cargo:rerun-if-changed=build.rs");
    winres::embed(&icon::ico(&[16, 20, 24, 32, 40, 48, 64, 128]), "RemoteDesktopRider", "Remote Desktop Rider");
}
