//! Embeds the icon and version information in RemoteDesktopRider.exe, so
//! Explorer, shortcuts, the taskbar and Task Manager show them.

#[path = "src/icon.rs"]
mod icon;

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=src/icon.rs");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let ico = out.join("app.ico");
    std::fs::write(&ico, icon::ico(&[16, 20, 24, 32, 40, 48, 64, 128])).expect("write app.ico");

    let version = std::env::var("CARGO_PKG_VERSION").expect("version");
    let mut parts: Vec<u16> = version.split(['.', '-']).take(3).map(|p| p.parse().unwrap_or(0)).collect();
    parts.resize(3, 0);
    let numeric = format!("{},{},{},0", parts[0], parts[1], parts[2]);
    let rc = out.join("app.rc");
    std::fs::write(
        &rc,
        format!(
            r#"1 ICON "{ico}"
1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Remote Desktop Rider"
      VALUE "FileDescription", "Remote Desktop Rider"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "RemoteDesktopRider"
      VALUE "OriginalFilename", "RemoteDesktopRider.exe"
      VALUE "ProductName", "Remote Desktop Rider"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
            ico = ico.display().to_string().replace('\\', "/"),
        ),
    )
    .expect("write app.rc");

    let Some(rc_exe) = find_rc() else {
        println!("cargo:warning=rc.exe (Windows SDK) not found: the executable gets no icon");
        return;
    };
    let res = out.join("app.res");
    let status = Command::new(&rc_exe).arg("/nologo").arg("/fo").arg(&res).arg(&rc).status().expect("run rc.exe");
    assert!(status.success(), "rc.exe failed");
    // link.exe takes compiled resources as plain inputs.
    println!("cargo:rustc-link-arg-bins={}", res.display());
}

/// rc.exe from $RC or the newest Windows 10/11 SDK.
fn find_rc() -> Option<PathBuf> {
    if let Some(rc) = std::env::var_os("RC") {
        return Some(rc.into());
    }
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86") => "x86",
        _ => "x64",
    };
    let bin = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(bin).ok()?.filter_map(|e| Some(e.ok()?.path())).collect();
    versions.sort();
    versions.into_iter().rev().map(|v| v.join(arch).join("rc.exe")).find(|p| p.is_file())
}
