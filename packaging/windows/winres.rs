//! Shared by the `build.rs` of the Windows programs (include it with
//! `#[path]`): embeds the app icon and version information, so Explorer,
//! shortcuts, the taskbar and Task Manager show them. Needs rc.exe from the
//! Windows SDK; without it the build goes on with a warning.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Embed `ico` and version info into the crate's binaries.
pub fn embed(ico_data: &[u8], name: &str, description: &str) {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let ico = out.join("app.ico");
    std::fs::write(&ico, ico_data).expect("write app.ico");

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
      VALUE "CompanyName", "Elvin Huseynov"
      VALUE "LegalCopyright", "© Elvin Huseynov"
      VALUE "FileDescription", "{description}"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "{name}"
      VALUE "OriginalFilename", "{name}.exe"
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
